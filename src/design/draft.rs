//! The shared design draft: `design-draft/{path}` objects, one draft per
//! site. Its first access copies the published view (baked ∪ `design/`) in,
//! so from then on it holds the full bundle. Like the published design, it is
//! read over the baked bundle: a path missing from the draft shows (and
//! publishes as) its baked default, so deleting a baked file from the draft
//! reverts it rather than removing it.
//!
//! Every mutation runs under `DesignStore.draft`, which also caches the
//! draft's bytes by version.

use bytes::Bytes;
use serde::Serialize;

use super::DesignStore;
use super::stored::{
    Cache, DesignError, Files, StoredFile, check_path, files_of, key, load_prefix,
};
use crate::storage::Storage;

/// Storage key prefix of the draft.
pub const DRAFT_PREFIX: &str = "design-draft";

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
    /// The draft's own objects (not merged with baked).
    pub files: Files,
    /// Its view against the published view, sorted by path.
    pub changes: Vec<FileChange>,
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

impl DesignStore {
    /// The draft and its changes against the published design.
    pub async fn draft(&self, storage: &Storage) -> Result<Draft, DesignError> {
        let mut cache = self.draft.lock().await;
        let files = self.draft_files(storage, &mut cache).await?;
        let changes = changes(&self.with_baked(&files), &self.published_view());
        Ok(Draft { files, changes })
    }

    /// One file of the draft view: the draft's copy, else the baked default.
    pub async fn draft_read(
        &self,
        storage: &Storage,
        path: &str,
    ) -> Result<Option<Bytes>, DesignError> {
        check_path(path)?;
        let mut cache = self.draft.lock().await;
        let files = self.draft_files(storage, &mut cache).await?;
        Ok(files
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
        self.draft_files(storage, &mut cache).await?;
        storage.put(&key(DRAFT_PREFIX, path), bytes.clone()).await?;
        cache.insert(
            path.to_string(),
            StoredFile {
                bytes,
                version: None,
            },
        );
        Ok(())
    }

    /// Remove the draft's copy of `path`: a baked file reverts to its
    /// default, any other file is gone.
    pub async fn draft_delete(&self, storage: &Storage, path: &str) -> Result<(), DesignError> {
        check_path(path)?;
        let mut cache = self.draft.lock().await;
        let files = self.draft_files(storage, &mut cache).await?;
        if !files.contains_key(path) {
            return Err(DesignError::NotInDraft(path.to_string()));
        }
        storage.delete(&key(DRAFT_PREFIX, path)).await?;
        cache.remove(path);
        Ok(())
    }

    /// Reset the draft to the published view.
    pub async fn draft_discard(&self, storage: &Storage) -> Result<(), DesignError> {
        self.draft_replace(storage, &self.published_view()).await
    }

    /// Make the draft hold exactly `files`.
    pub(super) async fn draft_replace(
        &self,
        storage: &Storage,
        files: &Files,
    ) -> Result<(), DesignError> {
        let mut cache = self.draft.lock().await;
        let current = files_of(&load_prefix(storage, DRAFT_PREFIX, &cache).await?);
        // Clear the cache first: a failure mid-mirror leaves storage
        // partially written, and the next load must see it as it is.
        cache.clear();
        mirror(storage, DRAFT_PREFIX, files, &current).await?;
        *cache = load_prefix(storage, DRAFT_PREFIX, &Cache::new()).await?;
        Ok(())
    }

    /// The draft's files, copying the published view in on first access
    /// (an empty draft). The caller holds `self.draft` (passed as `cache`).
    pub(super) async fn draft_files(
        &self,
        storage: &Storage,
        cache: &mut Cache,
    ) -> Result<Files, DesignError> {
        *cache = load_prefix(storage, DRAFT_PREFIX, cache).await?;
        if cache.is_empty() {
            let published = self.published_view();
            mirror(storage, DRAFT_PREFIX, &published, &Files::new()).await?;
            tracing::info!(files = published.len(), "design draft initialized");
            *cache = published
                .into_iter()
                .map(|(path, bytes)| {
                    let file = StoredFile {
                        bytes,
                        version: None,
                    };
                    (path, file)
                })
                .collect();
        }
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
