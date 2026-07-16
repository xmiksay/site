use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use base64::Engine;

use crate::repo::{
    files::{self as files_repo, FileMetaUpdate, NewFile},
    galleries::{self as galleries_repo, GalleryInput as RepoGalleryInput},
    pages::{self as pages_repo, PageUpdate, UpsertOutcome},
    tags::{self as tags_repo, ResolveError, TagInput as RepoTagInput, TagUpdate as RepoTagUpdate},
};
use crate::routes::oauth;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/mcp", post(handle))
}

const SERVER_NAME: &str = "site";
const SERVER_VERSION: &str = "1.0.0";
const PROTOCOL_VERSION: &str = "2025-03-26";

// --- JSON-RPC types ---

#[derive(Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl JsonRpcResponse {
    fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

// --- Tool input types ---

#[derive(Deserialize)]
struct ReadPageArgs {
    path: String,
}

#[derive(Deserialize)]
struct EditPageArgs {
    path: String,
    markdown: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    tag_names: Option<Vec<String>>,
    #[serde(default)]
    private: Option<bool>,
}

#[derive(Deserialize)]
struct SearchPagesArgs {
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    offset: Option<u64>,
}

#[derive(Deserialize)]
struct DeletePageArgs {
    path: String,
}

#[derive(Deserialize)]
struct TagArgs {
    name: String,
}

#[derive(Deserialize)]
struct TagInputArgs {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct UpdateTagArgs {
    name: String,
    #[serde(default)]
    new_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct ListFilesArgs {
    #[serde(default)]
    mime_prefix: Option<String>,
}

#[derive(Deserialize)]
struct FileIdArgs {
    id: i32,
}

#[derive(Deserialize)]
struct CreateFileArgs {
    path: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    mimetype: Option<String>,
    #[serde(default)]
    data_base64: Option<String>,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Deserialize)]
struct UpdateFileArgs {
    id: i32,
    path: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct GalleryIdArgs {
    id: i32,
}

#[derive(Deserialize)]
struct CreateGalleryArgs {
    path: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    file_ids: Vec<i32>,
}

#[derive(Deserialize)]
struct UpdateGalleryArgs {
    id: i32,
    path: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    file_ids: Vec<i32>,
}

// --- MCP endpoint ---

pub async fn handle(
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
        "tools/list" => handle_tools_list(req.id),
        "tools/call" => handle_tools_call(&state, user_id, req.id.clone(), req.params).await,
        _ => JsonRpcResponse::error(req.id, -32601, format!("Method not found: {}", req.method)),
    };

