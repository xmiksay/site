//! The shared design draft: `design-draft/{path}` objects, one draft per
//! site, plus its meta object `design-draft.json` ([`DraftMeta`]): present
//! means initialized. Until its first mutation the draft reads as the
//! published view and nothing is written; the first mutation copies the
//! published view (baked ∪ `design/`) in, so from then on the draft holds the
//! full bundle. Like the published design it is read over the baked bundle:
//! a path missing from the draft shows (and publishes as) its baked default,
//! so deleting a baked file from the draft reverts it, and an initialized
//! draft with no files is the pure baked bundle.
//!
//! The meta records the draft's **base**: the sha256 of every `design/`
//! object it was started from. A publish compares live `design/` with it to
//! catch edits made outside the draft (bucket edits, a stale publish).
//!
//! Every mutation runs under `DesignStore.draft`, which also caches the
//! draft's bytes by version.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::stored::{
    Cache, DESIGN_PREFIX, DesignError, Files, StoredFile, check_path, files_of, key, load_prefix,
};
use super::{DesignStore, baked_view};
use crate::files::hash_blob;
use crate::storage::Storage;

/// Storage key prefix of the draft.
pub const DRAFT_PREFIX: &str = "design-draft";

/// The draft's meta object; outside the bundle roots and the draft prefix.
pub const DRAFT_META_KEY: &str = "design-draft.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

/// One path whose draft view differs from the published view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
}

/// The draft as stored plus what publishing it would change.
pub struct Draft {
    /// False until the first mutation; `files` is then the published view.
    pub initialized: bool,
    /// The draft's own objects (not merged with baked).
    pub files: Files,
    /// Its view against the published view, sorted by path.
    pub changes: Vec<FileChange>,
}

/// `design-draft.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftMeta {
    /// When the draft was last (re)based.
    pub at: DateTime<Utc>,
    /// sha256 of every `design/` object the draft is based on, by path.
    pub base: BTreeMap<String, String>,
}

impl DraftMeta {
    pub fn based_on(live: &Files) -> Self {
        Self {
            at: Utc::now(),
            base: live
                .iter()
                .map(|(path, bytes)| (path.clone(), hash_blob(bytes)))
                .collect(),
        }
    }

    /// The `design/` paths that changed since the draft's base, sorted.
    pub fn external_changes(&self, live: &Files) -> Vec<String> {
        let now = Self::based_on(live).base;
        let paths: BTreeSet<&String> = self.base.keys().chain(now.keys()).collect();
        paths
            .into_iter()
            .filter(|p| self.base.get(*p) != now.get(*p))
            .cloned()
            .collect()
    }
}

/// What `source` differs from `target` by, sorted by path.
pub fn changes(source: &Files, target: &Files) -> Vec<FileChange> {
    let mut out: Vec<FileChange> = source
        .iter()
        .filter_map(|(path, bytes)| {
            let kind = match target.get(path) {
                None => ChangeKind::Added,
                Some(old) if old != bytes => ChangeKind::Modified,
                Some(_) => return None,
            };
            Some(FileChange {
                path: path.clone(),
                kind,
            })
        })
        .chain(
            target
                .keys()
                .filter(|p| !source.contains_key(*p))
                .map(|path| FileChange {
                    path: path.clone(),
                    kind: ChangeKind::Deleted,
                }),
        )
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Make the objects under `prefix/` hold exactly `source`, given that they
/// hold `target` now: write what differs, then delete what `source` lacks.
pub(super) async fn mirror(
    storage: &Storage,
    prefix: &str,
    source: &Files,
    target: &Files,
) -> Result<(), DesignError> {
    for change in changes(source, target) {
        let key = key(prefix, &change.path);
        match source.get(&change.path) {
            Some(bytes) => storage.put(&key, bytes.clone()).await?,
            None => storage.delete(&key).await?,
        }
    }
    Ok(())
}

/// The draft's meta, `None` while uninitialized. An unreadable meta keeps
/// the draft (an empty base: the next publish reports every live file as
/// changed outside the draft) rather than re-initializing over it.
pub async fn read_meta(storage: &Storage) -> Result<Option<DraftMeta>, DesignError> {
    let Some(raw) = storage.get(DRAFT_META_KEY).await? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_slice(&raw).unwrap_or_else(|e| {
        tracing::warn!("unreadable {DRAFT_META_KEY}, treating its base as empty: {e}");
        DraftMeta {
            at: Utc::now(),
            base: BTreeMap::new(),
        }
    })))
}

/// Base the draft on `live` (the current `design/` objects).
pub(super) async fn rebase(storage: &Storage, live: &Files) -> Result<(), DesignError> {
    let json = serde_json::to_vec(&DraftMeta::based_on(live))
        .expect("a DraftMeta (a timestamp and a string map) always serializes");
    storage.put(DRAFT_META_KEY, json.into()).await?;
    Ok(())
}

/// Initialize the draft from `design/` in storage unless it already is. The
/// meta is written last, so an interrupted init is redone.
pub async fn ensure_init(storage: &Storage) -> Result<(), DesignError> {
    if read_meta(storage).await?.is_some() {
        return Ok(());
    }
    let live = files_of(&load_prefix(storage, DESIGN_PREFIX, &Cache::new()).await?);
    let current = files_of(&load_prefix(storage, DRAFT_PREFIX, &Cache::new()).await?);
    mirror(storage, DRAFT_PREFIX, &baked_view(&live), &current).await?;
    rebase(storage, &live).await?;
    tracing::info!("design draft initialized");
    Ok(())
}

