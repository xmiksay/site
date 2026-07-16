//! Port of `src/ai/local_tools` to `entanglement_runtime::tools::Tool` — the
//! new engine's built-in (non-MCP) tool vocabulary. Each tool recovers the
//! calling user from the session id (`crate::ai::engine::user_id_from_session`)
//! instead of taking an explicit `ctx` struct the way the old `LocalTool`
//! trait did; session-scoping replaces that. `Tool::run` is never invoked by
//! the executor once `run_for_session` is overridden (only
//! `ToolRegistry::execute` calls `run_for_session`, which defaults to calling
//! `run_content`/`run` — see `entanglement_runtime::tools::Tool` docs), so
//! every tool below just bails out of `run` with an explanatory message
//! rather than silently no-op'ing.
//!
//! `ToolCall.input` arrives as a JSON **string** now (not a pre-parsed
//! `serde_json::Value` the old `LocalTool::call` took) — see
//! `common::parse_args`.

mod common;
mod files;
mod galleries;
mod pages;
mod tags;
mod web;

use std::sync::Arc;

use entanglement_runtime::ToolRegistry;
use sea_orm::DatabaseConnection;

/// Build the registry of built-in (non-MCP) tools: the 11 site CRUD tools
/// (pages/tags/files/galleries) plus `web_search`/`web_fetch`. `engine.rs`
/// builds this once at `SiteEngine` construction and layers per-session MCP
/// routing tools on top (see `crate::ai::mcp`).
pub fn registry(db: Arc<DatabaseConnection>, serper_api_key: Option<String>) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register(pages::ReadPageTool { db: db.clone() });
    reg.register(pages::SearchPagesTool { db: db.clone() });
    reg.register(pages::EditPageTool { db: db.clone() });
    reg.register(pages::DeletePageTool { db: db.clone() });
    reg.register(tags::ListTagsTool { db: db.clone() });
    reg.register(tags::CreateTagTool { db: db.clone() });
    reg.register(files::ListFilesTool { db: db.clone() });
    reg.register(files::CreateFileTool { db: db.clone() });
    reg.register(galleries::ListGalleriesTool { db: db.clone() });
    reg.register(galleries::CreateGalleryTool { db: db.clone() });
    reg.register(galleries::UpdateGalleryTool { db });
    reg.register(web::WebSearchTool::new(serper_api_key));
    reg.register(web::WebFetchTool);
    reg
}
