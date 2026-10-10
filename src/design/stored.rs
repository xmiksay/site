//! The published design: `design/{path}` objects in storage, `path`
//! mirroring the baked bundle (`templates/…`, `assets/…`, `mdcast/…`). A
//! publish (see [`publish`](super::publish)) mirrors the draft here; a
//! professional may also edit the objects straight in the bucket and reload.
//! Held in RAM between reloads.
//!
//! A reload lists `design/`, downloads only objects whose version changed,
//! syntax-checks every template, and only then swaps the overlay in — a
//! broken template leaves the running design untouched.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::DesignStore;
use super::publish::HistoryEntry;
use crate::storage::{self, Storage, Version};
use crate::templates::Templates;

/// Storage key prefix of the published design.
pub const DESIGN_PREFIX: &str = "design";

/// The bundle roots `DesignStore` serves; an override elsewhere is ignored.
pub const ROOTS: [&str; 3] = ["templates/", "assets/", "mdcast/"];

/// Bundle files by path, sorted.
pub type Files = BTreeMap<String, Bytes>;

/// Files loaded from one storage prefix, each with the version it was read
/// at so the next load downloads only what changed.
pub(super) type Cache = HashMap<String, StoredFile>;

#[derive(Default)]
pub(super) struct Stored {
    pub(super) files: Cache,
}

pub(super) struct StoredFile {
    pub(super) bytes: Bytes,
    /// `None` right after our own write: the next load downloads it once to
    /// learn its version.
    pub(super) version: Option<Version>,
}

pub(super) fn files_of(cache: &Cache) -> Files {
    cache
        .iter()
        .map(|(path, f)| (path.clone(), f.bytes.clone()))
        .collect()
}

/// Outcome of the last reload, shown in the admin.
#[derive(Debug, Clone, Serialize)]
pub struct ReloadStatus {
    pub at: DateTime<Utc>,
    pub ok: bool,
    pub files: usize,
    pub error: Option<String>,
    /// A publish left pending that this reload completed first.
    pub completed_publish: Option<HistoryEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum DesignError {
    #[error(transparent)]
    Storage(#[from] storage::Error),
    #[error("invalid design path {0:?}: must be a file under templates/, assets/ or mdcast/")]
    BadPath(String),
    #[error("{0} is not in the draft")]
    NotInDraft(String),
    #[error("no published design version {0:?}")]
    NoVersion(String),
    #[error("{}", .0.join("; "))]
    Invalid(Vec<String>),
    /// Live `design/` changed outside the draft since the draft's base
    /// (bucket edit, another publish): publishing would revert these paths.
    #[error(
        "design/ changed outside the draft since it was started: {}; discard the draft to adopt those changes, or publish with force=true to overwrite them",
        .0.join(", ")
    )]
    Conflict(Vec<String>),
    #[error("nothing to publish: the draft matches the live design")]
    NothingToPublish,
    /// The mirror to `design/` or the reload after it failed. `restored`:
    /// the previous `design/` objects were put back; `pending`: the publish
    /// marker is still set, so the next reload, publish or start completes
    /// this publish.
    #[error("publish failed ({}): {}", rollback_note(*.restored, *.pending), status_error(.error))]
    PublishFailed {
        error: Box<DesignError>,
        restored: bool,
        pending: bool,
    },
}

fn rollback_note(restored: bool, pending: bool) -> &'static str {
    match (restored, pending) {
        (true, false) => "previous design restored",
        (true, true) => {
            "previous design restored, but the publish marker could not be cleared: the next reload, publish or start completes this publish"
        }
        (false, _) => {
            "restoring the previous design failed: the next reload, publish or start completes this publish"
        }
    }
}

/// `path` names a file under one of the [`ROOTS`] and makes a valid key.
pub fn check_path(path: &str) -> Result<(), DesignError> {
    let under_root = ROOTS
        .iter()
        .any(|root| path.len() > root.len() && path.starts_with(root));
    if !under_root || storage::parse_key(&key(DESIGN_PREFIX, path)).is_err() {
        return Err(DesignError::BadPath(path.to_string()));
    }
    Ok(())
}

