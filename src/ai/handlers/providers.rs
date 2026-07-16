use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sea_orm::{ActiveModelTrait, EntityTrait, ModelTrait, QueryOrder, Set};

use crate::entity::llm_provider;
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct ProviderView {
    pub id: i32,
    pub label: String,
    pub kind: String,
    pub base_url: Option<String>,
    pub has_api_key: bool,
    pub created_at: String,
}

impl From<&llm_provider::Model> for ProviderView {
    fn from(p: &llm_provider::Model) -> Self {
        Self {
            id: p.id,
            label: p.label.clone(),
            kind: p.kind.clone(),
            base_url: p.base_url.clone(),
            has_api_key: p.api_key.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
            created_at: p.created_at.to_string(),
        }
    }
}

#[derive(serde::Deserialize)]
pub struct CreateProvider {
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct UpdateProvider {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

fn validate_kind(kind: &str) -> Result<(), ApiError> {
    match kind {
        "anthropic" | "ollama" | "gemini" => Ok(()),
        other => Err(ApiError::BadRequest(format!(
            "unsupported provider kind: {other} (expected anthropic/ollama/gemini)"
        ))),
    }
}

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<ProviderView>>> {
    let rows = llm_provider::Entity::find()
        .order_by_asc(llm_provider::Column::Label)
        .all(&state.db)
        .await?;
    Ok(Json(rows.iter().map(ProviderView::from).collect()))
}

pub async fn read(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<Json<ProviderView>> {
    let row = llm_provider::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ProviderView::from(&row)))
}

pub async fn create(
    State(state): State<AppState>,
    Json(input): Json<CreateProvider>,
) -> ApiResult<(StatusCode, Json<ProviderView>)> {
    if input.label.trim().is_empty() {
        return Err(ApiError::BadRequest("label is required".into()));
    }
    validate_kind(&input.kind)?;
    if matches!(input.kind.as_str(), "anthropic" | "gemini")
        && input
            .api_key
            .as_deref()
            .map(|s| s.is_empty())
            .unwrap_or(true)
    {
        return Err(ApiError::BadRequest(format!(
            "{} provider requires api_key",
            input.kind
        )));
    }
    if input.kind == "ollama"
        && input
            .base_url
            .as_deref()
            .map(|s| s.is_empty())
            .unwrap_or(true)
    {
        return Err(ApiError::BadRequest(
            "ollama provider requires base_url".into(),
        ));
    }

    let saved = llm_provider::ActiveModel {
        label: Set(input.label),
        kind: Set(input.kind),
        api_key: Set(input.api_key.filter(|s| !s.is_empty())),
        base_url: Set(input.base_url.filter(|s| !s.is_empty())),
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
    Ok((StatusCode::CREATED, Json(ProviderView::from(&saved))))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(input): Json<UpdateProvider>,
) -> ApiResult<Json<ProviderView>> {
    let row = llm_provider::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    let mut active: llm_provider::ActiveModel = row.into();
    if let Some(l) = input.label {
        active.label = Set(l);
    }
    if let Some(k) = input.api_key {
        active.api_key = Set(if k.is_empty() { None } else { Some(k) });
    }
    if let Some(u) = input.base_url {
        active.base_url = Set(if u.is_empty() { None } else { Some(u) });
    }
    let updated = active.update(&state.db).await?;
    state
        .agent_engine
        .catalog
        .refresh()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to refresh model catalog: {e}")))?;
    Ok(Json(ProviderView::from(&updated)))
}

pub async fn delete_one(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<StatusCode> {
    let row = llm_provider::Entity::find_by_id(id)
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
