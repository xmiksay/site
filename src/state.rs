use sea_orm::DatabaseConnection;
use std::sync::Arc;

use crate::ai::AiConfig;
use crate::ai::engine::SiteEngine;
use crate::config::Config;
use crate::design::DesignStore;
use crate::migration::{Migrator, MigratorTrait};
use crate::routes::ws::WsHub;
use crate::templates::Templates;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub tmpl: Templates,
    pub design: Arc<DesignStore>,
    pub agent_engine: Arc<SiteEngine>,
    pub ws_hub: Arc<WsHub>,
    /// Result of the startup `probe_pandoc` capability check (#64). `false`
    /// means the pandoc-backed export targets (DOCX/ODT/PPTX/reveal.js
    /// slides) must be refused with a clear error rather than attempted —
    /// PDF export is unaffected, since typst runs in-process.
    pub pandoc_available: bool,
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

    let design = Arc::new(DesignStore::new(config.design_dir.clone()));
    let tmpl = Templates::new(design.clone());

    let ai_config = Arc::new(AiConfig::new());
    let ws_hub = Arc::new(WsHub::new());
    let agent_engine = SiteEngine::spawn(
        db.clone(),
        ai_config,
        ws_hub.clone(),
        config.serper_api_key.clone(),
        None,
    )
    .await
    .expect("Failed to spawn assistant engine");
    crate::ai::ws_bridge::spawn(agent_engine.clone(), ws_hub.clone(), db.clone());

    let pandoc_available = match crate::export::probe_pandoc(&config.mdcast_pandoc_path).await {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(
                "{err} — DOCX/ODT/PPTX/reveal.js-slides export will be unavailable until it is installed (set MDCAST_PANDOC_PATH to override the binary path)"
            );
            false
        }
    };

    AppState {
        db,
        tmpl,
        design,
        agent_engine,
        ws_hub,
        pandoc_available,
    }
}
