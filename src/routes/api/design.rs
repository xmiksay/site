//! Admin design API: the shared draft (tree + changes, raw file read/write/
//! delete, discard), publishing it, the version history and restoring a
//! version into the draft, and reloading `design/` after edits made directly
//! in the bucket. See `design::{draft, publish, stored}`. Every draft
//! mutation broadcasts `design.draft_changed`.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};

use crate::design::draft::FileChange;
use crate::design::publish::HistoryEntry;
use crate::design::stored::{DesignError, ReloadStatus, check_path, status_error};
use crate::entity::user;
use crate::routes::api::error::{ApiError, ApiResult};
use crate::routes::broadcast::{self, DraftChange};
use crate::state::AppState;
use crate::storage;

/// Fonts and images are the largest files; templates are tiny.
const MAX_DESIGN_FILE_SIZE: usize = 20 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/reload", post(reload))
        .route("/draft", get(draft))
        .route("/draft/discard", post(discard))
        .route(
            "/draft/{*path}",
            get(read_file).put(put_file).delete(delete_file),
        )
        .route("/publish", post(publish))
        .route("/history", get(history))
        .route("/history/{id}/restore", post(restore))
        .layer(DefaultBodyLimit::max(MAX_DESIGN_FILE_SIZE))
}

/// The draft as the admin sees it.
#[derive(Serialize)]
pub struct DraftState {
    storage: &'static str,
    local_dir: bool,
    last_reload: Option<ReloadStatus>,
    /// The draft view (draft over baked), sorted by path.
    files: Vec<DesignFile>,
    /// What publishing would change.
    changes: Vec<FileChange>,
}

#[derive(Serialize)]
struct DesignFile {
    path: String,
    baked: bool,
    /// Differs from the baked default (or has none).
    overridden: bool,
    size: u64,
}

impl From<DesignError> for ApiError {
    fn from(err: DesignError) -> Self {
        match err {
            DesignError::Storage(e) => e.into(),
            DesignError::BadPath(_) => Self::BadRequest(err.to_string()),
            DesignError::NotInDraft(_) | DesignError::NoVersion(_) => Self::NotFound,
            DesignError::Invalid(_) => Self::Unprocessable(err.to_string()),
            DesignError::PublishFailed { ref error, .. } => {
                let msg = status_error(&err);
                match **error {
                    DesignError::Storage(storage::Error::Unavailable(_)) => {
                        Self::ServiceUnavailable(msg)
                    }
                    _ => Self::Internal(msg),
                }
            }
        }
    }
}

async fn draft_state(state: &AppState) -> ApiResult<Json<DraftState>> {
    let design = &state.design;
    let draft = design.draft(&state.storage).await?;
    let files = design
        .with_baked(&draft.files)
        .into_iter()
        .map(|(path, bytes)| {
            let baked = design.baked(&path);
            DesignFile {
                baked: baked.is_some(),
                overridden: baked.as_deref() != Some(bytes.as_ref()),
                size: bytes.len() as u64,
                path,
            }
        })
        .collect();
    Ok(Json(DraftState {
        storage: state.storage.kind(),
        local_dir: design.has_local_dir(),
        last_reload: design.last_reload(),
        files,
        changes: draft.changes,
    }))
}

async fn draft(State(state): State<AppState>) -> ApiResult<Json<DraftState>> {
    draft_state(&state).await
}

async fn reload(State(state): State<AppState>) -> ApiResult<Json<DraftState>> {
    state.design.reload(&state.storage, &state.tmpl).await?;
    draft_state(&state).await
}

#[derive(Deserialize)]
struct ReadQuery {
    source: Option<String>,
}

async fn read_file(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Query(query): Query<ReadQuery>,
) -> ApiResult<Response> {
    check_path(&path)?;
    let design = &state.design;
    let data = match query.source.as_deref() {
        None | Some("draft") => design.draft_read(&state.storage, &path).await?,
        Some("published") => design.published_view().remove(&path),
        Some("baked") => design.baked(&path).map(Bytes::from),
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "source must be draft, published or baked, not {other:?}"
            )));
        }
    }
    .ok_or(ApiError::NotFound)?;
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    Ok(([(header::CONTENT_TYPE, mime.to_string())], data).into_response())
}

async fn put_file(
    State(state): State<AppState>,
    Path(path): Path<String>,
    body: Bytes,
) -> ApiResult<Json<DraftState>> {
    state.design.draft_put(&state.storage, &path, body).await?;
    broadcast::design_draft_changed(&state.ws_hub, &DraftChange::Put { path: &path });
    draft_state(&state).await
}

async fn delete_file(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> ApiResult<Json<DraftState>> {
    state.design.draft_delete(&state.storage, &path).await?;
    broadcast::design_draft_changed(&state.ws_hub, &DraftChange::Delete { path: &path });
    draft_state(&state).await
}

async fn discard(State(state): State<AppState>) -> ApiResult<Json<DraftState>> {
    state.design.draft_discard(&state.storage).await?;
    broadcast::design_draft_changed(&state.ws_hub, &DraftChange::Discard);
    draft_state(&state).await
}

async fn publish(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
) -> ApiResult<Json<HistoryEntry>> {
    let by = user::Entity::find_by_id(user_id)
        .one(&state.db)
        .await?
        .map_or_else(|| format!("user #{user_id}"), |u| u.username);
    let entry = state
        .design
        .publish(&state.storage, &state.tmpl, &by)
        .await?;
    broadcast::design_published(&state.ws_hub, &entry);
    Ok(Json(entry))
}

async fn history(State(state): State<AppState>) -> ApiResult<Json<Vec<HistoryEntry>>> {
    Ok(Json(state.design.history(&state.storage).await?))
}

async fn restore(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<DraftState>> {
    state.design.restore(&state.storage, &id).await?;
    broadcast::design_draft_changed(&state.ws_hub, &DraftChange::Restore { version: &id });
    draft_state(&state).await
}
