//! `site_cli storage migrate`: copy blobs (the CLI passes every hash
//! `file_blobs` knows about, see [`Storage::known_hashes`]) and every keyed
//! object (see [`Storage::list_all`]) from one backend into another.
//! Idempotent, verified by read-back, never overwrites differing content, and
//! the source is only ever read.

use anyhow::{Context as _, Result, bail};

use super::Storage;
use crate::files::hash_blob;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// By blob hash.
    pub blobs: Tally,
    /// By object key.
    pub objects: Tally,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub copied: Vec<String>,
    /// Already in the target with the right content.
    pub present: Vec<String>,
    /// Missing or corrupt in the source, stored with other content in the
    /// target (never overwritten), or failed copies.
    pub problems: Vec<String>,
}

impl Tally {
    fn record(&mut self, id: &str, outcome: Result<Outcome>) {
        match outcome {
            Ok(Outcome::Copied) => self.copied.push(id.to_string()),
            Ok(Outcome::Present) => self.present.push(id.to_string()),
            Err(e) => self.problems.push(format!("{id}: {e:#}")),
        }
    }

    fn summary(&self) -> String {
        format!(
            "{} copied, {} already present, {} problem(s)",
            self.copied.len(),
            self.present.len(),
            self.problems.len()
        )
    }
}

impl Report {
    pub fn summary(&self) -> String {
        format!(
            "blobs: {}; objects: {}",
            self.blobs.summary(),
            self.objects.summary()
        )
    }

    pub fn problems(&self) -> impl Iterator<Item = &String> {
        self.blobs.problems.iter().chain(&self.objects.problems)
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
        report
            .blobs
            .record(hash, copy_blob(source, target, hash).await);
    }
    let objects = source
        .list_all()
        .await
        .context("list the source's keyed objects")?;
    for object in objects {
        let outcome = copy_object(source, target, &object.key).await;
        report.objects.record(&object.key, outcome);
    }
    Ok(report)
}

async fn copy_blob(source: &Storage, target: &Storage, hash: &str) -> Result<Outcome> {
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

async fn copy_object(source: &Storage, target: &Storage, key: &str) -> Result<Outcome> {
    // Listed a moment ago; gone now means a concurrent delete.
    let Some(data) = source.get(key).await? else {
        bail!("missing in the source");
    };
    if let Some(existing) = target.get(key).await? {
        if existing != data {
            bail!("already stored in the target with different content, kept");
        }
        return Ok(Outcome::Present);
    }
    target.put(key, data.clone()).await?;
    if target.get(key).await?.as_ref() != Some(&data) {
        bail!("read-back after the copy does not match");
    }
    Ok(Outcome::Copied)
}