/// The error as an admin sees it. Storage failures stay generic: DB errors
/// carry SQL and table names, outages the backend's URLs. The full error
/// goes to the log.
pub(crate) fn status_error(err: &DesignError) -> String {
    match err {
        DesignError::Storage(storage::Error::Db(_)) => "database error".into(),
        DesignError::Storage(storage::Error::Unavailable(_)) => "storage unavailable".into(),
        other => other.to_string(),
    }
}

pub(super) fn key(prefix: &str, path: &str) -> String {
    format!("{prefix}/{path}")
}

/// Every bundle file under `prefix/`, reusing `cache`d bytes whose version is
/// unchanged. Objects outside the bundle roots are skipped.
pub(super) async fn load_prefix(
    storage: &Storage,
    prefix: &str,
    cache: &Cache,
) -> Result<Cache, DesignError> {
    let mut files = HashMap::new();
    let strip = format!("{prefix}/");
    for object in storage.list(prefix).await? {
        let Some(path) = object.key.strip_prefix(&strip) else {
            continue;
        };
        if check_path(path).is_err() {
            // A snapshot's `meta.json` sits next to its bundle roots.
            if path != "meta.json" {
                tracing::warn!(key = %object.key, "ignoring design object outside the bundle roots");
            }
            continue;
        }
        let cached = cache
            .get(path)
            .filter(|f| f.version.as_ref() == Some(&object.version));
        let bytes = match cached {
            Some(f) => f.bytes.clone(),
            // Gone between list and get: a concurrent delete, skip it.
            None => match storage.get(&object.key).await? {
                Some(bytes) => bytes,
                None => continue,
            },
        };
        let version = Some(object.version);
        files.insert(path.to_string(), StoredFile { bytes, version });
    }
    Ok(files)
}

/// Every override template must be UTF-8 and parse; one message per broken
/// file. Render-time errors (an unknown variable) are not caught here.
pub fn validate<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Vec<String> {
    let mut errors = Vec::new();
    for (path, bytes) in files {
        let Some(name) = path.strip_prefix("templates/") else {
            continue;
        };
        let src = match std::str::from_utf8(bytes) {
            Ok(src) => src,
            Err(e) => {
                errors.push(format!("{path}: not valid UTF-8 ({e})"));
                continue;
            }
        };
        let mut env = minijinja::Environment::new();
        if let Err(e) = env.add_template(name, src) {
            errors.push(format!("{path}: {e}"));
        }
    }
    errors.sort();
    errors
}

/// The gate a draft passes before it goes live, over its full view (baked ∪
/// draft): one message per problem. Compile-only for now; the strict smoke
/// render (#117) plugs in here.
pub fn validate_for_publish(view: &Files) -> Vec<String> {
    validate(view.iter().map(|(p, b)| (p.as_str(), b.as_ref())))
}

impl DesignStore {
    /// Complete an unfinished publish if one is pending (see
    /// [`publish`](super::publish)), then reload the published design from
    /// storage and recompile `templates` on success, recording the outcome
    /// for the admin either way. Startup runs this before serving.
    pub async fn reload(
        &self,
        storage: &Storage,
        templates: &Templates,
    ) -> Result<ReloadStatus, DesignError> {
        let _guard = self.reload_lock.lock().await;
        let (result, completed) = match self.recover_locked(storage).await {
            Ok(completed) => (self.swap_in(storage).await, completed),
            Err(e) => (Err(e), None),
        };
        self.record_reload(templates, result, completed)
    }

    /// A plain reload for a caller already holding `reload_lock`.
    pub(super) async fn reload_locked(
        &self,
        storage: &Storage,
        templates: &Templates,
    ) -> Result<ReloadStatus, DesignError> {
        let result = self.swap_in(storage).await;
        self.record_reload(templates, result, None)
    }

    fn record_reload(
        &self,
        templates: &Templates,
        result: Result<(), DesignError>,
        completed_publish: Option<HistoryEntry>,
    ) -> Result<ReloadStatus, DesignError> {
        let status = ReloadStatus {
            at: Utc::now(),
            ok: result.is_ok(),
            files: self.stored.read().files.len(),
            error: result.as_ref().err().map(status_error),
            completed_publish,
        };
        if result.is_ok() {
            templates.refresh();
        }
        *self.status.write() = Some(status.clone());
        match &result {
            Ok(()) => tracing::info!(files = status.files, "design loaded"),
            Err(e) => tracing::warn!("design reload failed: {e}"),
        }
        result.map(|()| status)
    }

