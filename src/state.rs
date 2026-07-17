use sea_orm::DatabaseConnection;
use std::sync::Arc;

use crate::ai::AiConfig;
use crate::ai::engine::SiteEngine;
use crate::config::Config;
use crate::design::DesignStore;
use crate::routes::ws::WsHub;
use crate::templates::Templates;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub tmpl: Templates,
    pub design: Arc<DesignStore>,
    pub agent_engine: Arc<SiteEngine>,
    pub ws_hub: Arc<WsHub>,
}

pub async fn create_state(config: &Config) -> AppState {
    let db = sea_orm::Database::connect(&config.database_url)
        .await
        .expect("Failed to connect to database");

    let design = Arc::new(DesignStore::new(config.design_dir.clone()));
    let tmpl = Templates::new(design.clone());

    let ai_config = Arc::new(AiConfig::new());
    let agent_engine =
        SiteEngine::spawn(db.clone(), ai_config, config.serper_api_key.clone(), None)
            .await
            .expect("Failed to spawn assistant engine");
    let ws_hub = Arc::new(WsHub::new());
    crate::ai::ws_bridge::spawn(agent_engine.clone(), ws_hub.clone(), db.clone());

    AppState {
        db,
        tmpl,
        design,
        agent_engine,
        ws_hub,
    }
}
