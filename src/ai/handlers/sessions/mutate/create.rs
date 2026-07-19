//! `POST /sessions` — create a session and pin its live engine binding. Split
//! out of `mutate.rs` to keep that file under the workspace's 400-line cap.

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use sea_orm::{ActiveModelTrait, Set};

use super::generation::generation_overrides;
use super::{
    apply_live_changes, default_user_mcp_ids, filter_owned_mcp_ids, resolve_model_with_provider,
    session_mcp_specs, validate_agent_profile,
};
use crate::ai::engine::{BUILD_PROFILE, SiteEngine};
use crate::ai::handlers::sessions::{SessionSummary, ids_to_json};
use crate::entity::assistant_session;
use crate::routes::api::error::ApiResult;
use crate::state::AppState;

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
        Some(&model_row),
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
