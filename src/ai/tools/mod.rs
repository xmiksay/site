//! `entanglement_runtime::tools::Tool` implementations — the engine's
//! built-in (non-MCP) tool vocabulary. Each tool recovers the calling user
//! from the session id (`crate::ai::engine::user_id_from_session`);
//! session-scoping replaces an explicit `ctx` struct. `Tool::run` is never
//! invoked by the executor once `run_for_session` is overridden (only
//! `ToolRegistry::execute` calls `run_for_session`, which defaults to calling
//! `run_content`/`run` — see `entanglement_runtime::tools::Tool` docs), so
//! every tool below just bails out of `run` with an explanatory message
//! rather than silently no-op'ing.
//!
//! `ToolCall.input` arrives as a JSON **string** — see `common::parse_args`.

mod common;
mod design;
mod file_read;
mod files;
mod galleries;
mod image;
mod pages;
mod tags;
mod web;

use std::sync::Arc;

use entanglement_runtime::ToolRegistry;
use sea_orm::DatabaseConnection;

use crate::design::DesignStore;
use crate::routes::ws::WsHub;
use crate::storage::Storage;

/// Build the registry of built-in (non-MCP) tools: the 14 site tools — a
/// curated subset, not full CRUD (page read/search/edit/delete, tag
/// list/create, file list/create/read/update/delete, gallery
/// list/create/update) — plus `web_search`/`web_fetch` and the seven
/// design-draft tools (`design_*`, shared with MCP, #118). `engine.rs`
/// builds this once at `SiteEngine` construction and layers per-session MCP
/// routing tools on top (see `crate::ai::mcp`). Every mutating tool also
/// gets `ws_hub` so it broadcasts the same WS event a REST API mutation
/// would (issue #25, `crate::routes::broadcast`).
pub fn registry(
    db: Arc<DatabaseConnection>,
    storage: Storage,
    design: Arc<DesignStore>,
    ws_hub: Arc<WsHub>,
    serper_api_key: Option<String>,
) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    design::register(
        &mut reg,
        design::DesignDeps {
            db: db.clone(),
            storage: storage.clone(),
            design,
            ws_hub: ws_hub.clone(),
        },
    );
    reg.register(pages::ReadPageTool { db: db.clone() });
    reg.register(pages::SearchPagesTool { db: db.clone() });
    reg.register(pages::EditPageTool {
        db: db.clone(),
        ws_hub: ws_hub.clone(),
    });
    reg.register(pages::DeletePageTool {
        db: db.clone(),
        ws_hub: ws_hub.clone(),
    });
    reg.register(tags::ListTagsTool { db: db.clone() });
    reg.register(tags::CreateTagTool {
        db: db.clone(),
        ws_hub: ws_hub.clone(),
    });
    reg.register(files::ListFilesTool { db: db.clone() });
    reg.register(files::CreateFileTool {
        db: db.clone(),
        storage: storage.clone(),
        ws_hub: ws_hub.clone(),
    });
    reg.register(file_read::ReadFileTool {
        db: db.clone(),
        storage: storage.clone(),
    });
    reg.register(files::UpdateFileTool {
        db: db.clone(),
        storage,
        ws_hub: ws_hub.clone(),
    });
    reg.register(files::DeleteFileTool {
        db: db.clone(),
        ws_hub: ws_hub.clone(),
    });
    reg.register(galleries::ListGalleriesTool { db: db.clone() });
    reg.register(galleries::CreateGalleryTool {
        db: db.clone(),
        ws_hub: ws_hub.clone(),
    });
    reg.register(galleries::UpdateGalleryTool { db, ws_hub });
    reg.register(web::WebSearchTool::new(serper_api_key));
    reg.register(web::WebFetchTool);
    reg
}
