use minijinja::Environment;
use minijinja::value::Value;
use sea_orm::DatabaseConnection;
use std::sync::Arc;

use crate::ai::{
    AiConfig, llm::registry::ProviderRegistry, local_tools, mcp_client::UserMcpManager,
    tool_registry::ToolRegistry,
};
use crate::assets::AssetStore;
use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub tmpl: Arc<Environment<'static>>,
    pub assets: Arc<AssetStore>,
    pub ai_config: Arc<AiConfig>,
    pub provider_registry: Arc<ProviderRegistry>,
    pub tool_registry: Arc<ToolRegistry>,
    pub mcp_manager: Arc<UserMcpManager>,
}

fn timeformat(value: Value, format: Option<String>) -> Result<String, minijinja::Error> {
    let s = value.to_string();
    let fmt = format.as_deref().unwrap_or("%d. %m. %Y %H:%M");
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f") {
        return Ok(dt.format(fmt).to_string());
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Ok(dt.format(fmt).to_string());
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
        return Ok(d.format(fmt).to_string());
    }
    Ok(s)
}

pub async fn create_state(config: &Config) -> AppState {
    let db = sea_orm::Database::connect(&config.database_url)
        .await
        .expect("Failed to connect to database");

    let assets = Arc::new(AssetStore::new(
        config.namespace.clone(),
        config.assets_dir.clone(),
    ));

    let mut env = Environment::new();
    let loader_assets = assets.clone();
    env.set_loader(move |name| {
        match loader_assets.load(&format!("templates/{name}")) {
            Some(data) => match String::from_utf8(data) {
                Ok(src) => Ok(Some(src)),
                Err(e) => Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("template '{name}' is not valid UTF-8: {e}"),
                )),
            },
            None => Ok(None),
        }
    });
    env.add_filter("timeformat", timeformat);

    let ai_config = Arc::new(AiConfig::new());
    let provider_registry = Arc::new(ProviderRegistry::new(db.clone()));
    let tool_registry = Arc::new(ToolRegistry::new(local_tools::default_tools(
        config.serper_api_key.clone(),
    )));
    let mcp_manager = Arc::new(UserMcpManager::new(db.clone()));

    AppState {
        db,
        tmpl: Arc::new(env),
        assets,
        ai_config,
        provider_registry,
        tool_registry,
        mcp_manager,
    }
}
