//! `/api/assistant/sessions*` — CRUD over `assistant_session` rows plus the
//! engine wiring each mutation needs (minting/closing the engine session,
//! pushing `SetModel`, refreshing per-session MCP tool specs). The actual
//! turn-driving handlers (`send_message`/`approve`, which talk to `Holly` and
//! project `assistant_events`) live in `turn.rs` to keep this file under the
//! workspace's 400-line cap; both are re-exported here so the router
//! (`handlers/mod.rs`) sees one flat `sessions::*` surface.

mod compact;
mod turn;

pub use compact::compact;
pub use turn::{approve, send_message};

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use entanglement_core::{InMsg, SessionId};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ModelTrait, QueryFilter, QueryOrder, Set,
};
use serde_json::Value;

use crate::ai::engine::SiteEngine;
use crate::ai::persistence;
use crate::entity::{assistant_session, llm_model, llm_provider, user_mcp_server};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct SessionSummary {
    pub id: i32,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub model_id: Option<i32>,
    pub enabled_mcp_server_ids: Vec<i32>,
    pub created_at: String,
    pub updated_at: String,
}

fn parse_id_array(raw: &Value) -> Vec<i32> {
    raw.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_i64().map(|n| n as i32))
                .collect()
        })
        .unwrap_or_default()
}

fn ids_to_json(ids: &[i32]) -> Value {
    Value::Array(ids.iter().map(|n| Value::from(*n)).collect())
}

impl From<&assistant_session::Model> for SessionSummary {
    fn from(s: &assistant_session::Model) -> Self {
        Self {
            id: s.id,
            title: s.title.clone(),
            provider: s.provider.clone(),
            model: s.model.clone(),
            model_id: s.model_id,
            enabled_mcp_server_ids: parse_id_array(&s.enabled_mcp_server_ids),
            created_at: s.created_at.to_string(),
            updated_at: s.updated_at.to_string(),
        }
    }
}

#[derive(serde::Serialize)]
pub struct MessageView {
    pub id: i32,
    pub seq: i32,
    pub role: String,
    pub content: Value,
    pub created_at: String,
}

#[derive(serde::Serialize)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub summary: SessionSummary,
    pub messages: Vec<MessageView>,
}

#[derive(serde::Deserialize)]
pub struct CreateSession {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub model_id: Option<i32>,
    /// Optional explicit selection. When omitted, defaults to the user's
    /// currently-enabled MCP servers.
    #[serde(default)]
    pub enabled_mcp_server_ids: Option<Vec<i32>>,
}

