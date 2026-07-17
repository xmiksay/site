//! The effective system prompt cache — split out of `engine.rs` to keep that
//! file under the 400-line cap. `SiteEngine`'s `system_prompt_resolver`
//! (embedding.md §4's snapshot-cache pattern) must stay a sync `Fn`, so the
//! DB `system/prompt` page is read here on a timer rather than per-turn.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use sea_orm::DatabaseConnection;

use super::SiteEngine;
use crate::ai::config::SYSTEM_PROMPT_PAGE_PATH;

/// How often the background task re-reads the `system/prompt` page. The
/// resolver itself must stay a sync `Fn` — this is how often the cache it
/// reads gets refreshed, not how often the DB is hit per-turn.
const SYSTEM_PROMPT_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

impl SiteEngine {
    /// Current effective system prompt (the DB `system/prompt` page's
    /// markdown if non-blank, else `AiConfig`'s fallback) — refreshed every
    /// `SYSTEM_PROMPT_REFRESH_INTERVAL` in the background. Exposed for e.g.
    /// an admin "preview effective prompt" surface in the next phase.
    pub fn system_prompt(&self) -> String {
        self.system_prompt_cache.read().clone()
    }
}

pub(super) async fn load_system_prompt(db: &DatabaseConnection, fallback: &str) -> String {
    match crate::repo::pages::find_by_path(db, SYSTEM_PROMPT_PAGE_PATH).await {
        Ok(Some(page)) if !page.markdown.trim().is_empty() => page.markdown,
        Ok(_) => fallback.to_string(),
        Err(e) => {
            tracing::warn!(error = %e, "failed to load system/prompt page; using fallback");
            fallback.to_string()
        }
    }
}

/// Spawn the background task that keeps `cache` in sync with the DB
/// `system/prompt` page — `spawn`'s counterpart to [`load_system_prompt`]'s
/// one-shot initial read.
pub(super) fn spawn_refresh_task(
    db: DatabaseConnection,
    fallback: String,
    cache: Arc<RwLock<String>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SYSTEM_PROMPT_REFRESH_INTERVAL);
        interval.tick().await; // first tick fires immediately; skip the redundant reload
        loop {
            interval.tick().await;
            let prompt = load_system_prompt(&db, &fallback).await;
            *cache.write() = prompt;
        }
    })
}
