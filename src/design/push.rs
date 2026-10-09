//! `site_cli design push <dir>`: upload a design folder (the old
//! `DESIGN_DIR` layout) as `design/…` overrides. Idempotent; templates are
//! syntax-checked before anything is written. The running server picks the
//! files up on its next reload (admin Reload button or restart).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use bytes::Bytes;

use super::stored::{DESIGN_PREFIX, check_path, validate};
use crate::storage::Storage;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub uploaded: Vec<String>,
    pub unchanged: Vec<String>,
    /// Files outside the bundle roots, never uploaded.
    pub skipped: Vec<String>,
}

pub async fn push(storage: &Storage, dir: &Path) -> Result<Report> {
    if !storage.has_objects() {
        bail!("STORAGE_KIND=db holds no design overrides; set it to fs or s3");
    }
    let mut report = Report::default();
    let mut files = Vec::new();
    for (rel, path) in walk(dir)? {
        if check_path(&rel).is_err() {
            report.skipped.push(rel);
            continue;
        }
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        files.push((rel, Bytes::from(bytes)));
    }

    let errors = validate(files.iter().map(|(p, b)| (p.as_str(), b.as_ref())));
    if !errors.is_empty() {
        bail!(
            "nothing uploaded, broken templates:\n  {}",
            errors.join("\n  ")
        );
    }

    for (rel, bytes) in files {
        let key = format!("{DESIGN_PREFIX}/{rel}");
        if storage.get(&key).await?.as_ref() == Some(&bytes) {
            report.unchanged.push(rel);
            continue;
        }
        storage.put(&key, bytes).await?;
        report.uploaded.push(rel);
    }
    Ok(report)
}

/// Every non-hidden file under `dir` as `(forward-slash relative path, path)`.
fn walk(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in
            std::fs::read_dir(&current).with_context(|| format!("read {}", current.display()))?
        {
            let path = entry?.path();
            let hidden = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(dir) {
                out.push((rel.to_string_lossy().replace('\\', "/"), path.clone()));
            }
        }
    }
    out.sort();
    Ok(out)
}