#[derive(serde::Deserialize)]
pub struct UpdateSession {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub model_id: Option<i32>,
    #[serde(default)]
    pub enabled_mcp_server_ids: Option<Vec<i32>>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
) -> ApiResult<Json<Vec<SessionSummary>>> {
    let rows = assistant_session::Entity::find()
        .filter(assistant_session::Column::UserId.eq(user_id))
        .order_by_desc(assistant_session::Column::UpdatedAt)
        .all(&state.db)
        .await?;
    Ok(Json(rows.iter().map(SessionSummary::from).collect()))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Json(input): Json<CreateSession>,
) -> ApiResult<(StatusCode, Json<SessionSummary>)> {
    let (model_row, provider_row) = resolve_model_with_provider(&state, input.model_id).await?;

    let mcp_ids = match input.enabled_mcp_server_ids {
        Some(ids) => filter_owned_mcp_ids(&state, user_id, &ids).await?,
        None => default_user_mcp_ids(&state, user_id).await?,
    };

    let session_id = SiteEngine::session_id_for_user(user_id);
    let now = chrono::Utc::now().fixed_offset();
    let title = input.title.unwrap_or_else(|| "New chat".into());

    let saved = assistant_session::ActiveModel {
        user_id: Set(user_id),
        title: Set(title),
        provider: Set(provider_row.kind.clone()),
        model: Set(model_row.model.clone()),
        model_id: Set(Some(model_row.id)),
        enabled_mcp_server_ids: Set(ids_to_json(&mcp_ids)),
        engine_session_id: Set(Some(session_id.0.clone())),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;

    // A brand-new session id has no history to resume — sending its first
    // `InMsg` lazily spawns it blank, which is exactly right here. Mark it
    // live immediately so a later handler call in this process doesn't try
    // (and fail) to `resume` an id the engine already has live.
    let engine = &state.agent_engine;
    engine
        .holly
        .send(InMsg::SetModel {
            session: session_id.clone(),
            provider: provider_row.label.clone(),
            model: model_row.id.to_string(),
        })
        .await
        .map_err(|_| ApiError::Internal("engine inbox closed".into()))?;
    engine.mark_live(session_id.clone());

    let specs = session_mcp_specs(&state, user_id, &mcp_ids).await;
    engine.set_session_mcp_specs(session_id, specs);

    Ok((StatusCode::CREATED, Json(SessionSummary::from(&saved))))
}

async fn default_user_mcp_ids(state: &AppState, user_id: i32) -> ApiResult<Vec<i32>> {
    let rows = user_mcp_server::Entity::find()
        .filter(user_mcp_server::Column::UserId.eq(user_id))
        .filter(user_mcp_server::Column::Enabled.eq(true))
        .all(&state.db)
        .await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// Keep only IDs that actually belong to the user. Silently drops unknown or
/// foreign ids — prevents a session from referencing servers another user
/// owns.
async fn filter_owned_mcp_ids(state: &AppState, user_id: i32, ids: &[i32]) -> ApiResult<Vec<i32>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = user_mcp_server::Entity::find()
        .filter(user_mcp_server::Column::UserId.eq(user_id))
        .filter(user_mcp_server::Column::Id.is_in(ids.to_vec()))
        .all(&state.db)
        .await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// Tool specs visible to this *session*, not just this user: `SiteMcp` only
/// knows per-user enabled servers (`user_mcp_server.enabled`), so narrow that
/// down to the subset the session actually opted into (`mcp_ids`, from
/// `assistant_session.enabled_mcp_server_ids`) by filtering on the
/// `"{server_name}__{tool}"` spec-naming convention `mcp.rs` documents.
async fn session_mcp_specs(
    state: &AppState,
    user_id: i32,
    mcp_ids: &[i32],
) -> Vec<entanglement_core::ToolSpec> {
    if mcp_ids.is_empty() {
        return Vec::new();
    }
    let names: Vec<String> = user_mcp_server::Entity::find()
        .filter(user_mcp_server::Column::Id.is_in(mcp_ids.to_vec()))
        .all(&state.db)
        .await
        .map(|rows| rows.into_iter().map(|r| r.name).collect())
        .unwrap_or_default();
    if names.is_empty() {
        return Vec::new();
    }
    state
        .agent_engine
        .mcp
        .tool_specs_for_user(user_id)
        .await
        .into_iter()
        .filter(|spec| {
            names
                .iter()
                .any(|n| spec.name.starts_with(&format!("{n}__")))
        })
        .collect()
}

pub async fn read(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
) -> ApiResult<Json<SessionDetail>> {
    let session = load_owned(&state, user_id, id).await?;
    let records = match &session.engine_session_id {
        Some(sid) => {
            let session_id = SessionId::new(sid.clone());
            // So a freshly-restarted server can still show history for an old
            // session — best-effort: a resume hiccup shouldn't fail a read
            // when `assistant_events` (the actual source of truth here) is
            // fine regardless of the in-memory task's state.
            if let Err(e) = state
                .agent_engine
                .ensure_live(&state.db, session_id.clone())
                .await
            {
                tracing::warn!(error = %e, session_id = %sid, "failed to resume engine session for read");
            }
            turn::load_prior_records(&state.db, &session_id).await?
        }
        None => Vec::new(),
    };
    let projected = crate::ai::projection::project(&records);
    Ok(Json(turn::to_detail(&session, projected)))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
    Json(input): Json<UpdateSession>,
) -> ApiResult<Json<SessionSummary>> {
    let session = load_owned(&state, user_id, id).await?;
    let session_id = session.engine_session_id.clone().map(SessionId::new);

    let mut mcp_changed = false;
    let mut new_mcp_ids: Vec<i32> = parse_id_array(&session.enabled_mcp_server_ids);
    let mut model_changed: Option<(llm_model::Model, llm_provider::Model)> = None;
    let mut active: assistant_session::ActiveModel = session.into();

    if let Some(t) = input.title {
        active.title = Set(t);
    }
    if let Some(mid) = input.model_id {
        let (model_row, provider_row) = resolve_model_with_provider(&state, Some(mid)).await?;
        active.model_id = Set(Some(mid));
        active.provider = Set(provider_row.kind.clone());
        active.model = Set(model_row.model.clone());
        model_changed = Some((model_row, provider_row));
    }
    if let Some(ids) = input.enabled_mcp_server_ids {
        let owned = filter_owned_mcp_ids(&state, user_id, &ids).await?;
        active.enabled_mcp_server_ids = Set(ids_to_json(&owned));
        new_mcp_ids = owned;
        mcp_changed = true;
    }
    active.updated_at = Set(chrono::Utc::now().fixed_offset());
    let updated = active.update(&state.db).await?;

    if let Some(session_id) = session_id {
        let engine = &state.agent_engine;
        if let Some((model_row, provider_row)) = model_changed {
            // The session may have existing history this process hasn't
            // resumed yet — resume first, or `SetModel` would lazily spawn a
            // blank in-memory session and permanently desync it from
            // `assistant_events` (see `SiteEngine::ensure_live`'s doc).
            engine
                .ensure_live(&state.db, session_id.clone())
                .await
                .map_err(|e| ApiError::Internal(format!("failed to resume engine session: {e}")))?;
            engine
                .holly
                .send(InMsg::SetModel {
                    session: session_id.clone(),
                    provider: provider_row.label.clone(),
                    model: model_row.id.to_string(),
                })
                .await
                .map_err(|_| ApiError::Internal("engine inbox closed".into()))?;
        }
        if mcp_changed {
            let specs = session_mcp_specs(&state, user_id, &new_mcp_ids).await;
            engine.set_session_mcp_specs(session_id, specs);
        }
    }

    Ok(Json(SessionSummary::from(&updated)))
}

pub async fn delete_one(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
) -> ApiResult<StatusCode> {
    let session = load_owned(&state, user_id, id).await?;
    if let Some(sid) = &session.engine_session_id {
        let session_id = SessionId::new(sid.clone());
        state
            .agent_engine
            .holly
            .send(InMsg::CloseSession {
                session: session_id.clone(),
            })
            .await
            .map_err(|_| ApiError::Internal("engine inbox closed".into()))?;
        persistence::delete_session_events(&state.db, &session_id)
            .await
            .map_err(|e| ApiError::Internal(format!("failed to delete assistant_events: {e}")))?;
        state.agent_engine.clear_session_mcp_specs(&session_id);
        state.agent_engine.forget_live(&session_id);
    }
    session.delete(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn load_owned(
    state: &AppState,
    user_id: i32,
    id: i32,
) -> ApiResult<assistant_session::Model> {
    let session = assistant_session::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    if session.user_id != user_id {
        return Err(ApiError::NotFound);
    }
    Ok(session)
}

async fn resolve_model_with_provider(
    state: &AppState,
    model_id: Option<i32>,
) -> ApiResult<(llm_model::Model, llm_provider::Model)> {
    let model_row = match model_id {
        Some(mid) => llm_model::Entity::find_by_id(mid)
            .one(&state.db)
            .await?
            .ok_or_else(|| ApiError::BadRequest(format!("model {mid} not found")))?,
        None => {
            let mut rows = llm_model::Entity::find().all(&state.db).await?;
            if rows.is_empty() {
                return Err(ApiError::BadRequest(
                    "no LLM models configured — add a provider and a model first".into(),
                ));
            }
            rows.sort_by_key(|r| (!r.is_default, r.id));
            rows.into_iter().next().unwrap()
        }
    };
    let provider_row = llm_provider::Entity::find_by_id(model_row.provider_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| ApiError::Internal("model points at missing provider".into()))?;
    Ok((model_row, provider_row))
}