    (StatusCode::OK, Json(resp)).into_response()
}

async fn handle_initialize(state: &AppState, id: Option<Value>) -> JsonRpcResponse {
    let instructions = match pages_repo::find_by_path(&state.db, "CLAUDE").await {
        Ok(Some(p)) => p.markdown,
        _ => server_instructions(),
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

fn server_instructions() -> String {
    format!(
        "{SERVER_INSTRUCTIONS_HEADER}\n{}\n",
        crate::markdown::MARKDOWN_EXTENSIONS_DOC
    )
}

const SERVER_INSTRUCTIONS_HEADER: &str = "\
# Site — MCP Integration

Server-rendered site. Pages are stored in PostgreSQL and served at their `path` \
(e.g. path `notes/example` → URL `/notes/example`).

The full site admin API is exposed as MCP tools — pages, tags, files, \
galleries, menu items and service tokens can all be managed here. To override \
these instructions for your installation, create a page with path `CLAUDE` and \
its markdown will be served instead.

## Pages

- **path**: unique URL slug. Hierarchical paths use `/` (e.g. `section/sub/page`).
- **markdown**: content in Markdown with custom extensions (see below).
- **summary**: short description for listings.
- **tags**: assigned by name via `edit_page`. Tags must already exist.
- **private**: private pages are only visible to logged-in users. \
  New pages created via MCP default to private.
- **revisions**: every markdown change stores a diff automatically.

## Markdown extensions

";

fn handle_tools_list(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        json!({
            "tools": [
                // ----- Pages -----
                {
                    "name": "read_page",
                    "description": "Read a page by its path. Returns title (path), summary, tags, and full markdown content.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "path": { "type": "string", "description": "The page path (e.g. 'section/sub/page')" } },
                        "required": ["path"]
                    }
                },
                {
                    "name": "edit_page",
                    "description": "Create or update a page by its path. Creates the page if it doesn't exist. A revision diff is stored automatically when markdown changes.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "The page path to edit" },
                            "markdown": { "type": "string", "description": "New markdown content (optional)" },
                            "summary": { "type": "string", "description": "New summary (optional)" },
                            "tag_names": { "type": "array", "items": { "type": "string" }, "description": "Tag names to assign (optional, replaces existing tags)" },
                            "private": { "type": "boolean", "description": "Visibility flag (optional, defaults to true on create)" }
                        },
                        "required": ["path"]
                    }
                },
                {
                    "name": "search_pages",
                    "description": "Search pages by path prefix, tag name, and/or fulltext query (q). Path and tag matches rank above markdown content matches. Returns path, summary for each match, plus total count and has_more flag for pagination.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "prefix": { "type": "string", "description": "Path prefix to filter by (case-insensitive). If omitted, returns all pages." },
                            "tag": { "type": "string", "description": "Optional tag name — only returns pages with this tag" },
                            "q": { "type": "string", "description": "Optional fulltext query (accent-insensitive); ranks path and tag matches above markdown content" },
                            "limit": { "type": "integer", "description": "Max results to return (default 20, max 100)" },
                            "offset": { "type": "integer", "description": "Number of results to skip for pagination (default 0)" }
                        }
                    }
                },
                {
                    "name": "delete_page",
                    "description": "Delete a page by its path.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"]
                    }
                },

                // ----- Tags -----
                {
                    "name": "list_tags",
                    "description": "List all available tags. Returns tag name and description.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "read_tag",
                    "description": "Read a single tag by name.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "name": { "type": "string" } },
                        "required": ["name"]
                    }
                },
                {
                    "name": "create_tag",
                    "description": "Create a new tag.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "description": { "type": "string" }
                        },
                        "required": ["name"]
                    }
                },
                {
                    "name": "update_tag",
                    "description": "Update an existing tag's name and/or description (look up by current name).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Current name (lookup key)" },
                            "new_name": { "type": "string" },
                            "description": { "type": "string" }
                        },
                        "required": ["name"]
                    }
                },
                {
                    "name": "delete_tag",
                    "description": "Delete a tag by name.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "name": { "type": "string" } },
                        "required": ["name"]
                    }
                },

                // ----- Files -----
                {
                    "name": "list_files",
                    "description": "List uploaded files. Optionally filter by mimetype prefix (e.g. 'image/').",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "mime_prefix": { "type": "string" }
                        }
                    }
                },
                {
                    "name": "create_file",
                    "description": "Upload a file at the given path. Provide either `data_base64` (binary, e.g. images) or `data` (raw text, e.g. PGN/FEN/SVG). The display title is derived from the basename of the path. Returns the new file id (referenceable via `<image id=\"ID\">` / `<gallery id=\"ID\">` in markdown). Generates a thumbnail automatically for images.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Filename or path used as the file's identifier (must be unique)." },
                            "description": { "type": "string" },
                            "mimetype": { "type": "string", "description": "e.g. image/png. Defaults to application/octet-stream." },
                            "data_base64": { "type": "string", "description": "Base64-encoded binary contents." },
                            "data": { "type": "string", "description": "Raw text contents (alternative to data_base64)." }
                        },
                        "required": ["path"]
                    }
                },
                {
                    "name": "read_file",
                    "description": "Read file metadata by ID (does not return binary contents).",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "id": { "type": "integer" } },
                        "required": ["id"]
                    }
                },
                {
                    "name": "update_file",
                    "description": "Update file metadata (path, description). The display title is always derived from the path basename.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "integer" },
                            "path": { "type": "string" },
                            "description": { "type": "string" }
                        },
                        "required": ["id", "path"]
                    }
                },
                {
                    "name": "delete_file",
                    "description": "Delete a file by ID.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "id": { "type": "integer" } },
                        "required": ["id"]
                    }
                },

                // ----- Galleries -----
                {
                    "name": "list_galleries",
                    "description": "List all galleries.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "read_gallery",
                    "description": "Read a gallery by ID.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "id": { "type": "integer" } },
                        "required": ["id"]
                    }
                },
                {
                    "name": "create_gallery",
                    "description": "Create a gallery from a list of file IDs. `path` is the unique URL slug (e.g. `holiday-2024`).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "title": { "type": "string" },
                            "description": { "type": "string" },
                            "file_ids": { "type": "array", "items": { "type": "integer" } }
                        },
                        "required": ["path", "title"]
                    }
                },
                {
                    "name": "update_gallery",
                    "description": "Update a gallery (replaces all fields).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "integer" },
                            "path": { "type": "string" },
                            "title": { "type": "string" },
                            "description": { "type": "string" },
                            "file_ids": { "type": "array", "items": { "type": "integer" } }
                        },
                        "required": ["id", "path", "title"]
                    }
                },
                {
                    "name": "delete_gallery",
                    "description": "Delete a gallery by ID.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "id": { "type": "integer" } },
                        "required": ["id"]
                    }
                }
            ]
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
        "read_page" => tool_read_page(state, id, arguments).await,
        "edit_page" => tool_edit_page(state, user_id, id, arguments).await,
        "search_pages" => tool_search_pages(state, id, arguments).await,
        "delete_page" => tool_delete_page(state, id, arguments).await,

        // tags
        "list_tags" => tool_list_tags(state, id).await,
        "read_tag" => tool_read_tag(state, id, arguments).await,
        "create_tag" => tool_create_tag(state, id, arguments).await,
        "update_tag" => tool_update_tag(state, id, arguments).await,
        "delete_tag" => tool_delete_tag(state, id, arguments).await,

        // files
        "list_files" => tool_list_files(state, id, arguments).await,
        "create_file" => tool_create_file(state, user_id, id, arguments).await,
        "read_file" => tool_read_file(state, id, arguments).await,
        "update_file" => tool_update_file(state, id, arguments).await,
        "delete_file" => tool_delete_file(state, id, arguments).await,

        // galleries
        "list_galleries" => tool_list_galleries(state, id).await,
        "read_gallery" => tool_read_gallery(state, id, arguments).await,
        "create_gallery" => tool_create_gallery(state, user_id, id, arguments).await,
        "update_gallery" => tool_update_gallery(state, id, arguments).await,
        "delete_gallery" => tool_delete_gallery(state, id, arguments).await,

        _ => JsonRpcResponse::error(id, -32602, format!("Unknown tool: {tool_name}")),
    }
}

