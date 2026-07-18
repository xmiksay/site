use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ModelTrait, QueryFilter, QueryOrder, Set,
};

use crate::entity::{llm_model, llm_provider};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct ModelView {
    pub id: i32,
    pub provider_id: i32,
    pub provider_label: String,
    pub provider_kind: String,
    pub label: String,
    pub model: String,
    pub is_default: bool,
    pub context_window: Option<i32>,
    pub created_at: String,
}

#[derive(serde::Deserialize)]
pub struct CreateModel {
    pub provider_id: i32,
    pub label: String,
    pub model: String,
    #[serde(default)]
    pub is_default: Option<bool>,
    #[serde(default)]
    pub context_window: Option<i32>,
}

#[derive(serde::Deserialize)]
pub struct UpdateModel {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub is_default: Option<bool>,
    #[serde(default)]
    pub context_window: Option<i32>,
}

async fn enrich(state: &AppState, rows: Vec<llm_model::Model>) -> ApiResult<Vec<ModelView>> {
    let providers = llm_provider::Entity::find()
        .all(&state.db)
        .await?
        .into_iter()
        .map(|p| (p.id, p))
        .collect::<std::collections::HashMap<_, _>>();

    Ok(rows
        .into_iter()
        .filter_map(|m| {
            let p = providers.get(&m.provider_id)?;
            Some(ModelView {
                id: m.id,
                provider_id: m.provider_id,
                provider_label: p.label.clone(),
                provider_kind: p.kind.clone(),
                label: m.label,
                model: m.model,
                is_default: m.is_default,
                context_window: m.context_window,
                created_at: m.created_at.to_string(),
            })
        })
        .collect())
}

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<ModelView>>> {
    let rows = llm_model::Entity::find()
        .order_by_desc(llm_model::Column::IsDefault)
        .order_by_asc(llm_model::Column::Label)
        .all(&state.db)
        .await?;
    Ok(Json(enrich(&state, rows).await?))
}

pub async fn create(
    State(state): State<AppState>,
    Json(input): Json<CreateModel>,
) -> ApiResult<(StatusCode, Json<ModelView>)> {
    if input.label.trim().is_empty() {
        return Err(ApiError::BadRequest("label is required".into()));
    }
    if input.model.trim().is_empty() {
        return Err(ApiError::BadRequest("model is required".into()));
    }
    llm_provider::Entity::find_by_id(input.provider_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("provider {} not found", input.provider_id)))?;

    let want_default = input.is_default.unwrap_or(false);
    if want_default {
        clear_default(&state).await?;
    }
    let none_yet = llm_model::Entity::find().one(&state.db).await?.is_none();

    let saved = llm_model::ActiveModel {
        provider_id: Set(input.provider_id),
        label: Set(input.label),
        model: Set(input.model),
        is_default: Set(want_default || none_yet),
        context_window: Set(input.context_window),
        ..Default::default()
    }
    .insert(&state.db)
    .await
    .map_err(|e| ApiError::Conflict(format!("could not save: {e}")))?;

    state
        .agent_engine
        .catalog
        .refresh()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to refresh model catalog: {e}")))?;
    let view = enrich(&state, vec![saved])
        .await?
        .into_iter()
        .next()
        .unwrap();
    Ok((StatusCode::CREATED, Json(view)))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(input): Json<UpdateModel>,
) -> ApiResult<Json<ModelView>> {
    let row = llm_model::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    if input.is_default == Some(true) {
        clear_default(&state).await?;
    }
    let mut active: llm_model::ActiveModel = row.into();
    if let Some(l) = input.label {
        active.label = Set(l);
    }
    if let Some(m) = input.model {
        active.model = Set(m);
    }
    if let Some(d) = input.is_default {
        active.is_default = Set(d);
    }
    if let Some(w) = input.context_window {
        active.context_window = Set(Some(w));
    }
    let updated = active.update(&state.db).await?;
    state
        .agent_engine
        .catalog
        .refresh()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to refresh model catalog: {e}")))?;
    let view = enrich(&state, vec![updated])
        .await?
        .into_iter()
        .next()
        .unwrap();
    Ok(Json(view))
}

pub async fn delete_one(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<StatusCode> {
    let row = llm_model::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    row.delete(&state.db).await?;
    state
        .agent_engine
        .catalog
        .refresh()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to refresh model catalog: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_default(state: &AppState) -> ApiResult<()> {
    let defaults = llm_model::Entity::find()
        .filter(llm_model::Column::IsDefault.eq(true))
        .all(&state.db)
        .await?;
    for row in defaults {
        let mut active: llm_model::ActiveModel = row.into();
        active.is_default = Set(false);
        active.update(&state.db).await?;
    }
    Ok(())
}
