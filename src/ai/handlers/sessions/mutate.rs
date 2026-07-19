//! `create`/`update` — the two session handlers that mutate an
//! `assistant_session` row *and* talk live to the engine (`InMsg::SetModel`/
//! `SetAgent`/`SetGeneration`, #42). Split out of `mod.rs` to keep that file
//! under the workspace's 400-line cap, mirroring how `turn.rs`/`compact.rs`
//! already carry the other engine-talking handlers; `list`/`read`/
//! `delete_one` stay in `mod.rs` since they never send an `InMsg`.

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use entanglement_core::{GenerationParams, InMsg, SessionId};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use generation::generation_overrides;

use super::{SessionSummary, ids_to_json, load_owned, parse_id_array};
use crate::ai::engine::{BUILD_PROFILE, SWITCHABLE_PROFILES, SiteEngine};
use crate::entity::{assistant_session, llm_model, llm_provider, user_mcp_server};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

mod generation;

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
    /// `#42` — omitted means "no override, use the model's default".
    #[serde(default)]
    pub temperature: Option<f32>,
    /// `"low" | "medium" | "high"` (#42); omitted means "no override".
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Hard cap on tokens generated per turn (#42); omitted means "no
    /// override, use the model's default".
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    /// Extended-thinking budget in tokens (#42); omitted means "no override".
    #[serde(default)]
    pub thinking_budget_tokens: Option<u32>,
    /// One of `SWITCHABLE_PROFILES` (#42); omitted defaults to `BUILD_PROFILE`.
    #[serde(default)]
    pub agent_profile: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct UpdateSession {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub model_id: Option<i32>,
    #[serde(default)]
    pub enabled_mcp_server_ids: Option<Vec<i32>>,
    /// `#42` — omitted leaves the session's current override untouched.
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Hard cap on tokens generated per turn (#42); omitted leaves the
    /// session's current override untouched.
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    /// Extended-thinking budget in tokens (#42); omitted leaves the session's
    /// current override untouched.
    #[serde(default)]
    pub thinking_budget_tokens: Option<u32>,
    #[serde(default)]
    pub agent_profile: Option<String>,
}

/// `InMsg::SetAgent`'s target must be a profile this site actually registers
/// (`SWITCHABLE_PROFILES`) — `entanglement_core` itself imposes no reachability
/// gate on a direct `SetAgent` (see `engine/profiles.rs`'s doc), so this is the
/// site's own allowlist check.
fn validate_agent_profile(name: &str) -> ApiResult<()> {
    if SWITCHABLE_PROFILES.contains(&name) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!(
            "unknown agent profile `{name}` (expected one of {SWITCHABLE_PROFILES:?})"
        )))
    }
}

