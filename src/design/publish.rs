//! Publishing the draft and the version history: every publish keeps a full
//! snapshot of the draft under `design-history/{id}/` (`id` = the UTC
//! RFC 3339 publish time, so ids sort chronologically) with a `meta.json`
//! ([`HistoryEntry`]), never deleted. Restoring a version copies it into the
//! draft, never straight to live.
//!
//! A publish runs under `reload_lock` and the draft lock: validate the draft
//! view → snapshot the draft → write the pending marker → mirror the draft to
//! `design/` → reload → write `meta.json` and drop the marker. Visitors switch
//! designs only in the reload's RAM swap, so the running site never serves a
//! half-mirrored `design/`. A failed mirror or reload puts the previous
//! `design/` objects back; a crash after the marker is completed by the next
//! start ([`DesignStore::recover_publish`]), which rolls the validated
//! snapshot forward.

use bytes::Bytes;
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::future::try_join_all;
use serde::{Deserialize, Serialize};

use super::DesignStore;
use super::draft::mirror;
use super::stored::{
    Cache, DESIGN_PREFIX, DesignError, Files, files_of, key, load_prefix, validate_for_publish,
};
use crate::storage::Storage;
use crate::templates::Templates;

/// Storage key prefix of the version history.
pub const HISTORY_PREFIX: &str = "design-history";

/// The [`HistoryEntry`] of a publish between its snapshot and its
/// `meta.json`: present means `design/` may not match that snapshot yet.
const PENDING_KEY: &str = "design-publish-pending.json";

const META: &str = "meta.json";

/// One published version, stored as its snapshot's `meta.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub at: DateTime<Utc>,
    /// The publisher's username.
    pub by: String,
    pub files: usize,
}

impl HistoryEntry {
    fn json(&self) -> Bytes {
        serde_json::to_vec(self)
            .expect("a HistoryEntry (strings, a timestamp, a count) always serializes")
            .into()
    }
}

fn snapshot_prefix(id: &str) -> String {
    format!("{HISTORY_PREFIX}/{id}")
}

/// A version id names one key segment: a UTC RFC 3339 timestamp.
fn valid_id(id: &str) -> bool {
    !id.contains('/') && DateTime::parse_from_rfc3339(id).is_ok()
}

impl DesignStore {
    /// Publish the draft as user `by`. A draft that fails validation leaves
    /// everything untouched ([`DesignError::Invalid`]); see the module doc
    /// for the failure guarantees.
    pub async fn publish(
        &self,
        storage: &Storage,
        templates: &Templates,
        by: &str,
    ) -> Result<HistoryEntry, DesignError> {
        let _reload = self.reload_lock.lock().await;
        let mut cache = self.draft.lock().await;
        let files = self.draft_files(storage, &mut cache).await?;
        let errors = validate_for_publish(&self.with_baked(&files));
        if !errors.is_empty() {
            return Err(DesignError::Invalid(errors));
        }
        let current = self.stored.read().clone();
        let live = files_of(&load_prefix(storage, DESIGN_PREFIX, &current.files).await?);

        let at = Utc::now();
        let entry = HistoryEntry {
            id: at.to_rfc3339_opts(SecondsFormat::Micros, true),
            at,
            by: by.to_string(),
            files: files.len(),
        };
        let snapshot = snapshot_prefix(&entry.id);
        for (path, bytes) in &files {
            storage.put(&key(&snapshot, path), bytes.clone()).await?;
        }
        storage.put(PENDING_KEY, entry.json()).await?;

        let went_live = match mirror(storage, DESIGN_PREFIX, &files, &live).await {
            Ok(()) => self.reload_locked(storage, templates).await.map(|_| ()),
            Err(e) => Err(e),
        };
        if let Err(error) = went_live {
            // Partially mirrored paths hold either `live` or `files`.
            let restored = match mirror(storage, DESIGN_PREFIX, &live, &files).await {
                Ok(()) => storage.delete(PENDING_KEY).await.is_ok(),
                Err(e) => {
                    tracing::error!("design publish {}: restoring design/ failed: {e}", entry.id);
                    false
                }
            };
            tracing::error!("design publish {} failed: {error}", entry.id);
            return Err(DesignError::PublishFailed {
                error: Box::new(error),
                restored,
            });
        }
        // Live already: a failure here only delays the history entry to the
        // next start, which finds the marker and completes it.
        if let Err(e) = finish(storage, &entry).await {
            tracing::error!("design publish {}: recording it failed: {e}", entry.id);
        }
        tracing::info!(id = %entry.id, by, files = entry.files, "design published");
        Ok(entry)
    }

