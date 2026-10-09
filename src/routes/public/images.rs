use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sea_orm::EntityTrait;

use crate::entity::file_thumbnail;
use crate::repo::files as files_repo;
use crate::state::AppState;
use crate::storage::Error;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{hash}", get(serve))
        .route("/{hash}/nahled", get(serve_thumbnail))
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not found").into_response()
}

pub async fn serve(State(state): State<AppState>, Path(hash): Path<String>) -> Response {
    let Ok(Some(f)) = files_repo::find_by_hash(&state.db, &hash).await else {
        return not_found();
    };
    stream_blob(&state, &f.hash, f.mimetype).await
}

pub async fn serve_thumbnail(State(state): State<AppState>, Path(hash): Path<String>) -> Response {
    let Ok(Some(f)) = files_repo::find_by_hash(&state.db, &hash).await else {
        return not_found();
    };
    let Ok(Some(thumb)) = file_thumbnail::Entity::find_by_id(f.id)
        .one(&state.db)
        .await
    else {
        return not_found();
    };
    stream_blob(&state, &thumb.hash, thumb.mimetype).await
}

/// A row pointing at a blob the backend lacks is 404 like any missing file;
/// an unreachable backend is 503 so caches and clients retry.
async fn stream_blob(state: &AppState, hash: &str, mimetype: String) -> Response {
    match state.storage.get_blob_stream(hash).await {
        Ok(Some(download)) => (
            [
                (header::CONTENT_TYPE, mimetype),
                (header::CONTENT_LENGTH, download.size.to_string()),
                (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
            ],
            Body::from_stream(download.stream),
        )
            .into_response(),
        Ok(None) => {
            tracing::warn!(hash, "file row points at a blob missing from storage");
            not_found()
        }
        Err(Error::Unavailable(e)) => {
            tracing::error!(hash, "storage unavailable: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "Storage unavailable").into_response()
        }
        Err(e) => {
            tracing::error!(hash, "reading blob failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
        }
    }
}
