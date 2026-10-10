//! Admin design API: the shared draft (tree + changes, raw file read/write/
//! delete, discard), publishing it, the version history and restoring a
//! version into the draft, and reloading `design/` after edits made directly
//! in the bucket. See `design::{draft, publish, stored}`. Every draft
//! mutation broadcasts `design.draft_changed`. Also toggles draft preview
//! mode for this browser (`routes::public::preview`).

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};

use crate::design::draft::FileChange;
use crate::design::publish::HistoryEntry;
use crate::design::stored::{DesignError, ReloadStatus, status_error};
use crate::design::tools::{DesignFile, MAX_FILE_SIZE, Source, file_entries, read_source};
use crate::entity::user;
use crate::routes::api::error::{ApiError, ApiResult};
use crate::routes::broadcast::{self, DraftChange};
use crate::routes::public::preview::{EXIT_PATH, PREVIEW_COOKIE};
use crate::state::AppState;
use crate::storage;

/// Session-only: publishing, discarding, the history, reloads and the
/// draft preview are human actions in the admin.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/reload", post(reload))
        .route("/draft/discard", post(discard))
        .route("/publish", post(publish))
        .route("/history", get(history))
        .route("/history/{id}/restore", post(restore))
        .route("/preview", post(set_preview))
        .route("/preview/exit", get(exit_preview))
}

/// The draft tree and its files, also open to external agents with the MCP
/// Bearer token (#118) — raw bodies, so `curl -T font.woff2` works.
pub fn draft_router() -> Router<AppState> {
    Router::new()
        .route("/draft", get(draft))
        .route(
            "/draft/{*path}",
            get(read_file).put(put_file).delete(delete_file),
        )
        .layer(DefaultBodyLimit::max(MAX_FILE_SIZE))
}

/// The draft as the admin sees it.
#[derive(Serialize)]
pub struct DraftState {
    storage: &'static str,
    local_dir: bool,
    last_reload: Option<ReloadStatus>,
    /// False until the draft's first mutation (it then shows the published
    /// view).
    initialized: bool,
    /// The draft view (draft over baked), sorted by path.
    files: Vec<DesignFile>,
    /// What publishing would change.
    changes: Vec<FileChange>,
}

impl From<DesignError> for ApiError {
    fn from(err: DesignError) -> Self {
        match err {
            DesignError::Storage(e) => e.into(),
            DesignError::BadPath(_) => Self::BadRequest(err.to_string()),
            DesignError::NotInDraft(_) | DesignError::NoVersion(_) => Self::NotFound,
            DesignError::Invalid(ref errors) => Self::Detailed {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                message: err.to_string(),
                code: "invalid",
                details: errors.clone(),
            },
            DesignError::Conflict(ref paths) => Self::Detailed {
                status: StatusCode::CONFLICT,
                message: err.to_string(),
                code: "conflict",
                details: paths.clone(),
            },
            DesignError::NothingToPublish => Self::Detailed {
                status: StatusCode::CONFLICT,
                message: err.to_string(),
                code: "nothing_to_publish",
                details: Vec::new(),
            },
            DesignError::RenderCheck => Self::Internal(err.to_string()),
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
    let files = file_entries(design, &design.with_baked(&draft.files));
    Ok(Json(DraftState {
        storage: state.storage.kind(),
        local_dir: design.has_local_dir(),
        last_reload: design.last_reload(),
        initialized: draft.initialized,
        files,
        changes: draft.changes,
    }))
}

async fn draft(State(state): State<AppState>) -> ApiResult<Json<DraftState>> {
    draft_state(&state).await
}

/// The answer's `last_reload.completed_publish` names a pending publish
/// this reload completed first.
async fn reload(State(state): State<AppState>) -> ApiResult<Json<DraftState>> {
    let status = state.design.reload(&state.storage, &state.tmpl).await?;
    if let Some(entry) = &status.completed_publish {
        broadcast::design_published(&state.ws_hub, entry);
    }
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
    let source = Source::parse(query.source.as_deref()).map_err(ApiError::BadRequest)?;
    let data = read_source(&state.design, &state.storage, &path, source)
        .await?
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

#[derive(Deserialize)]
struct PublishQuery {
    #[serde(default)]
    force: bool,
}

async fn publish(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Query(query): Query<PublishQuery>,
) -> ApiResult<Json<HistoryEntry>> {
    let by = user::Entity::find_by_id(user_id)
        .one(&state.db)
        .await?
        .map_or_else(|| format!("user #{user_id}"), |u| u.username);
    let entry = state
        .design
        .publish(&state.db, &state.storage, &state.tmpl, &by, query.force)
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

#[derive(Deserialize, Serialize)]
struct Preview {
    on: bool,
}

fn with_preview(jar: CookieJar, on: bool) -> CookieJar {
    if on {
        let cookie = Cookie::build((PREVIEW_COOKIE, "1"))
            .http_only(true)
            .path("/")
            .same_site(SameSite::Lax)
            .build();
        jar.add(cookie)
    } else {
        // `jar.remove` only answers a removal for a cookie this request
        // sent; switching off must clear it regardless.
        let mut removal = Cookie::build(PREVIEW_COOKIE).path("/").build();
        removal.make_removal();
        jar.add(removal)
    }
}

/// Turn draft preview on or off for this browser.
async fn set_preview(jar: CookieJar, Json(body): Json<Preview>) -> (CookieJar, Json<Preview>) {
    (with_preview(jar, body.on), Json(body))
}

/// The preview banner's exit link: preview off, back to the page it was
/// clicked on.
async fn exit_preview(jar: CookieJar, headers: HeaderMap) -> (CookieJar, Redirect) {
    let referer = headers.get(header::REFERER).and_then(|v| v.to_str().ok());
    (with_preview(jar, false), Redirect::to(&back_to(referer)))
}

/// The referer's path and query on this site; only the path is kept, so a
/// foreign referer cannot turn this into an open redirect (nor can `//` or
/// a `\`, which browsers read as `/`).
fn back_to(referer: Option<&str>) -> String {
    referer
        .and_then(|r| r.parse::<Uri>().ok())
        .and_then(|uri| uri.path_and_query().map(|pq| pq.as_str().to_owned()))
        .filter(|p| {
            p.starts_with('/')
                && !p.starts_with("//")
                && !p.contains('\\')
                && !p.starts_with(EXIT_PATH)
        })
        .unwrap_or_else(|| "/".to_owned())
}

#[cfg(test)]
mod tests {
    use super::back_to;

    #[test]
    fn exit_goes_back_to_the_referring_path_only() {
        assert_eq!(back_to(Some("https://site.example/a/b?x=1")), "/a/b?x=1");
        assert_eq!(back_to(Some("https://evil.example/phish")), "/phish");
        assert_eq!(back_to(Some("/api/design/preview/exit")), "/");
        assert_eq!(back_to(Some("not a uri")), "/");
        assert_eq!(back_to(None), "/");
        assert_eq!(back_to(Some("/\\evil.example/x")), "/");
        assert_eq!(back_to(Some("https://site.example/a\\b")), "/");
    }
}
