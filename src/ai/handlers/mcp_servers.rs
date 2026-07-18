use std::collections::HashMap;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ModelTrait, QueryFilter, QueryOrder, Set,
};
use serde_json::{Value, json};

use crate::ai::tool_permissions;
use crate::entity::user_mcp_server;
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct McpServerView {
    pub id: i32,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub forward_user_token: bool,
    pub headers: HashMap<String, String>,
    /// Config-side capability hint (#39, ADR-0117): raw remote tool name →
    /// capability (`read`/`write`/`call`) — see `tool_permissions::CAPABILITIES`.
    pub capabilities: HashMap<String, String>,
    pub created_at: String,
}

fn string_map(v: &Value) -> HashMap<String, String> {
    v.as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default()
}

impl From<&user_mcp_server::Model> for McpServerView {
    fn from(m: &user_mcp_server::Model) -> Self {
        Self {
            id: m.id,
            name: m.name.clone(),
            url: m.url.clone(),
            enabled: m.enabled,
            forward_user_token: m.forward_user_token,
            headers: string_map(&m.headers),
            capabilities: string_map(&m.capabilities),
            created_at: m.created_at.to_string(),
        }
    }
}

fn json_map(m: HashMap<String, String>) -> Value {
    Value::Object(m.into_iter().map(|(k, v)| (k, Value::String(v))).collect())
}

/// A capability annotation's values must be one of this site's own capability
/// names (#39, `tool_permissions::CAPABILITIES`) — a typo'd value would
/// otherwise silently fail to fan out at resolve time instead of erroring up
/// front, matching how every other rule field here validates eagerly.
fn validate_capabilities(capabilities: &HashMap<String, String>) -> Result<(), ApiError> {
    for (tool, capability) in capabilities {
        if !tool_permissions::CAPABILITIES
            .iter()
            .any(|(name, _)| name == capability)
        {
            return Err(ApiError::BadRequest(format!(
                "capabilities.{tool}: unknown capability `{capability}` (expected `read`, `write`, or `call`)"
            )));
        }
    }
    Ok(())
}

#[derive(serde::Deserialize)]
pub struct CreateMcpServer {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub forward_user_token: Option<bool>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub capabilities: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
pub struct UpdateMcpServer {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub forward_user_token: Option<bool>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub capabilities: Option<HashMap<String, String>>,
}

/// Build the `discovered` list from `SiteMcp`'s per-user tool-spec cache
/// (`"{server}__{tool}"`-prefixed) grouped back by owning server row.
///
/// Note: `SiteMcp::tool_specs_for_user` only queries enabled servers and
/// silently drops ones it can't reach (see `mcp.rs`'s doc), and is cached for
/// 60s — so here `connected` is inferred from "did any tool surface for this
/// server's prefix", which can't distinguish "disabled" from "connection
/// failed", and a server added/fixed elsewhere can lag up to 60s before this
/// reflects it. Acceptable for this phase (see the issue's note on this exact
/// tradeoff); a future pass could add a real per-server discovery method to
/// `SiteMcp` if that distinction turns out to matter.
async fn discovered_servers(
    state: &AppState,
    user_id: i32,
    rows: &[user_mcp_server::Model],
) -> Vec<Value> {
    let specs = state.agent_engine.mcp.tool_specs_for_user(user_id).await;
    rows.iter()
        .map(|row| {
            let prefix = format!("{}__", row.name);
            let tools: Vec<Value> = specs
                .iter()
                .filter(|s| s.name.starts_with(&prefix))
                .map(|s| {
                    json!({
                        "name": s.name.strip_prefix(&prefix).unwrap_or(&s.name),
                        "prefixed_name": s.name,
                        "description": s.description,
                        "schema": s.schema,
                    })
                })
                .collect();
            json!({
                "name": row.name,
                "url": row.url,
                "enabled": row.enabled,
                "forward_user_token": row.forward_user_token,
                "connected": !tools.is_empty(),
                "tools": tools,
            })
        })
        .collect()
}

pub async fn list(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
) -> ApiResult<Json<Value>> {
    let registered = user_mcp_server::Entity::find()
        .filter(user_mcp_server::Column::UserId.eq(user_id))
        .order_by_asc(user_mcp_server::Column::Name)
        .all(&state.db)
        .await?;
    let registered_view: Vec<McpServerView> = registered.iter().map(McpServerView::from).collect();
    let discovered = discovered_servers(&state, user_id, &registered).await;

    Ok(Json(json!({
        "user_servers": registered_view,
        "discovered": discovered,
    })))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Json(input): Json<CreateMcpServer>,
) -> ApiResult<(StatusCode, Json<McpServerView>)> {
    let name = input.name.trim().to_string();
    let url = input.url.trim().to_string();
    if name.is_empty() || url.is_empty() {
        return Err(ApiError::BadRequest("name and url required".into()));
    }
    validate_capabilities(&input.capabilities)?;

    let now = chrono::Utc::now().fixed_offset();
    let saved = user_mcp_server::ActiveModel {
        user_id: Set(user_id),
        name: Set(name),
        url: Set(url),
        enabled: Set(input.enabled.unwrap_or(true)),
        forward_user_token: Set(input.forward_user_token.unwrap_or(false)),
        headers: Set(json_map(input.headers)),
        capabilities: Set(json_map(input.capabilities)),
        created_at: Set(now),
        ..Default::default()
    }
    .insert(&state.db)
    .await
    .map_err(|e| ApiError::Conflict(format!("could not save: {e}")))?;

    state.agent_engine.mcp.invalidate_user(user_id);
    Ok((StatusCode::CREATED, Json(McpServerView::from(&saved))))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
    Json(input): Json<UpdateMcpServer>,
) -> ApiResult<Json<McpServerView>> {
    let row = user_mcp_server::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    if row.user_id != user_id {
        return Err(ApiError::NotFound);
    }

    let mut active: user_mcp_server::ActiveModel = row.into();
    if let Some(v) = input.name {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".into()));
        }
        active.name = Set(trimmed.to_string());
    }
    if let Some(v) = input.enabled {
        active.enabled = Set(v);
    }
    if let Some(v) = input.forward_user_token {
        active.forward_user_token = Set(v);
    }
    if let Some(v) = input.url {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            return Err(ApiError::BadRequest("url cannot be empty".into()));
        }
        active.url = Set(trimmed.to_string());
    }
    if let Some(h) = input.headers {
        active.headers = Set(json_map(h));
    }
    if let Some(c) = input.capabilities {
        validate_capabilities(&c)?;
        active.capabilities = Set(json_map(c));
    }
    let updated = active.update(&state.db).await?;
    state.agent_engine.mcp.invalidate_user(user_id);
    Ok(Json(McpServerView::from(&updated)))
}

pub async fn delete_one(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
) -> ApiResult<StatusCode> {
    let row = user_mcp_server::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    if row.user_id != user_id {
        return Err(ApiError::NotFound);
    }
    row.delete(&state.db).await?;
    state.agent_engine.mcp.invalidate_user(user_id);
    Ok(StatusCode::NO_CONTENT)
}