fn tool_result(id: Option<Value>, text: String) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        json!({
            "content": [{
                "type": "text",
                "text": text
            }]
        }),
    )
}

fn tool_error(id: Option<Value>, message: &str) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        json!({
            "isError": true,
            "content": [{
                "type": "text",
                "text": message
            }]
        }),
    )
}

fn json_result(id: Option<Value>, value: Value) -> JsonRpcResponse {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    tool_result(id, text)
}

fn parse_args<T: serde::de::DeserializeOwned>(
    id: Option<Value>,
    arguments: Value,
) -> Result<T, JsonRpcResponse> {
    serde_json::from_value(arguments)
        .map_err(|e| tool_error(id, &format!("Invalid arguments: {e}")))
}

// ============================== Pages ==============================

async fn tool_read_page(state: &AppState, id: Option<Value>, arguments: Value) -> JsonRpcResponse {
    let args: ReadPageArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };

    match pages_repo::find_by_path(&state.db, &args.path).await {
        Ok(Some(p)) => {
            let tag_names = tags_repo::resolve_names(&state.db, &p.tag_ids).await;
            let mut out = format!("# {}\n\n", p.path);
            if !tag_names.is_empty() {
                out.push_str(&format!("Tags: {}\n", tag_names.join(", ")));
            }
            if let Some(ref summary) = p.summary {
                out.push_str(&format!("Summary: {summary}\n"));
            }
            out.push_str(&format!("Modified: {}\n", p.modified_at));
            if p.private {
                out.push_str("Private: yes\n");
            }
            out.push_str("\n---\n");
            out.push_str(&p.markdown);
            tool_result(id, out)
        }
        Ok(None) => tool_error(id, &format!("Page not found: {}", args.path)),
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

async fn tool_edit_page(
    state: &AppState,
    user_id: i32,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: EditPageArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };

    if args.markdown.is_none()
        && args.summary.is_none()
        && args.tag_names.is_none()
        && args.private.is_none()
    {
        return tool_error(
            id,
            "Nothing to update — provide markdown, summary, tag_names, or private",
        );
    }

    let tag_ids = match &args.tag_names {
        Some(names) if !names.is_empty() => match tags_repo::resolve_ids(&state.db, names).await {
            Ok(ids) => Some(ids),
            Err(ResolveError::Db(e)) => return tool_error(id, &format!("Database error: {e}")),
            Err(e @ ResolveError::Unknown(_)) => return tool_error(id, &e.to_string()),
        },
        _ => None,
    };

    let outcome = pages_repo::upsert_by_path(
        &state.db,
        user_id,
        &args.path,
        PageUpdate {
            markdown: args.markdown,
            summary: args.summary,
            tag_ids,
            private: args.private,
        },
    )
    .await;

    match outcome {
        Ok(UpsertOutcome::Created(_)) => tool_result(id, format!("created: {}", args.path)),
        Ok(UpsertOutcome::Updated(_)) => tool_result(id, format!("updated: {}", args.path)),
        Err(e) => tool_error(id, &format!("Save failed: {e}")),
    }
}

