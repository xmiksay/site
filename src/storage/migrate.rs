//! `site_cli storage migrate`: copy blobs (the CLI passes every hash
//! `file_blobs` knows about, see [`Storage::known_hashes`]) from one backend
//! into another. Idempotent, verified by sha256, and the source is only ever
//! read.

use anyhow::{Result, bail};

use super::Storage;
use crate::files::hash_blob;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub copied: Vec<String>,
    /// Already in the target with the right content.
    pub present: Vec<String>,
    /// Missing or corrupt in the source, stored with other content in the
    /// target (never overwritten), or failed copies.
    pub problems: Vec<String>,
}

impl Report {
    pub fn summary(&self) -> String {
        format!(
            "{} copied, {} already present, {} problem(s)",
            self.copied.len(),
            self.present.len(),
            self.problems.len()
        )
    }
}

enum Outcome {
    Copied,
    Present,
}

pub async fn migrate(source: &Storage, target: &Storage, hashes: &[String]) -> Result<Report> {
    if source.objects.is_none() && target.objects.is_none() {
        bail!("source and target are both the database; set STORAGE_KIND to fs or s3");
    }
    let mut report = Report::default();
    for hash in hashes {
        match copy_one(source, target, hash).await {
            Ok(Outcome::Copied) => report.copied.push(hash.clone()),
            Ok(Outcome::Present) => report.present.push(hash.clone()),
            Err(e) => report.problems.push(format!("{hash}: {e:#}")),
        }
    }
    Ok(report)
}

async fn copy_one(source: &Storage, target: &Storage, hash: &str) -> Result<Outcome> {
    if let Some(existing) = target.get_blob(hash).await? {
        if hash_blob(&existing) != hash {
            bail!("already stored in the target with different content, kept");
        }
        return Ok(Outcome::Present);
    }
    let Some(data) = source.get_blob(hash).await? else {
        bail!("missing in the source");
    };
    if hash_blob(&data) != hash {
        bail!("source content does not match its hash");
    }
    target.put_blob(&data).await?;
    let stored = target.get_blob(hash).await?;
    if stored.as_deref().map(hash_blob).as_deref() != Some(hash) {
        bail!("read-back after the copy does not match");
    }
    Ok(Outcome::Copied)
}
