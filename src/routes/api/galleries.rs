use axum::Json;
use axum::Router;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::get;

use crate::entity::gallery;
use crate::repo::galleries::{
    self as galleries_repo, GalleryInput as RepoGalleryInput, GallerySaveError,
};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::routes::broadcast;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/paths", get(list_paths))
        .route("/{id}", get(read).put(update).delete(delete_one))
}

impl From<GallerySaveError> for ApiError {
    fn from(e: GallerySaveError) -> Self {
        match e {
            GallerySaveError::EmptyPath => ApiError::BadRequest("path is required".into()),
            GallerySaveError::EmptyTitle => ApiError::BadRequest("title is required".into()),
            GallerySaveError::Db(db) => ApiError::from(db),
        }
    }
}

#[derive(serde::Deserialize)]
pub struct GalleryInput {
    pub path: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub file_ids: Vec<i32>,
}

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<gallery::Model>>> {
    Ok(Json(galleries_repo::list_all(&state.db).await?))
}

pub async fn list_paths(State(state): State<AppState>) -> ApiResult<Json<Vec<String>>> {
    Ok(Json(galleries_repo::list_paths(&state.db).await?))
}

pub async fn read(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<Json<gallery::Model>> {
    galleries_repo::find_by_id(&state.db, id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Json(input): Json<GalleryInput>,
) -> ApiResult<(StatusCode, Json<gallery::Model>)> {
    let saved = galleries_repo::create_gallery(
        &state.db,
        user_id,
        RepoGalleryInput {
            path: input.path,
            title: input.title,
            description: input.description,
            file_ids: input.file_ids,
        },
    )
    .await?;
    broadcast::gallery_created(&state.ws_hub, &saved);
    Ok((StatusCode::CREATED, Json(saved)))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(input): Json<GalleryInput>,
) -> ApiResult<Json<gallery::Model>> {
    let updated = galleries_repo::update_gallery(
        &state.db,
        id,
        RepoGalleryInput {
            path: input.path,
            title: input.title,
            description: input.description,
            file_ids: input.file_ids,
        },
    )
    .await?
    .ok_or(ApiError::NotFound)?;
    broadcast::gallery_updated(&state.ws_hub, &updated);
    Ok(Json(updated))
}

pub async fn delete_one(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<StatusCode> {
    if galleries_repo::delete_by_id(&state.db, id).await? {
        broadcast::gallery_deleted(&state.ws_hub, id);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}
