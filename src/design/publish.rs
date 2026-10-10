//! Publishing the draft and the version history: every publish keeps a full
//! snapshot of the draft under `design-history/{id}/` (`id` = the UTC
//! RFC 3339 publish time, so ids sort chronologically) with a `meta.json`
//! ([`HistoryEntry`]), never deleted. Restoring a version copies it into the
//! draft, never straight to live.
//!
//! A publish runs under `reload_lock` and the draft lock: complete any
//! pending publish → check the draft (initialized, compiles and passes the
//! strict smoke render (`templates::smoke`) as it would go live, something to
//! publish, live `design/` still at the draft's base unless forced) →
//! snapshot → pending marker → mirror the draft to `design/` → reload →
//! `meta.json`, drop the marker, re-base the draft. Visitors switch designs
//! only in the reload's RAM swap, so the running site never serves a
//! half-mirrored `design/`. A failed mirror or reload puts the previous
//! `design/` objects back; a marker left behind (crash, failed restore) is
//! completed by the next reload, publish or start, which rolls the validated
//! snapshot forward.

use bytes::Bytes;
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use futures_util::future::try_join_all;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use super::DesignStore;
use super::draft::{changes, mirror, read_meta, rebase};
use super::stored::{
    Cache, DESIGN_PREFIX, DesignError, Files, files_of, key, load_prefix, validate_for_publish,
};
use crate::storage::Storage;
use crate::templates::Templates;
use crate::templates::smoke::{SmokeError, smoke_render};

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