    async fn swap_in(&self, storage: &Storage) -> Result<(), DesignError> {
        let current = self.stored.read().clone();
        let files = load_prefix(storage, DESIGN_PREFIX, &current.files).await?;
        let errors = validate(files.iter().map(|(p, f)| (p.as_str(), f.bytes.as_ref())));
        if !errors.is_empty() {
            return Err(DesignError::Invalid(errors));
        }
        *self.stored.write() = Arc::new(Stored { files });
        Ok(())
    }

    /// The published design files currently live (without `DESIGN_DIR`).
    pub(super) fn published_files(&self) -> Files {
        files_of(&self.stored.read().files)
    }

    /// The `design/` objects in storage now (which a bucket edit may have
    /// moved ahead of the RAM copy), reusing the RAM copy's bytes.
    pub(super) async fn live_files(&self, storage: &Storage) -> Result<Files, DesignError> {
        let current = self.stored.read().clone();
        Ok(files_of(
            &load_prefix(storage, DESIGN_PREFIX, &current.files).await?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_must_be_files_under_a_bundle_root() {
        for ok in [
            "templates/base.html",
            "assets/css/style.css",
            "mdcast/typst/layouts/pdf/hero.typ",
        ] {
            assert!(check_path(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "templates/",
            "preview/x.html",
            "node_modules/a.js",
            "/templates/base.html",
            "templates/../secret",
            "assets//x.css",
            "meta.json",
        ] {
            assert!(
                matches!(check_path(bad), Err(DesignError::BadPath(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn reload_status_does_not_leak_storage_errors() {
        let db = sea_orm::DbErr::Custom("relation \"storage_objects\" does not exist".into());
        let msg = status_error(&DesignError::Storage(storage::Error::Db(db)));
        assert_eq!(msg, "database error");

        let down = object_store::Error::Generic {
            store: "S3",
            source: "http://secret-host:3900 refused".into(),
        };
        let msg = status_error(&DesignError::Storage(storage::Error::Unavailable(down)));
        assert_eq!(msg, "storage unavailable");

        let invalid = DesignError::Invalid(vec!["templates/404.html: syntax error".into()]);
        assert!(status_error(&invalid).contains("templates/404.html"));
    }

    #[test]
    fn publish_failure_message_does_not_leak_storage_errors() {
        let db = sea_orm::DbErr::Custom("relation \"storage_objects\" does not exist".into());
        let err = DesignError::PublishFailed {
            error: Box::new(DesignError::Storage(storage::Error::Db(db))),
            restored: true,
            pending: false,
        };
        let msg = status_error(&err);
        assert!(msg.contains("(previous design restored)"), "{msg}");
        assert!(msg.ends_with("database error"), "{msg}");
        assert!(!msg.contains("storage_objects"), "{msg}");
    }

    #[test]
    fn publish_failure_reports_the_rollback_accurately() {
        let fail = |restored, pending| {
            DesignError::PublishFailed {
                error: Box::new(DesignError::Invalid(vec!["x".into()])),
                restored,
                pending,
            }
            .to_string()
        };
        let marker_left = fail(true, true);
        assert!(marker_left.contains("previous design restored, but the publish marker"));
        assert!(marker_left.contains("next reload, publish or start completes"));
        assert!(fail(false, true).contains("restoring the previous design failed"));
        assert!(!fail(true, false).contains("completes"));
    }

    #[test]
    fn validate_reports_broken_templates_only() {
        let files: [(&str, &[u8]); 4] = [
            ("templates/ok.html", b"{% if x %}yes{% endif %}"),
            ("templates/broken.html", b"{% if x %}never closed"),
            ("templates/binary.html", &[0xff, 0xfe]),
            ("assets/css/broken.css", b"{% if"),
        ];
        let errors = validate(files);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].starts_with("templates/binary.html: not valid UTF-8"));
        assert!(errors[1].starts_with("templates/broken.html:"));
    }
}
