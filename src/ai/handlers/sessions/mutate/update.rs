//! `PATCH /sessions/{id}` — mutate a session's row and mirror the change
//! live onto its engine binding. Split out of `mutate.rs` to keep that file
//! under the workspace's 400-line cap.

use axum::Json;
use axum::extract::{Extension, Path, State};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};

use super::generation::{generation_after_model_switch, generation_overrides};
use super::{
    apply_live_changes, filter_owned_mcp_ids, resolve_model_with_provider, session_mcp_specs,
    validate_agent_profile,
};
use crate::ai::handlers::sessions::{SessionSummary, ids_to_json, load_owned, parse_id_array};
use crate::entity::{assistant_session, llm_model, llm_provider};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;
use entanglement_core::SessionId;

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

pub async fn update(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
    Json(input): Json<UpdateSession>,
) -> ApiResult<Json<SessionSummary>> {
    let session = load_owned(&state, user_id, id).await?;
    let session_id = session.engine_session_id.clone().map(SessionId::new);
    let existing_model_id = session.model_id;
    // Snapshot before `session` is consumed into `active` below — needed by
    // `generation_after_model_switch` (#54).
    let existing_temperature = session.temperature;
    let existing_reasoning_effort = session.reasoning_effort.clone();
    let existing_max_output_tokens = session.max_output_tokens.map(|v| v as u32);
    let existing_thinking_budget_tokens = session.thinking_budget_tokens.map(|v| v as u32);

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
    //
    // Gate against the model this update actually leaves the session on: a
    // newly-selected `model_id` if this call changed it, otherwise the
    // session's existing model — a `PATCH` may set generation knobs without
    // touching `model_id` at all, so the *current* model still governs which
    // knobs are legal (#53).
    let generation_target = match &model_changed {
        Some((m, _)) => Some(m.clone()),
        None => match existing_model_id {
            Some(mid) => llm_model::Entity::find_by_id(mid).one(&state.db).await?,
            None => None,
        },
    };
    let generation_changed = generation_overrides(
        generation_target.as_ref(),
        input.temperature,
        input.reasoning_effort.as_deref(),
        input.max_output_tokens,
        input.thinking_budget_tokens,
    )?;
    // Re-apply the row's existing overrides across a model switch, or
    // `rebind()` silently wipes them (#54) — see `generation_after_model_switch`.
    let generation_changed = generation_after_model_switch(
        &model_changed,
        &generation_target,
        generation_changed,
        existing_temperature,
        existing_reasoning_effort.as_deref(),
        existing_max_output_tokens,
        existing_thinking_budget_tokens,
    );
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