fn version_id(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Validate exactly what would go live (`view` = the draft over the baked
/// bundle): every template compiles, then the strict smoke render against
/// the site's real data reports no template error.
async fn check_view(
    db: &DatabaseConnection,
    storage: &Storage,
    view: &Files,
) -> Result<(), DesignError> {
    let errors = validate_for_publish(view);
    if !errors.is_empty() {
        return Err(DesignError::Invalid(errors));
    }
    let files = view.clone();
    let load = move |path: &str| files.get(path).map(|bytes| bytes.to_vec());
    let report = smoke_render(db, storage, load).await.map_err(|e| {
        tracing::error!("design publish: smoke render failed: {e:#}");
        DesignError::RenderCheck
    })?;
    if report.is_ok() {
        return Ok(());
    }
    Err(DesignError::Invalid(
        report.errors.iter().map(smoke_message).collect(),
    ))
}

/// `templates/{name}:{line}: {message} (rendering {case})`, in the same
/// `templates/…` path form as the compile errors.
fn smoke_message(e: &SmokeError) -> String {
    let line = e.line.map(|l| format!(":{l}")).unwrap_or_default();
    format!(
        "templates/{}{line}: {} (rendering {})",
        e.template, e.message, e.case
    )
}

impl DesignStore {
    /// Publish the draft as user `by`; `force` overwrites `design/` changes
    /// made outside the draft ([`DesignError::Conflict`]). Every rejection
    /// (`Invalid`, `Conflict`, `NothingToPublish`) leaves everything
    /// untouched; see the module doc for the failure guarantees.
    pub async fn publish(
        &self,
        db: &DatabaseConnection,
        storage: &Storage,
        templates: &Templates,
        by: &str,
        force: bool,
    ) -> Result<HistoryEntry, DesignError> {
        let _reload = self.reload_lock.lock().await;
        if self.recover_locked(storage).await?.is_some() {
            self.reload_locked(storage, templates).await?;
        }
        let mut cache = self.draft.lock().await;
        let Some(meta) = read_meta(storage).await? else {
            return Err(DesignError::NothingToPublish);
        };
        let files = self.draft_files(storage, &mut cache).await?;
        check_view(db, storage, &self.with_baked(&files)).await?;
        let live = self.live_files(storage).await?;
        if changes(&self.with_baked(&files), &self.with_baked(&live)).is_empty() {
            return Err(DesignError::NothingToPublish);
        }
        let external = meta.external_changes(&live);
        if !external.is_empty() && !force {
            return Err(DesignError::Conflict(external));
        }

        let entry = HistoryEntry {
            files: files.len(),
            by: by.to_string(),
            ..unused_version(storage, Utc::now()).await?
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
                Ok(()) => true,
                Err(e) => {
                    tracing::error!("design publish {}: restoring design/ failed: {e}", entry.id);
                    false
                }
            };
            // Only a restored design/ may drop the marker: otherwise the next
            // reload must roll the half-mirrored design/ forward.
            let pending = !restored || storage.delete(PENDING_KEY).await.is_err();
            tracing::error!("design publish {} failed: {error}", entry.id);
            return Err(DesignError::PublishFailed {
                error: Box::new(error),
                restored,
                pending,
            });
        }
        // Live already: a failure here only delays the history entry to the
        // next reload, publish or start, which finds the marker and completes
        // it; a stale draft base only makes the next publish ask for force.
        if let Err(e) = finish(storage, &entry).await {
            tracing::error!("design publish {}: recording it failed: {e}", entry.id);
        }
        if let Err(e) = rebase(storage, &files).await {
            tracing::error!(
                "design publish {}: re-basing the draft failed: {e}",
                entry.id
            );
        }
        tracing::info!(id = %entry.id, by, files = entry.files, "design published");
        Ok(entry)
    }

    /// Complete a publish whose marker is still set: mirror its snapshot to
    /// `design/`, record it and re-base the draft on it if the draft still
    /// is that snapshot. The caller holds `reload_lock` (not the draft lock)
    /// and reloads afterwards.
    pub(super) async fn recover_locked(
        &self,
        storage: &Storage,
    ) -> Result<Option<HistoryEntry>, DesignError> {
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
        let snapshot = snapshot_prefix(&entry.id);
        let files = files_of(&load_prefix(storage, &snapshot, &Cache::new()).await?);
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
        {
            // Lock order reload_lock → draft, as in `publish`.
            let mut cache = self.draft.lock().await;
            // Only a draft that still is this snapshot moves its base along;
            // one changed since (discarded, restored, edited) keeps its base,
            // so its next publish reports the rolled-forward paths as a
            // conflict instead of silently reverting them.
            if read_meta(storage).await?.is_some()
                && self.draft_files(storage, &mut cache).await? == files
            {
                rebase(storage, &files).await?;
            }
        }
        finish(storage, &entry).await?;
        tracing::warn!(id = %entry.id, "completed an unfinished design publish");
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

    /// Replace the draft with published version `id` and base it on the
    /// current `design/`.
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
        self.draft_reset(storage, Some(files)).await
    }
}

/// A history entry stamped `at`, its id bumped by a microsecond while
/// `design-history/{id}/` already holds anything (a clock that went back).
async fn unused_version(
    storage: &Storage,
    mut at: DateTime<Utc>,
) -> Result<HistoryEntry, DesignError> {
    while !storage
        .list(&snapshot_prefix(&version_id(at)))
        .await?
        .is_empty()
    {
        at += TimeDelta::microseconds(1);
    }
    Ok(HistoryEntry {
        id: version_id(at),
        at,
        by: String::new(),
        files: 0,
    })
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
    fn smoke_errors_read_like_compile_errors() {
        let mut e = SmokeError {
            template: "markdown/fen.html".into(),
            line: Some(2),
            message: "undefined value".into(),
            case: "page `x` (anonymous)".into(),
        };
        assert_eq!(
            smoke_message(&e),
            "templates/markdown/fen.html:2: undefined value (rendering page `x` (anonymous))"
        );
        e.line = None;
        assert!(smoke_message(&e).starts_with("templates/markdown/fen.html: undefined"));
    }

    #[test]
    fn version_ids_are_single_segment_rfc3339() {
        let id = version_id(Utc::now());
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
        assert!(version_id(earlier) < version_id(later));
        assert!(version_id(later) < version_id(later + TimeDelta::microseconds(1)));
    }

    #[tokio::test]
    async fn version_id_collisions_are_bumped() {
        let dir = std::env::temp_dir().join(format!("design-ids-{}", uuid::Uuid::new_v4()));
        let storage =
            Storage::local(&dir, sea_orm::DatabaseConnection::Disconnected).expect("fs storage");
        let at: DateTime<Utc> = "2026-10-10T12:00:00Z".parse().expect("ts");
        let taken = |t| key(&snapshot_prefix(&version_id(t)), "templates/x.html");
        for t in [at, at + TimeDelta::microseconds(1)] {
            storage.put(&taken(t), "x".into()).await.expect("put");
        }
        let entry = unused_version(&storage, at).await.expect("version");
        assert_eq!(entry.id, version_id(at + TimeDelta::microseconds(2)));
        assert_eq!(entry.at, at + TimeDelta::microseconds(2));
        let free = unused_version(&storage, at - TimeDelta::seconds(1)).await;
        assert_eq!(free.expect("version").at, at - TimeDelta::seconds(1));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
