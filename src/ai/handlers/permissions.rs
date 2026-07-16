use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, ModelTrait, QueryFilter, QueryOrder, Set,
};

use crate::ai::tool_permissions::Effect;
use crate::entity::tool_permission;
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct RuleView {
    pub id: i32,
    pub name: String,
    pub effect: String,
    pub priority: i32,
    pub created_at: String,
}

impl From<&tool_permission::Model> for RuleView {
    fn from(r: &tool_permission::Model) -> Self {
        Self {
            id: r.id,
            name: r.name.clone(),
            effect: r.effect.clone(),
            priority: r.priority,
            created_at: r.created_at.to_string(),
        }
    }
}

#[derive(serde::Deserialize)]
pub struct CreateRule {
    pub name: String,
    pub effect: String,
    #[serde(default)]
    pub priority: Option<i32>,
}

#[derive(serde::Deserialize)]
pub struct UpdateRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub effect: Option<String>,
    #[serde(default)]
    pub priority: Option<i32>,
}

fn validate_effect(effect: &str) -> Result<(), ApiError> {
    match Effect::from_str(effect) {
        Effect::Allow | Effect::Deny | Effect::Prompt => Ok(()),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
) -> ApiResult<Json<Vec<RuleView>>> {
    let rows = tool_permission::Entity::find()
        .filter(tool_permission::Column::UserId.eq(user_id))
        .order_by_asc(tool_permission::Column::Priority)
        .order_by_asc(tool_permission::Column::Id)
        .all(&state.db)
        .await?;
    Ok(Json(rows.iter().map(RuleView::from).collect()))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Json(input): Json<CreateRule>,
) -> ApiResult<(StatusCode, Json<RuleView>)> {
    if input.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    validate_effect(&input.effect)?;
    let saved = tool_permission::ActiveModel {
        user_id: Set(user_id),
        name: Set(input.name.trim().to_string()),
        effect: Set(Effect::from_str(&input.effect).as_str().to_string()),
        priority: Set(input.priority.unwrap_or(100)),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state.agent_engine.policy.invalidate_user(user_id);
    Ok((StatusCode::CREATED, Json(RuleView::from(&saved))))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
    Json(input): Json<UpdateRule>,
) -> ApiResult<Json<RuleView>> {
    let row = tool_permission::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    if row.user_id != user_id {
        return Err(ApiError::NotFound);
    }
    let mut active: tool_permission::ActiveModel = row.into();
    if let Some(n) = input.name {
        active.name = Set(n);
    }
    if let Some(e) = input.effect {
        validate_effect(&e)?;
        active.effect = Set(Effect::from_str(&e).as_str().to_string());
    }
    if let Some(p) = input.priority {
        active.priority = Set(p);
    }
    let updated = active.update(&state.db).await?;
    state.agent_engine.policy.invalidate_user(user_id);
    Ok(Json(RuleView::from(&updated)))
}

pub async fn delete_one(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
) -> ApiResult<StatusCode> {
    let row = tool_permission::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    if row.user_id != user_id {
        return Err(ApiError::NotFound);
    }
    row.delete(&state.db).await?;
    state.agent_engine.policy.invalidate_user(user_id);
    Ok(StatusCode::NO_CONTENT)
}
