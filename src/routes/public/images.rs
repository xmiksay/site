use axum::Router;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sea_orm::EntityTrait;

use crate::entity::file_thumbnail;
use crate::files::read_blob;
use crate::repo::files as files_repo;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{hash}", get(serve))
        .route("/{hash}/nahled", get(serve_thumbnail))
}

pub async fn serve(State(state): State<AppState>, Path(hash): Path<String>) -> Response {
    let Ok(Some(f)) = files_repo::find_by_hash(&state.db, &hash).await else {
        return (axum::http::StatusCode::NOT_FOUND, "Not found").into_response();
    };
    match read_blob(&state.db, &f.hash).await {
        Ok(Some(data)) => (
            [
                (header::CONTENT_TYPE, f.mimetype.clone()),
                (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
            ],
            data,
        )
            .into_response(),
        _ => (axum::http::StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

pub async fn serve_thumbnail(State(state): State<AppState>, Path(hash): Path<String>) -> Response {
    let Ok(Some(f)) = files_repo::find_by_hash(&state.db, &hash).await else {
        return (axum::http::StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let Ok(Some(thumb)) = file_thumbnail::Entity::find_by_id(f.id)
        .one(&state.db)
        .await
    else {
        return (axum::http::StatusCode::NOT_FOUND, "Not found").into_response();
    };
    match read_blob(&state.db, &thumb.hash).await {
        Ok(Some(data)) => (
            [
                (header::CONTENT_TYPE, thumb.mimetype.clone()),
                (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
            ],
            data,
        )
            .into_response(),
        _ => (axum::http::StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}
