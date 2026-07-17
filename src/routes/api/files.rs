use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Extension, Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;

use crate::repo::files::{self as files_repo, FileMetaUpdate, FileSaveError, NewFile};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::routes::broadcast::{self, FileSummary};
use crate::state::AppState;

pub const MAX_UPLOAD_SIZE: usize = 50 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(upload))
        .route("/{id}", get(read).put(update).delete(delete_one))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_SIZE))
}

impl From<FileSaveError> for ApiError {
    fn from(e: FileSaveError) -> Self {
        match e {
            FileSaveError::EmptyPath => ApiError::BadRequest("path is required".into()),
            FileSaveError::EmptyData => ApiError::BadRequest("uploaded file is empty".into()),
            FileSaveError::Db(db) => ApiError::from(db),
        }
    }
}

#[derive(serde::Deserialize)]
pub struct ListQuery {
    pub mime_prefix: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct FileMetaInput {
    pub path: String,
    #[serde(default)]
    pub description: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<FileSummary>>> {
    let rows = files_repo::list_with_thumbnails(&state.db, query.mime_prefix.as_deref()).await?;
    Ok(Json(
        rows.into_iter()
            .map(|f| FileSummary::new(&f.model, f.has_thumbnail))
            .collect(),
    ))
}

pub async fn read(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<Json<FileSummary>> {
    let f = files_repo::find_with_thumbnail(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(FileSummary::new(&f.model, f.has_thumbnail)))
}

pub async fn upload(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    mut multipart: Multipart,
) -> ApiResult<(StatusCode, Json<FileSummary>)> {
    let mut path: Option<String> = None;
    let mut description: Option<String> = None;
    let mut data: Option<Vec<u8>> = None;
    let mut mimetype: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "path" => {
                path = Some(field.text().await.unwrap_or_default());
            }
            "description" => {
                let v = field.text().await.unwrap_or_default();
                description = if v.is_empty() { None } else { Some(v) };
            }
            "file" => {
                let ct = field
                    .content_type()
                    .map(|s| s.to_string())
                    .or_else(|| {
                        field
                            .file_name()
                            .and_then(|n| mime_guess::from_path(n).first().map(|m| m.to_string()))
                    })
                    .unwrap_or_else(|| "application/octet-stream".to_string());
                mimetype = Some(ct);
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("read error: {e}")))?;
                data = Some(bytes.to_vec());
            }
            _ => {}
        }
    }

    let data = data.ok_or_else(|| ApiError::BadRequest("missing file field".into()))?;
    let path = path.ok_or_else(|| ApiError::BadRequest("missing path field".into()))?;
    let mimetype = mimetype.unwrap_or_else(|| "application/octet-stream".to_string());

    let created = files_repo::create_file(
        &state.db,
        user_id,
        NewFile {
            path,
            description,
            mimetype,
            data,
        },
    )
    .await?;
    let summary = broadcast::file_created(&state.ws_hub, &created.model, created.has_thumbnail);
    Ok((StatusCode::CREATED, Json(summary)))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(input): Json<FileMetaInput>,
) -> ApiResult<Json<FileSummary>> {
    let updated = files_repo::update_metadata(
        &state.db,
        id,
        FileMetaUpdate {
            path: input.path,
            description: input.description,
        },
    )
    .await?
    .ok_or(ApiError::NotFound)?;
    let summary = broadcast::file_updated(&state.ws_hub, &updated.model, updated.has_thumbnail);
    Ok(Json(summary))
}

pub async fn delete_one(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<StatusCode> {
    if files_repo::delete_by_id(&state.db, id).await? {
        broadcast::file_deleted(&state.ws_hub, id);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}