async fn tool_search_pages(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: SearchPagesArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let limit = args.limit.unwrap_or(20).min(100);
    let offset = args.offset.unwrap_or(0);

    let result = match pages_repo::search(
        &state.db,
        args.prefix.as_deref(),
        args.tag.as_deref(),
        args.q.as_deref(),
        true,
        limit,
        offset,
    )
    .await
    {
        Ok(r) => r,
        Err(pages_repo::SearchError::UnknownTag) => {
            return tool_result(id, "No pages found.".into());
        }
        Err(pages_repo::SearchError::Db(e)) => {
            return tool_error(id, &format!("Database error: {e}"));
        }
    };

    if result.total == 0 {
        return tool_result(id, "No pages found.".into());
    }

    let has_more = offset + limit < result.total;
    let mut out = result
        .pages
        .iter()
        .map(|p| match &p.summary {
            Some(s) if !s.is_empty() => format!("{}: {s}", p.path),
            _ => p.path.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    out.push_str(&format!(
        "\n\n--- total: {}, has_more: {has_more}",
        result.total
    ));
    if has_more {
        out.push_str(&format!(", next_offset: {}", offset + limit));
    }
    out.push_str(" ---");

    tool_result(id, out)
}

async fn tool_delete_page(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: DeletePageArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match pages_repo::delete_by_path(&state.db, &args.path).await {
        Ok(true) => tool_result(id, format!("deleted: {}", args.path)),
        Ok(false) => tool_error(id, &format!("Page not found: {}", args.path)),
        Err(e) => tool_error(id, &format!("Delete failed: {e}")),
    }
}

// ============================== Tags ==============================

async fn tool_list_tags(state: &AppState, id: Option<Value>) -> JsonRpcResponse {
    match tags_repo::list_all(&state.db).await {
        Ok(tags) if tags.is_empty() => tool_result(id, "No tags defined.".into()),
        Ok(tags) => {
            let out = tags
                .iter()
                .map(|t| match &t.description {
                    Some(d) if !d.is_empty() => format!("[{}] {}: {d}", t.id, t.name),
                    _ => format!("[{}] {}", t.id, t.name),
                })
                .collect::<Vec<_>>()
                .join("\n");
            tool_result(id, out)
        }
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

async fn tool_read_tag(state: &AppState, id: Option<Value>, arguments: Value) -> JsonRpcResponse {
    let args: TagArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match tags_repo::find_by_name(&state.db, &args.name).await {
        Ok(Some(t)) => json_result(
            id,
            json!({ "id": t.id, "name": t.name, "description": t.description }),
        ),
        Ok(None) => tool_error(id, &format!("Tag not found: {}", args.name)),
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

async fn tool_create_tag(state: &AppState, id: Option<Value>, arguments: Value) -> JsonRpcResponse {
    let args: TagInputArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if args.name.is_empty() {
        return tool_error(id, "name is required");
    }
    match tags_repo::create_tag(
        &state.db,
        RepoTagInput {
            name: args.name,
            description: args.description,
        },
    )
    .await
    {
        Ok(t) => tool_result(id, format!("created tag [{}] {}", t.id, t.name)),
        Err(e) => tool_error(id, &format!("Create failed: {e}")),
    }
}

async fn tool_update_tag(state: &AppState, id: Option<Value>, arguments: Value) -> JsonRpcResponse {
    let args: UpdateTagArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let name = args.name.clone();
    match tags_repo::update_tag_by_name(
        &state.db,
        &name,
        RepoTagUpdate {
            new_name: args.new_name,
            description: args.description,
        },
    )
    .await
    {
        Ok(Some(t)) => tool_result(id, format!("updated tag [{}] {}", t.id, t.name)),
        Ok(None) => tool_error(id, &format!("Tag not found: {name}")),
        Err(e) => tool_error(id, &format!("Update failed: {e}")),
    }
}

async fn tool_delete_tag(state: &AppState, id: Option<Value>, arguments: Value) -> JsonRpcResponse {
    let args: TagArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match tags_repo::delete_by_name(&state.db, &args.name).await {
        Ok(true) => tool_result(id, format!("deleted tag {}", args.name)),
        Ok(false) => tool_error(id, &format!("Tag not found: {}", args.name)),
        Err(e) => tool_error(id, &format!("Delete failed: {e}")),
    }
}

// ============================== Files ==============================

async fn tool_list_files(state: &AppState, id: Option<Value>, arguments: Value) -> JsonRpcResponse {
    let args: ListFilesArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match files_repo::list_with_thumbnails(&state.db, args.mime_prefix.as_deref()).await {
        Ok(rows) if rows.is_empty() => tool_result(id, "No files.".into()),
        Ok(rows) => {
            let lines: Vec<String> = rows
                .iter()
                .map(|f| {
                    format!(
                        "[{}] {} ({}, {} bytes)",
                        f.model.id, f.model.path, f.model.mimetype, f.model.size_bytes
                    )
                })
                .collect();
            tool_result(id, lines.join("\n"))
        }
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

async fn tool_create_file(
    state: &AppState,
    user_id: i32,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: CreateFileArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if args.path.trim().is_empty() {
        return tool_error(id, "path is required");
    }
    let data = match (args.data_base64, args.data) {
        (Some(b64), _) if !b64.is_empty() => {
            match base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
                Ok(d) => d,
                Err(e) => return tool_error(id, &format!("Invalid base64: {e}")),
            }
        }
        (_, Some(text)) if !text.is_empty() => text.into_bytes(),
        _ => return tool_error(id, "either data_base64 or data is required"),
    };
    if data.is_empty() {
        return tool_error(id, "decoded data is empty");
    }

    let mimetype = args
        .mimetype
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());

    match files_repo::create_file(
        &state.db,
        user_id,
        NewFile {
            path: args.path,
            description: args.description,
            mimetype,
            data,
        },
    )
    .await
    {
        Ok(created) => {
            let title = files_repo::title_from_path(&created.model.path);
            json_result(
                id,
                json!({
                    "id": created.model.id,
                    "path": created.model.path,
                    "title": title,
                    "mimetype": created.model.mimetype,
                    "size_bytes": created.model.size_bytes,
                    "has_thumbnail": created.has_thumbnail,
                    "embed": format!("<image id=\"{}\">", created.model.id),
                }),
            )
        }
        Err(e) => tool_error(id, &format!("Create failed: {e}")),
    }
}

async fn tool_read_file(state: &AppState, id: Option<Value>, arguments: Value) -> JsonRpcResponse {
    let args: FileIdArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match files_repo::find_with_thumbnail(&state.db, args.id).await {
        Ok(Some(f)) => {
            let title = files_repo::title_from_path(&f.model.path);
            json_result(
                id,
                json!({
                    "id": f.model.id,
                    "path": f.model.path,
                    "title": title,
                    "description": f.model.description,
                    "mimetype": f.model.mimetype,
                    "size_bytes": f.model.size_bytes,
                    "has_thumbnail": f.has_thumbnail,
                    "created_at": f.model.created_at.to_string(),
                }),
            )
        }
        Ok(None) => tool_error(id, &format!("File not found: {}", args.id)),
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

async fn tool_update_file(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: UpdateFileArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match files_repo::update_metadata(
        &state.db,
        args.id,
        FileMetaUpdate {
            path: args.path,
            description: args.description,
        },
    )
    .await
    {
        Ok(Some(f)) => tool_result(
            id,
            format!("updated file [{}] {}", f.model.id, f.model.path),
        ),
        Ok(None) => tool_error(id, &format!("File not found: {}", args.id)),
        Err(e) => tool_error(id, &format!("Update failed: {e}")),
    }
}

async fn tool_delete_file(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: FileIdArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match files_repo::delete_by_id(&state.db, args.id).await {
        Ok(true) => tool_result(id, format!("deleted file {}", args.id)),
        Ok(false) => tool_error(id, &format!("File not found: {}", args.id)),
        Err(e) => tool_error(id, &format!("Delete failed: {e}")),
    }
}

// ============================== Galleries ==============================

async fn tool_list_galleries(state: &AppState, id: Option<Value>) -> JsonRpcResponse {
    match galleries_repo::list_all(&state.db).await {
        Ok(rows) if rows.is_empty() => tool_result(id, "No galleries.".into()),
        Ok(rows) => {
            let lines: Vec<String> = rows
                .iter()
                .map(|g| format!("[{}] {} ({} files)", g.id, g.title, g.file_ids.len()))
                .collect();
            tool_result(id, lines.join("\n"))
        }
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

async fn tool_read_gallery(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: GalleryIdArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match galleries_repo::find_by_id(&state.db, args.id).await {
        Ok(Some(g)) => json_result(
            id,
            json!({
                "id": g.id,
                "title": g.title,
                "description": g.description,
                "file_ids": g.file_ids,
                "created_at": g.created_at.to_string(),
            }),
        ),
        Ok(None) => tool_error(id, &format!("Gallery not found: {}", args.id)),
        Err(e) => tool_error(id, &format!("Database error: {e}")),
    }
}

async fn tool_create_gallery(
    state: &AppState,
    user_id: i32,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: CreateGalleryArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if args.title.is_empty() {
        return tool_error(id, "title is required");
    }
    if args.path.trim().is_empty() {
        return tool_error(id, "path is required");
    }
    match galleries_repo::create_gallery(
        &state.db,
        user_id,
        RepoGalleryInput {
            path: args.path,
            title: args.title,
            description: args.description,
            file_ids: args.file_ids,
        },
    )
    .await
    {
        Ok(g) => tool_result(id, format!("created gallery [{}] {}", g.id, g.title)),
        Err(e) => tool_error(id, &format!("Create failed: {e}")),
    }
}

async fn tool_update_gallery(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: UpdateGalleryArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let gallery_id = args.id;
    if args.path.trim().is_empty() {
        return tool_error(id, "path is required");
    }
    match galleries_repo::update_gallery(
        &state.db,
        gallery_id,
        RepoGalleryInput {
            path: args.path,
            title: args.title,
            description: args.description,
            file_ids: args.file_ids,
        },
    )
    .await
    {
        Ok(Some(g)) => tool_result(id, format!("updated gallery [{}] {}", g.id, g.title)),
        Ok(None) => tool_error(id, &format!("Gallery not found: {gallery_id}")),
        Err(e) => tool_error(id, &format!("Update failed: {e}")),
    }
}

async fn tool_delete_gallery(
    state: &AppState,
    id: Option<Value>,
    arguments: Value,
) -> JsonRpcResponse {
    let args: GalleryIdArgs = match parse_args(id.clone(), arguments) {
        Ok(a) => a,
        Err(r) => return r,
    };
    match galleries_repo::delete_by_id(&state.db, args.id).await {
        Ok(true) => tool_result(id, format!("deleted gallery {}", args.id)),
        Ok(false) => tool_error(id, &format!("Gallery not found: {}", args.id)),
        Err(e) => tool_error(id, &format!("Delete failed: {e}")),
    }
}