/// Send whichever of `SetModel`/`SetAgent`/`SetGeneration` this call actually
/// changed, in that order — safe regardless of order since neither of this
/// site's built-in profiles pins a model (`engine/profiles.rs`), so `SetAgent`
/// never clobbers an explicit `model_id` selection. Shared by `create`/
/// `update` so the three send-and-map-err blocks exist once.
async fn apply_live_changes(
    engine: &SiteEngine,
    session_id: &SessionId,
    model: Option<(String, String)>,
    agent: Option<String>,
    generation: Option<GenerationParams>,
) -> ApiResult<()> {
    if let Some((provider, model)) = model {
        engine
            .holly
            .send(InMsg::SetModel {
                session: session_id.clone(),
                provider,
                model,
            })
            .await
            .map_err(|_| ApiError::Internal("engine inbox closed".into()))?;
    }
    if let Some(agent) = agent {
        engine
            .holly
            .send(InMsg::SetAgent {
                session: session_id.clone(),
                agent,
            })
            .await
            .map_err(|_| ApiError::Internal("engine inbox closed".into()))?;
    }
    if let Some(overrides) = generation {
        engine
            .holly
            .send(InMsg::SetGeneration {
                session: session_id.clone(),
                overrides,
            })
            .await
            .map_err(|_| ApiError::Internal("engine inbox closed".into()))?;
    }
    Ok(())
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Json(input): Json<CreateSession>,
) -> ApiResult<(StatusCode, Json<SessionSummary>)> {
    let (model_row, provider_row) = resolve_model_with_provider(&state, input.model_id).await?;

    // Validate before writing anything — an unknown profile/reasoning-effort
    // string must not leave a half-configured session row behind.
    let agent_profile = match &input.agent_profile {
        Some(name) => {
            validate_agent_profile(name)?;
            name.clone()
        }
        None => BUILD_PROFILE.to_string(),
    };
    let generation = generation_overrides(
        input.temperature,
        input.reasoning_effort.as_deref(),
        input.max_output_tokens,
        input.thinking_budget_tokens,
    )?;

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
        temperature: Set(input.temperature),
        reasoning_effort: Set(input.reasoning_effort.clone()),
        max_output_tokens: Set(input.max_output_tokens.map(|n| n as i32)),
        thinking_budget_tokens: Set(input.thinking_budget_tokens.map(|n| n as i32)),
        agent_profile: Set(agent_profile.clone()),
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
    apply_live_changes(
        engine,
        &session_id,
        Some((provider_row.label.clone(), model_row.id.to_string())),
        (agent_profile != BUILD_PROFILE).then_some(agent_profile),
        generation,
    )
    .await?;
    engine.mark_live(session_id.clone());

    let specs = session_mcp_specs(&state, user_id, &mcp_ids).await;
    engine.set_session_mcp_specs(session_id, specs);

    Ok((StatusCode::CREATED, Json(SessionSummary::from(&saved))))
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
    let mut agent_changed: Option<String> = None;
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
    if let Some(name) = input.agent_profile {
        validate_agent_profile(&name)?;
        active.agent_profile = Set(name.clone());
        agent_changed = Some(name);
    }
    // `generation_overrides` validates `reasoning_effort` up front; persist
    // exactly the fields the caller sent (an omitted field keeps the row's
    // current value, same partial-update convention as `title`/`model_id`
    // above — SeaORM's `NotSet` leaves the column untouched).
    let generation_changed = generation_overrides(
        input.temperature,
        input.reasoning_effort.as_deref(),
        input.max_output_tokens,
        input.thinking_budget_tokens,
    )?;
    if let Some(t) = input.temperature {
        active.temperature = Set(Some(t));
    }
    if let Some(r) = input.reasoning_effort {
        active.reasoning_effort = Set(Some(r));
    }
    if let Some(v) = input.max_output_tokens {
        active.max_output_tokens = Set(Some(v as i32));
    }
    if let Some(v) = input.thinking_budget_tokens {
        active.thinking_budget_tokens = Set(Some(v as i32));
    }
    active.updated_at = Set(chrono::Utc::now().fixed_offset());
    let updated = active.update(&state.db).await?;

    if let Some(session_id) = session_id {
        let engine = &state.agent_engine;
        if model_changed.is_some() || agent_changed.is_some() || generation_changed.is_some() {
            // The session may have existing history this process hasn't
            // resumed yet — resume first, or a live `SetModel`/`SetAgent`/
            // `SetGeneration` would lazily spawn a blank in-memory session and
            // permanently desync it from `assistant_events` (see
            // `SiteEngine::ensure_live`'s doc).
            engine
                .ensure_live(&state.db, session_id.clone())
                .await
                .map_err(|e| ApiError::Internal(format!("failed to resume engine session: {e}")))?;
            apply_live_changes(
                engine,
                &session_id,
                model_changed.map(|(m, p)| (p.label, m.id.to_string())),
                agent_changed,
                generation_changed,
            )
            .await?;
        }
        if mcp_changed {
            let specs = session_mcp_specs(&state, user_id, &new_mcp_ids).await;
            engine.set_session_mcp_specs(session_id, specs);
        }
    }

    Ok(Json(SessionSummary::from(&updated)))
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
///
/// `pub(super)`: also called by `compact.rs` to carry a session's MCP
/// selection forward onto its `/compact` successor.
pub(super) async fn session_mcp_specs(
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

/// `pub(super)`: also called by `compact.rs` to re-pin the `/compact`
/// successor's model.
pub(super) async fn resolve_model_with_provider(
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
