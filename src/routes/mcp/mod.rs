//! Hand-rolled JSON-RPC 2.0 MCP server at `POST /mcp`. This is the site
//! *serving* MCP to external clients (a service token or OAuth access token
//! authenticates the caller); it's distinct from `crate::ai::mcp`, which is
//! the site *consuming* per-user MCP servers on behalf of the AI assistant.
//!
//! Split by tool family: JSON-RPC envelope/plumbing lives in `rpc`, the
//! static `initialize`/`tools/list` content lives in `instructions`, and each
//! tool family (`pages`, `tags`, `files`, `galleries`) is a plain module of
//! `async fn`s dispatched from `handle_tools_call` below — callable directly
//! without going through the Axum router.

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::repo::pages::{self as pages_repo};
use crate::routes::oauth;
use crate::state::AppState;

mod files;
mod galleries;
mod instructions;
mod pages;
mod rpc;
mod tags;

use rpc::{JsonRpcRequest, JsonRpcResponse};

pub fn router() -> Router<AppState> {
    Router::new().route("/mcp", post(handle))
}

const SERVER_NAME: &str = "site";
const SERVER_VERSION: &str = "1.0.0";
const PROTOCOL_VERSION: &str = "2025-03-26";

async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    let user_id = match oauth::authenticate_mcp(&state, &headers).await {
        Ok(uid) => uid,
        Err((status, www_auth)) => {
            let body = Json(JsonRpcResponse::error(None, -32000, "Unauthorized"));
            let mut response: Response = (status, body).into_response();
            if let Ok(val) = HeaderValue::from_str(&www_auth) {
                response.headers_mut().insert("WWW-Authenticate", val);
            }
            return response;
        }
    };

    let resp = match req.method.as_str() {
        "initialize" => handle_initialize(&state, req.id).await,
        "notifications/initialized" => {
            return (
                StatusCode::OK,
                Json(JsonRpcResponse::success(req.id, json!({}))),
            )
                .into_response();
        }
        "tools/list" => instructions::handle_tools_list(req.id),
        "tools/call" => handle_tools_call(&state, user_id, req.id.clone(), req.params).await,
        _ => JsonRpcResponse::error(req.id, -32601, format!("Method not found: {}", req.method)),
    };

    (StatusCode::OK, Json(resp)).into_response()
}

async fn handle_initialize(state: &AppState, id: Option<Value>) -> JsonRpcResponse {
    let instructions = match pages_repo::find_by_path(&state.db, "CLAUDE").await {
        Ok(Some(p)) => p.markdown,
        _ => instructions::server_instructions(),
    };

    JsonRpcResponse::success(
        id,
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION
            },
            "instructions": instructions
        }),
    )
}

async fn handle_tools_call(
    state: &AppState,
    user_id: i32,
    id: Option<Value>,
    params: Option<Value>,
) -> JsonRpcResponse {
    let params = match params {
        Some(p) => p,
        None => return JsonRpcResponse::error(id, -32602, "Missing params"),
    };

    let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    match tool_name {
        // pages
        "read_page" => pages::tool_read_page(state, id, arguments).await,
        "edit_page" => pages::tool_edit_page(state, user_id, id, arguments).await,
        "search_pages" => pages::tool_search_pages(state, id, arguments).await,
        "delete_page" => pages::tool_delete_page(state, id, arguments).await,

        // tags
        "list_tags" => tags::tool_list_tags(state, id).await,
        "read_tag" => tags::tool_read_tag(state, id, arguments).await,
        "create_tag" => tags::tool_create_tag(state, id, arguments).await,
        "update_tag" => tags::tool_update_tag(state, id, arguments).await,
        "delete_tag" => tags::tool_delete_tag(state, id, arguments).await,

        // files
        "list_files" => files::tool_list_files(state, id, arguments).await,
        "create_file" => files::tool_create_file(state, user_id, id, arguments).await,
        "read_file" => files::tool_read_file(state, id, arguments).await,
        "update_file" => files::tool_update_file(state, id, arguments).await,
        "delete_file" => files::tool_delete_file(state, id, arguments).await,

        // galleries
        "list_galleries" => galleries::tool_list_galleries(state, id).await,
        "read_gallery" => galleries::tool_read_gallery(state, id, arguments).await,
        "create_gallery" => galleries::tool_create_gallery(state, user_id, id, arguments).await,
        "update_gallery" => galleries::tool_update_gallery(state, id, arguments).await,
        "delete_gallery" => galleries::tool_delete_gallery(state, id, arguments).await,

        _ => JsonRpcResponse::error(id, -32602, format!("Unknown tool: {tool_name}")),
    }
}