    /// Complete a publish interrupted by a crash (the pending marker is
    /// set): mirror its snapshot to `design/` and record it. Run at startup,
    /// before the first reload.
    pub async fn recover_publish(
        &self,
        storage: &Storage,
    ) -> Result<Option<HistoryEntry>, DesignError> {
        let _reload = self.reload_lock.lock().await;
        let Some(raw) = storage.get(PENDING_KEY).await? else {
            return Ok(None);
        };
        let entry = match serde_json::from_slice::<HistoryEntry>(&raw) {
            Ok(entry) if valid_id(&entry.id) => entry,
            _ => {
                tracing::error!("dropping an unreadable design publish marker");
                storage.delete(PENDING_KEY).await?;
                return Ok(None);
            }
        };
        let files =
            files_of(&load_prefix(storage, &snapshot_prefix(&entry.id), &Cache::new()).await?);
        // The marker is written after the whole snapshot; a mismatch means
        // the snapshot was tampered with, and mirroring it could wipe design/.
        if files.len() != entry.files {
            tracing::error!(
                id = %entry.id,
                "design publish snapshot incomplete ({} of {} files); leaving design/ as is",
                files.len(),
                entry.files
            );
            storage.delete(PENDING_KEY).await?;
            return Ok(None);
        }
        let live = files_of(&load_prefix(storage, DESIGN_PREFIX, &Cache::new()).await?);
        mirror(storage, DESIGN_PREFIX, &files, &live).await?;
        finish(storage, &entry).await?;
        tracing::warn!(id = %entry.id, "completed an interrupted design publish");
        Ok(Some(entry))
    }

    /// Every published version, newest first. Unreadable entries are skipped.
    pub async fn history(&self, storage: &Storage) -> Result<Vec<HistoryEntry>, DesignError> {
        let strip = format!("{HISTORY_PREFIX}/");
        let metas: Vec<String> = storage
            .list(HISTORY_PREFIX)
            .await?
            .into_iter()
            .filter(|o| {
                o.key
                    .strip_prefix(&strip)
                    .and_then(|rest| rest.strip_suffix("/meta.json"))
                    .is_some_and(|id| !id.contains('/'))
            })
            .map(|o| o.key)
            .collect();
        let raw = try_join_all(metas.iter().map(|k| storage.get(k))).await?;
        let mut out: Vec<HistoryEntry> = metas
            .iter()
            .zip(raw)
            .filter_map(|(k, raw)| match serde_json::from_slice(&raw?) {
                Ok(entry) => Some(entry),
                Err(e) => {
                    tracing::warn!(key = %k, "skipping unreadable design history entry: {e}");
                    None
                }
            })
            .collect();
        out.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(out)
    }

    /// Replace the draft with published version `id`.
    pub async fn restore(&self, storage: &Storage, id: &str) -> Result<(), DesignError> {
        let missing = || DesignError::NoVersion(id.to_string());
        if !valid_id(id) {
            return Err(missing());
        }
        let snapshot = snapshot_prefix(id);
        if storage.get(&key(&snapshot, META)).await?.is_none() {
            return Err(missing());
        }
        let files: Files = files_of(&load_prefix(storage, &snapshot, &Cache::new()).await?);
        self.draft_replace(storage, &files).await
    }
}

/// Record the publish in the history, then drop the pending marker.
async fn finish(storage: &Storage, entry: &HistoryEntry) -> Result<(), DesignError> {
    let meta = key(&snapshot_prefix(&entry.id), META);
    storage.put(&meta, entry.json()).await?;
    storage.delete(PENDING_KEY).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ids_are_single_segment_rfc3339() {
        let id = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        assert!(valid_id(&id), "{id}");
        for bad in [
            "",
            "latest",
            "2026-10-10T12:00:00Z/x",
            "../2026-10-10T12:00:00Z",
        ] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn version_ids_sort_chronologically() {
        let earlier: DateTime<Utc> = "2026-10-10T09:59:59.999999Z".parse().expect("ts");
        let later: DateTime<Utc> = "2026-10-10T10:00:00Z".parse().expect("ts");
        let fmt = |t: DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Micros, true);
        assert!(fmt(earlier) < fmt(later));
    }
}
