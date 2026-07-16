use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Extension, Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;

use crate::repo::files::{self as files_repo, FileMetaUpdate, FileWithThumb, NewFile};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::routes::ws::Topic;
use crate::state::AppState;

pub const MAX_UPLOAD_SIZE: usize = 50 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(upload))
        .route("/{id}", get(read).put(update).delete(delete_one))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_SIZE))
}

#[derive(serde::Serialize)]
pub struct FileSummary {
    pub id: i32,
    pub hash: String,
    pub path: String,
    pub title: String,
    pub description: Option<String>,
    pub mimetype: String,
    pub size_bytes: i64,
    pub has_thumbnail: bool,
    pub created_at: String,
}

impl From<FileWithThumb> for FileSummary {
    fn from(f: FileWithThumb) -> Self {
        Self {
            id: f.model.id,
            hash: f.model.hash,
            title: files_repo::title_from_path(&f.model.path),
            path: f.model.path,
            description: f.model.description,
            mimetype: f.model.mimetype,
            size_bytes: f.model.size_bytes,
            has_thumbnail: f.has_thumbnail,
            created_at: f.model.created_at.to_string(),
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
    Ok(Json(rows.into_iter().map(FileSummary::from).collect()))
}

pub async fn read(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<Json<FileSummary>> {
    let f = files_repo::find_with_thumbnail(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(FileSummary::from(f)))
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
    if data.is_empty() {
        return Err(ApiError::BadRequest("uploaded file is empty".into()));
    }
    let path = path
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing path field".into()))?;
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
    let m = created.model;
    let summary = FileSummary {
        id: m.id,
        hash: m.hash,
        title: files_repo::title_from_path(&m.path),
        path: m.path,
        description: m.description,
        mimetype: m.mimetype,
        size_bytes: m.size_bytes,
        has_thumbnail: created.has_thumbnail,
        created_at: m.created_at.to_string(),
    };
    state
        .ws_hub
        .broadcast_serialized(Topic::Files, "created", &summary);
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
    let summary = FileSummary::from(updated);
    state
        .ws_hub
        .broadcast_serialized(Topic::Files, "updated", &summary);
    Ok(Json(summary))
}

pub async fn delete_one(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<StatusCode> {
    if files_repo::delete_by_id(&state.db, id).await? {
        state
            .ws_hub
            .broadcast_event(Topic::Files, "deleted", serde_json::json!({ "id": id }));
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}
