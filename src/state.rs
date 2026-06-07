use sea_orm::DatabaseConnection;
use std::sync::Arc;

use crate::ai::{
    AiConfig, llm::registry::ProviderRegistry, local_tools, mcp_client::UserMcpManager,
    tool_registry::ToolRegistry,
};
use crate::assets::AssetStore;
use crate::config::Config;
use crate::templates::Templates;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub tmpl: Templates,
    pub assets: Arc<AssetStore>,
    pub ai_config: Arc<AiConfig>,
    pub provider_registry: Arc<ProviderRegistry>,
    pub tool_registry: Arc<ToolRegistry>,
    pub mcp_manager: Arc<UserMcpManager>,
}

pub async fn create_state(config: &Config) -> AppState {
    let db = sea_orm::Database::connect(&config.database_url)
        .await
        .expect("Failed to connect to database");

    let assets = Arc::new(AssetStore::new(
        config.namespace.clone(),
        config.assets_dir.clone(),
    ));
    let tmpl = Templates::new(assets.clone());

    let ai_config = Arc::new(AiConfig::new());
    let provider_registry = Arc::new(ProviderRegistry::new(db.clone()));
    let tool_registry = Arc::new(ToolRegistry::new(local_tools::default_tools(
        config.serper_api_key.clone(),
    )));
    let mcp_manager = Arc::new(UserMcpManager::new(db.clone()));

    AppState {
        db,
        tmpl,
        assets,
        ai_config,
        provider_registry,
        tool_registry,
        mcp_manager,
    }
}
