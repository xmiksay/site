//! Local (in-process) tool implementations the assistant can call directly,
//! without going through MCP. These mirror the most useful tools in
//! `routes::mcp` so the built-in chat is usable without the user registering
//! the site's own MCP endpoint.

pub mod site_tools;
pub mod web_fetch;
pub mod web_search;

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use serde_json::Value;

use crate::ai::mcp_client::ToolDispatchError;

pub struct LocalToolCtx {
    pub db: DatabaseConnection,
    pub user_id: i32,
    pub session_id: i32,
}

#[async_trait]
pub trait LocalTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    async fn call(&self, ctx: &LocalToolCtx, args: Value) -> Result<Value, ToolDispatchError>;
}

pub fn default_tools(serper_api_key: Option<String>) -> Vec<Arc<dyn LocalTool>> {
    let mut tools = site_tools::tools();
    tools.push(Arc::new(web_search::WebSearch::new(serper_api_key)));
    tools.push(Arc::new(web_fetch::WebFetch));
    tools
}
