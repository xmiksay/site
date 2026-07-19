//! `/api/assistant/sessions*` — CRUD over `assistant_session` rows plus the
//! engine wiring each mutation needs (minting/closing the engine session,
//! pushing `SetModel`/`SetAgent`/`SetGeneration`, refreshing per-session MCP
//! tool specs). The two handlers that actually talk live to the engine on
//! write (`create`/`update`, #42) live in `mutate.rs`, and the turn-driving
//! handlers (`send_message`/`approve`, which talk to `Holly` and project
//! `assistant_events`) live in `turn.rs` — both split out to keep this file
//! under the workspace's 400-line cap and re-exported here so the router
//! (`handlers/mod.rs`) sees one flat `sessions::*` surface.
//!
//! `turn::session_for_call_awaiting` is also re-exported, but only for
//! `tests/assistant_session_subagent_approval_race.rs` to call directly
//! against a real DB — see that function's own doc for why.

mod compact;
mod mutate;
mod turn;

pub use compact::compact;
pub use mutate::{create, update};
pub use turn::{approve, send_message, session_for_call_awaiting};

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use entanglement_core::{InMsg, SessionId};
use sea_orm::{ColumnTrait, EntityTrait, ModelTrait, QueryFilter, QueryOrder};
use serde_json::Value;

use crate::ai::persistence;
use crate::entity::assistant_session;
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
    /// Session-level generation overrides (#42) — mirrors `assistant_session`'s
    /// own columns verbatim, `None` meaning "no override, use the model's
    /// default".
    pub temperature: Option<f32>,
    pub reasoning_effort: Option<String>,
    pub max_output_tokens: Option<i32>,
    pub thinking_budget_tokens: Option<i32>,
    pub agent_profile: String,
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
            temperature: s.temperature,
            reasoning_effort: s.reasoning_effort.clone(),
            max_output_tokens: s.max_output_tokens,
            thinking_budget_tokens: s.thinking_budget_tokens,
            agent_profile: s.agent_profile.clone(),
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
