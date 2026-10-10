use sea_orm::DatabaseConnection;
use std::sync::Arc;
use std::time::Duration;

use crate::ai::AiConfig;
use crate::ai::engine::SiteEngine;
use crate::config::Config;
use crate::design::DesignStore;
use crate::migration::{Migrator, MigratorTrait};
use crate::routes::ws::WsHub;
use crate::storage::Storage;
use crate::templates::Templates;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    /// File and thumbnail bytes (`STORAGE_KIND`).
    pub storage: Storage,
    pub tmpl: Templates,
    pub design: Arc<DesignStore>,
    pub agent_engine: Arc<SiteEngine>,
    pub ws_hub: Arc<WsHub>,
    /// Client for the remote `mdcast-server` that renders every export
    /// target. `None` when `MDCAST_URL` is unset (or the URL is invalid) —
    /// both export routes answer 503 with a clear message; nothing else in
    /// the site degrades. The startup capabilities probe is log-only: a
    /// server that is down at boot starts working the moment it comes up.
    pub mdcast: Option<mdcast_client::Client>,
}

/// How long a shared endpoint-state file must have sat untouched before the
/// startup sweep may remove it — long enough that a pair belonging to any
/// endpoint still in rotation is never a candidate. Matches what
/// entanglement's own binary sweeps with.
const ENDPOINT_STATE_MAX_IDLE: Duration = Duration::from_secs(3600);

/// Best-effort sweep of leaked cross-process endpoint state (#97). Every LLM
/// request and every MCP connect writes a `.state`/`.lock` pair under
/// `${data_dir}/entanglement/endpoints/` keyed by the endpoint's URL, and
/// nothing in normal operation removes one — so an edited MCP server URL, a
/// rotated key or a changed provider `base_url` orphans its pair forever (248
/// → 916 files over one debugging session here). `prune_stale` only deletes
/// pairs that are *both* idle past `ENDPOINT_STATE_MAX_IDLE` and carry no live
/// lease, cool-down or recent request, so it can never sweep state another
/// process is relying on. Blocking file I/O (each candidate is opened under
/// its advisory lock), hence the blocking pool; and never fatal — a failed
/// sweep just means the litter stays another boot.
async fn prune_endpoint_state() {
    match tokio::task::spawn_blocking(|| {
        entanglement_provider::prune_stale(ENDPOINT_STATE_MAX_IDLE)
    })
    .await
    {
        Ok(0) => tracing::debug!("startup sweep found no orphaned endpoint-state files"),
        Ok(removed) => {
            tracing::info!("startup sweep removed {removed} orphaned endpoint-state file(s)")
        }
        Err(e) => tracing::warn!("startup endpoint-state sweep failed: {e}"),
    }
}

pub async fn create_state(config: &Config) -> AppState {
    let db = sea_orm::Database::connect(&config.database_url)
        .await
        .expect("Failed to connect to database");

    // Migrate before anything reads the schema: `SiteEngine::spawn` below
    // hydrates the model catalog from `llm_providers`, so running migrations
    // after state creation (as site_server once did) crashloops on any
    // migration that state hydration depends on.
    Migrator::up(&db, None).await.expect("Migrations failed");

    let storage = Storage::new(&config.storage, db.clone()).expect("Failed to configure storage");
    tracing::info!(kind = storage.kind(), "blob storage");

    let design = Arc::new(DesignStore::new(config.design_dir.clone()));
    let tmpl = Templates::new(design.clone());
    // Refuse the start rather than serve the baked design in place of the
    // site's own while the bucket is down or an override is broken.
    design
        .recover_publish(&storage)
        .await
        .expect("Failed to complete an interrupted design publish");
    design
        .reload(&storage, &tmpl)
        .await
        .expect("Failed to load design overrides from storage");

    // Before the engine (and its per-provider HTTP clients) starts writing
    // fresh endpoint state of its own.
    prune_endpoint_state().await;

    let ai_config = Arc::new(AiConfig::new());
    let ws_hub = Arc::new(WsHub::new());
    let agent_engine = SiteEngine::spawn(
        db.clone(),
        storage.clone(),
        ai_config,
        ws_hub.clone(),
        config.serper_api_key.clone(),
        None,
    )
    .await
    .expect("Failed to spawn assistant engine");
    crate::ai::ws_bridge::spawn(agent_engine.clone(), ws_hub.clone(), db.clone());

    let mdcast = match &config.mdcast_url {
        None => {
            tracing::warn!(
                "MDCAST_URL not set; PDF/slides export is disabled (export routes answer 503)"
            );
            None
        }
        Some(url) => match crate::export::build_client(url, config.mdcast_token.as_deref()) {
            Err(err) => {
                tracing::warn!("{err:#} — export is disabled (export routes answer 503)");
                None
            }
            Ok(client) => {
                match client.capabilities().await {
                    Ok(caps) => tracing::info!(
                        version = %caps.version,
                        targets = ?caps.targets,
                        max_upload_bytes = caps.max_upload_bytes,
                        "mdcast render server reachable"
                    ),
                    Err(err) => tracing::warn!(
                        "mdcast render server at `{url}` unreachable at startup ({err}); exports will still be attempted per-request"
                    ),
                }
                Some(client)
            }
        },
    };

    AppState {
        db,
        storage,
        tmpl,
        design,
        agent_engine,
        ws_hub,
        mdcast,
    }
}
