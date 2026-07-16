use axum::Json;
use axum::Router;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};

use crate::entity::page;
use crate::repo::pages::{self as pages_repo, PageNew, PageSort};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::routes::ws::Topic;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/paths", get(list_paths))
        .route("/{id}", get(read).put(update).delete(delete_one))
        .route("/{id}/revisions/{rev_id}", get(read_revision))
        .route("/{id}/revisions/{rev_id}/restore", post(restore))
}

#[derive(serde::Serialize)]
pub struct PageSummary {
    pub id: i32,
    pub path: String,
    pub summary: Option<String>,
    pub tag_ids: Vec<i32>,
    pub private: bool,
    pub created_at: String,
    pub modified_at: String,
}

impl From<&page::Model> for PageSummary {
    fn from(p: &page::Model) -> Self {
        Self {
            id: p.id,
            path: p.path.clone(),
            summary: p.summary.clone(),
            tag_ids: p.tag_ids.clone(),
            private: p.private,
            created_at: p.created_at.to_string(),
            modified_at: p.modified_at.to_string(),
        }
    }
}

#[derive(serde::Serialize)]
pub struct PageDetail {
    #[serde(flatten)]
    pub summary: PageSummary,
    pub markdown: String,
    pub revisions: Vec<RevisionSummary>,
}

#[derive(serde::Serialize)]
pub struct RevisionSummary {
    pub id: i32,
    pub created_at: String,
}

#[derive(serde::Serialize)]
pub struct RevisionDetail {
    pub id: i32,
    pub created_at: String,
    /// Reconstructed markdown at this revision.
    pub markdown: String,
    /// Unified diff from this revision (old) to the current page (new).
    pub diff: String,
}

#[derive(serde::Deserialize)]
pub struct ListQuery {
    pub sort: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct PathsQuery {
    pub prefix: Option<String>,
    pub limit: Option<u64>,
}

#[derive(serde::Deserialize)]
pub struct PageInput {
    pub path: String,
    #[serde(default)]
    pub summary: Option<String>,
    pub markdown: String,
    #[serde(default)]
    pub tag_ids: Vec<i32>,
    #[serde(default)]
    pub private: bool,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<PageSummary>>> {
    let pages = pages_repo::list_all(&state.db, PageSort::parse(query.sort.as_deref())).await?;
    Ok(Json(pages.iter().map(PageSummary::from).collect()))
}

pub async fn list_paths(
    State(state): State<AppState>,
    Query(query): Query<PathsQuery>,
) -> ApiResult<Json<Vec<String>>> {
    let paths = pages_repo::list_paths(&state.db, query.prefix.as_deref(), query.limit).await?;
    Ok(Json(paths))
}

pub async fn read(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<Json<PageDetail>> {
    use sea_orm::EntityTrait;
    let pg = page::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    let revisions: Vec<RevisionSummary> = pages_repo::list_revisions(&state.db, id)
        .await?
        .iter()
        .map(|r| RevisionSummary {
            id: r.id,
            created_at: r.created_at.to_string(),
        })
        .collect();

    Ok(Json(PageDetail {
        markdown: pg.markdown.clone(),
        summary: PageSummary::from(&pg),
        revisions,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Json(input): Json<PageInput>,
) -> ApiResult<(StatusCode, Json<PageSummary>)> {
    if input.path.is_empty() {
        return Err(ApiError::BadRequest("path is required".into()));
    }
    let saved = pages_repo::create(
        &state.db,
        user_id,
        PageNew {
            path: input.path,
            markdown: input.markdown,
            summary: input.summary,
            tag_ids: input.tag_ids,
            private: input.private,
        },
    )
    .await
    .map_err(|e| match e {
        sea_orm::DbErr::Exec(_) | sea_orm::DbErr::Query(_) => {
            ApiError::Conflict(format!("path already exists: {e}"))
        }
        other => ApiError::from(other),
    })?;
    let summary = PageSummary::from(&saved);
    state
        .ws_hub
        .broadcast_serialized(Topic::Pages, "created", &summary);
    Ok((StatusCode::CREATED, Json(summary)))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path(id): Path<i32>,
    Json(input): Json<PageInput>,
) -> ApiResult<Json<PageSummary>> {
    let updated = pages_repo::replace(
        &state.db,
        user_id,
        id,
        PageNew {
            path: input.path,
            markdown: input.markdown,
            summary: input.summary,
            tag_ids: input.tag_ids,
            private: input.private,
        },
    )
    .await?
    .ok_or(ApiError::NotFound)?;
    let summary = PageSummary::from(&updated);
    state
        .ws_hub
        .broadcast_serialized(Topic::Pages, "updated", &summary);
    Ok(Json(summary))
}

pub async fn delete_one(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> ApiResult<StatusCode> {
    if pages_repo::delete_by_id(&state.db, id).await? {
        state
            .ws_hub
            .broadcast_event(Topic::Pages, "deleted", serde_json::json!({ "id": id }));
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

pub async fn read_revision(
    State(state): State<AppState>,
    Path((id, rev_id)): Path<(i32, i32)>,
) -> ApiResult<Json<RevisionDetail>> {
    use sea_orm::EntityTrait;
    let pg = page::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    let rev = pages_repo::get_revision(&state.db, id, rev_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let revision_markdown =
        crate::routes::revision::reconstruct_at_revision(&state.db, id, rev_id, &pg.markdown)
            .await
            .map_err(|e| match e {
                crate::routes::revision::ReconstructError::NotFound => ApiError::NotFound,
                other => ApiError::Internal(other.to_string()),
            })?;

    let diff = diffy::create_patch(&revision_markdown, &pg.markdown).to_string();

    Ok(Json(RevisionDetail {
        id: rev.id,
        created_at: rev.created_at.to_string(),
        markdown: revision_markdown,
        diff,
    }))
}

pub async fn restore(
    State(state): State<AppState>,
    Extension(user_id): Extension<i32>,
    Path((id, rev_id)): Path<(i32, i32)>,
) -> ApiResult<Json<PageSummary>> {
    let updated = pages_repo::restore_revision(&state.db, user_id, id, rev_id)
        .await
        .map_err(|e| match e {
            pages_repo::RestoreError::NotFound => ApiError::NotFound,
            pages_repo::RestoreError::Db(db) => ApiError::from(db),
            other => ApiError::Internal(other.to_string()),
        })?;
    Ok(Json(PageSummary::from(&updated)))
}
