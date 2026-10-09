//! Design overrides kept in storage as `design/{path}` objects, `path`
//! mirroring the baked bundle (`templates/…`, `assets/…`, `mdcast/…`). They
//! are edited straight in the bucket (then reloaded) or through the admin
//! Design page, and held in RAM between reloads.
//!
//! A reload lists `design/`, downloads only objects whose version changed,
//! applies the pending admin change (if any), syntax-checks every override
//! template, and only then writes the change and swaps the overlay in — a
//! broken template leaves both storage and the running design untouched.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::DesignStore;
use crate::storage::{self, Storage, Version};
use crate::templates::Templates;

/// Storage key prefix of every override.
pub const DESIGN_PREFIX: &str = "design";

/// The bundle roots `DesignStore` serves; an override elsewhere is ignored.
pub const ROOTS: [&str; 3] = ["templates/", "assets/", "mdcast/"];

#[derive(Default)]
pub(super) struct Stored {
    pub(super) files: HashMap<String, StoredFile>,
}

pub(super) struct StoredFile {
    pub(super) bytes: Bytes,
    /// `None` right after an admin write: the next reload downloads it once
    /// to learn its version.
    version: Option<Version>,
}

/// Outcome of the last reload, shown in the admin.
#[derive(Debug, Clone, Serialize)]
pub struct ReloadStatus {
    pub at: DateTime<Utc>,
    pub ok: bool,
    pub files: usize,
    pub error: Option<String>,
}

pub enum Change {
    Put { path: String, bytes: Bytes },
    Delete { path: String },
}

#[derive(Debug, thiserror::Error)]
pub enum DesignError {
    #[error(transparent)]
    Storage(#[from] storage::Error),
    #[error("invalid design path {0:?}: must be a file under templates/, assets/ or mdcast/")]
    BadPath(String),
    #[error("{0} has no override")]
    NoOverride(String),
    #[error("{}", .0.join("; "))]
    Invalid(Vec<String>),
}

/// `path` names a file under one of the [`ROOTS`] and makes a valid key.
pub fn check_path(path: &str) -> Result<(), DesignError> {
    let under_root = ROOTS
        .iter()
        .any(|root| path.len() > root.len() && path.starts_with(root));
    if !under_root || storage::parse_key(&key(path)).is_err() {
        return Err(DesignError::BadPath(path.to_string()));
    }
    Ok(())
}

fn key(path: &str) -> String {
    format!("{DESIGN_PREFIX}/{path}")
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

impl DesignStore {
    /// Reload the overrides from storage, applying `change` first if given,
    /// and recompile `templates` on success. A plain reload (no change)
    /// records its outcome for the admin either way; a rejected change leaves
    /// the last status alone since nothing changed.
    pub async fn apply(
        &self,
        storage: &Storage,
        templates: &Templates,
        change: Option<Change>,
    ) -> Result<ReloadStatus, DesignError> {
        let _guard = self.reload_lock.lock().await;
        let is_reload = change.is_none();
        let result = self.apply_locked(storage, change).await;
        let status = ReloadStatus {
            at: Utc::now(),
            ok: result.is_ok(),
            files: self.stored.read().files.len(),
            error: result.as_ref().err().map(ToString::to_string),
        };
        if result.is_ok() {
            templates.refresh();
        }
        if result.is_ok() || is_reload {
            *self.status.write() = Some(status.clone());
        }
        match &result {
            Ok(()) => tracing::info!(files = status.files, "design overrides loaded"),
            Err(e) => tracing::warn!("design reload failed: {e}"),
        }
        result.map(|()| status)
    }

    async fn apply_locked(
        &self,
        storage: &Storage,
        change: Option<Change>,
    ) -> Result<(), DesignError> {
        if !storage.has_objects() {
            if change.is_some() {
                return Err(storage::Error::NoObjectStore.into());
            }
            *self.stored.write() = Arc::default();
            return Ok(());
        }

        let current = self.stored.read().clone();
        let mut files = HashMap::new();
        for object in storage.list(DESIGN_PREFIX).await? {
            let Some(path) = object.key.strip_prefix("design/") else {
                continue;
            };
            if check_path(path).is_err() {
                tracing::warn!(key = %object.key, "ignoring design object outside the bundle roots");
                continue;
            }
            let cached = current
                .files
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

        match &change {
            Some(Change::Put { path, bytes }) => {
                check_path(path)?;
                let file = StoredFile {
                    bytes: bytes.clone(),
                    version: None,
                };
                files.insert(path.clone(), file);
            }
            Some(Change::Delete { path }) => {
                check_path(path)?;
                if files.remove(path).is_none() {
                    return Err(DesignError::NoOverride(path.clone()));
                }
            }
            None => {}
        }

        let errors = validate(files.iter().map(|(p, f)| (p.as_str(), f.bytes.as_ref())));
        if !errors.is_empty() {
            return Err(DesignError::Invalid(errors));
        }

        match change {
            Some(Change::Put { path, bytes }) => storage.put(&key(&path), bytes).await?,
            Some(Change::Delete { path }) => storage.delete(&key(&path)).await?,
            None => {}
        }
        *self.stored.write() = Arc::new(Stored { files });
        Ok(())
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
        ] {
            assert!(
                matches!(check_path(bad), Err(DesignError::BadPath(_))),
                "{bad:?}"
            );
        }
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