impl DesignStore {
    /// The draft and its changes against the published design. Never writes.
    pub async fn draft(&self, storage: &Storage) -> Result<Draft, DesignError> {
        let mut cache = self.draft.lock().await;
        if read_meta(storage).await?.is_none() {
            let files = self.published_view();
            return Ok(Draft {
                initialized: false,
                files,
                changes: Vec::new(),
            });
        }
        let files = self.draft_files(storage, &mut cache).await?;
        let changes = changes(&self.with_baked(&files), &self.published_view());
        Ok(Draft {
            initialized: true,
            files,
            changes,
        })
    }

    /// One file of the draft view: the draft's copy, else the baked default.
    pub async fn draft_read(
        &self,
        storage: &Storage,
        path: &str,
    ) -> Result<Option<Bytes>, DesignError> {
        check_path(path)?;
        let draft = self.draft(storage).await?;
        Ok(draft
            .files
            .get(path)
            .cloned()
            .or_else(|| self.baked(path).map(Bytes::from)))
    }

    pub async fn draft_put(
        &self,
        storage: &Storage,
        path: &str,
        bytes: Bytes,
    ) -> Result<(), DesignError> {
        check_path(path)?;
        let mut cache = self.draft.lock().await;
        ensure_init(storage).await?;
        storage.put(&key(DRAFT_PREFIX, path), bytes.clone()).await?;
        let file = StoredFile {
            bytes,
            version: None,
        };
        cache.insert(path.to_string(), file);
        Ok(())
    }

    /// Remove the draft's copy of `path`: a baked file reverts to its
    /// default, any other file is gone.
    pub async fn draft_delete(&self, storage: &Storage, path: &str) -> Result<(), DesignError> {
        check_path(path)?;
        let mut cache = self.draft.lock().await;
        ensure_init(storage).await?;
        let files = self.draft_files(storage, &mut cache).await?;
        if !files.contains_key(path) {
            return Err(DesignError::NotInDraft(path.to_string()));
        }
        storage.delete(&key(DRAFT_PREFIX, path)).await?;
        cache.remove(path);
        Ok(())
    }

    /// Reset the draft to the published view and re-base it.
    pub async fn draft_discard(&self, storage: &Storage) -> Result<(), DesignError> {
        self.draft_reset(storage, None).await
    }

    /// Make the draft hold exactly `files` (the published view when `None`)
    /// and base it on the current `design/`.
    pub(super) async fn draft_reset(
        &self,
        storage: &Storage,
        files: Option<Files>,
    ) -> Result<(), DesignError> {
        let mut cache = self.draft.lock().await;
        let live = self.live_files(storage).await?;
        let files = files.unwrap_or_else(|| baked_view(&live));
        let current = files_of(&load_prefix(storage, DRAFT_PREFIX, &cache).await?);
        // Clear the cache first: a failure mid-mirror leaves storage
        // partially written, and the next load must see it as it is.
        cache.clear();
        mirror(storage, DRAFT_PREFIX, &files, &current).await?;
        rebase(storage, &live).await?;
        *cache = load_prefix(storage, DRAFT_PREFIX, &Cache::new()).await?;
        Ok(())
    }

    /// The initialized draft's files. The caller holds `self.draft` (passed
    /// as `cache`).
    pub(super) async fn draft_files(
        &self,
        storage: &Storage,
        cache: &mut Cache,
    ) -> Result<Files, DesignError> {
        *cache = load_prefix(storage, DRAFT_PREFIX, cache).await?;
        Ok(files_of(cache))
    }

    /// What the public site serves (without `DESIGN_DIR`): baked ∪ `design/`.
    pub fn published_view(&self) -> Files {
        self.with_baked(&self.published_files())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entries: &[(&str, &str)]) -> Files {
        entries
            .iter()
            .map(|(p, b)| (p.to_string(), Bytes::from(b.to_string())))
            .collect()
    }

    #[test]
    fn changes_list_added_modified_and_deleted_paths_sorted() {
        let published = files(&[
            ("assets/a.css", "same"),
            ("templates/b.html", "old"),
            ("templates/gone.html", "x"),
        ]);
        let draft = files(&[
            ("assets/a.css", "same"),
            ("templates/b.html", "new"),
            ("assets/new.css", "n"),
        ]);
        let got = changes(&draft, &published);
        let want = [
            ("assets/new.css", ChangeKind::Added),
            ("templates/b.html", ChangeKind::Modified),
            ("templates/gone.html", ChangeKind::Deleted),
        ];
        assert_eq!(got.len(), want.len(), "{got:?}");
        for (change, (path, kind)) in got.iter().zip(want) {
            assert_eq!((change.path.as_str(), change.kind), (path, kind));
        }
        assert!(changes(&draft, &draft).is_empty());
    }

    #[test]
    fn external_changes_compare_live_with_the_base() {
        let base = files(&[("assets/a.css", "a"), ("templates/b.html", "b")]);
        let meta = DraftMeta::based_on(&base);
        assert!(meta.external_changes(&base).is_empty());
        let live = files(&[("assets/a.css", "edited"), ("assets/new.css", "n")]);
        assert_eq!(
            meta.external_changes(&live),
            ["assets/a.css", "assets/new.css", "templates/b.html"]
        );
    }

    #[test]
    fn draft_view_falls_back_to_baked() {
        let store = DesignStore::new(None);
        let baked_404 = store.baked("templates/404.html").expect("baked 404");
        let view = store.with_baked(&files(&[("assets/extra.css", "x")]));
        assert_eq!(
            view.get("templates/404.html").map(|b| b.to_vec()),
            Some(baked_404)
        );
        assert!(view.contains_key("assets/extra.css"));
        // The published view of a store with no overrides is the baked bundle.
        assert_eq!(store.published_view(), store.with_baked(&Files::new()));
    }
}
