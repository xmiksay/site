//! `create`/`update` — the two session handlers that mutate an
//! `assistant_session` row *and* talk live to the engine (`InMsg::SetModel`/
//! `SetAgent`/`SetGeneration`, #42). Split out of `mod.rs` to keep that file
//! under the workspace's 400-line cap, mirroring how `turn.rs`/`compact.rs`
//! already carry the other engine-talking handlers; `list`/`read`/
//! `delete_one` stay in `mod.rs` since they never send an `InMsg`. `create`/
//! `update` themselves are further split into their own files (this file's
//! own 400-line cap) — this file keeps only what both share.

use entanglement_core::{GenerationParams, InMsg, SessionId};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::ai::engine::{SWITCHABLE_PROFILES, SiteEngine};
use crate::entity::{llm_model, llm_provider, user_mcp_server};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

mod create;
mod generation;
mod update;

pub use create::create;
pub use update::update;

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
