//! Admin Design manager: the merged tree of baked files and storage
//! overrides, reading/writing/deleting overrides, and reloading them after
//! edits made directly in the bucket. See `design::stored`.

use std::collections::BTreeMap;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use crate::design::stored::{Change, DesignError, ROOTS, ReloadStatus, check_path};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Fonts and images are the largest overrides; templates are tiny.
const MAX_DESIGN_FILE_SIZE: usize = 20 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(state))
        .route("/reload", post(reload))
        .route(
            "/files/{*path}",
            get(read_file).put(put_file).delete(delete_file),
        )
        .layer(DefaultBodyLimit::max(MAX_DESIGN_FILE_SIZE))
}

#[derive(Serialize)]
pub struct DesignState {
    storage: &'static str,
    editable: bool,
    local_dir: bool,
    last_reload: Option<ReloadStatus>,
    files: Vec<DesignFile>,
}

#[derive(Serialize, Default)]
struct DesignFile {
    path: String,
    baked: bool,
    overridden: bool,
    size: u64,
}

impl From<DesignError> for ApiError {
    fn from(err: DesignError) -> Self {
        match err {
            DesignError::Storage(e) => e.into(),
            DesignError::BadPath(_) => Self::BadRequest(err.to_string()),
            DesignError::NoOverride(_) => Self::NotFound,
            DesignError::Invalid(_) => Self::Unprocessable(err.to_string()),
        }
    }
}

fn design_state(state: &AppState) -> DesignState {
    let design = &state.design;
    let mut files: BTreeMap<String, DesignFile> = BTreeMap::new();
    for root in ROOTS {
        for path in design.baked_paths(root) {
            let size = design.baked(&path).map_or(0, |b| b.len() as u64);
            let entry = DesignFile {
                path: path.clone(),
                baked: true,
                size,
                ..Default::default()
            };
            files.insert(path, entry);
        }
    }
    for (path, size) in design.stored_paths() {
        let entry = files.entry(path.clone()).or_insert_with(|| DesignFile {
            path,
            ..Default::default()
        });
        entry.overridden = true;
        entry.size = size;
    }
    DesignState {
        storage: state.storage.kind(),
        editable: state.storage.has_objects(),
        local_dir: design.has_local_dir(),
        last_reload: design.last_reload(),
        files: files.into_values().collect(),
    }
}

async fn state(State(state): State<AppState>) -> Json<DesignState> {
    Json(design_state(&state))
}

async fn reload(State(state): State<AppState>) -> ApiResult<Json<DesignState>> {
    apply(&state, None).await
}

async fn apply(state: &AppState, change: Option<Change>) -> ApiResult<Json<DesignState>> {
    state
        .design
        .apply(&state.storage, &state.tmpl, change)
        .await?;
    Ok(Json(design_state(state)))
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
    let data = match query.source.as_deref() {
        Some("baked") => state.design.baked(&path),
        None | Some("effective") => state.design.load(&path),
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "source must be baked or effective, not {other:?}"
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
) -> ApiResult<Json<DesignState>> {
    apply(&state, Some(Change::Put { path, bytes: body })).await
}

async fn delete_file(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> ApiResult<Json<DesignState>> {
    apply(&state, Some(Change::Delete { path })).await
}
