//! Admin/API export route (#67): `GET /api/export/pages/{id}?format=pdf|slides`.
//! Gated by `require_login_api` (nested under the `protected` router in
//! `src/routes/api/mod.rs`), so unlike the public route it needs no separate
//! privacy check — any logged-in user can export any page.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sea_orm::EntityTrait;

use crate::entity::page;
use crate::export::{self, ExportFormat};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/pages/{id}", get(export_page))
}

#[derive(serde::Deserialize)]
pub struct ExportQuery {
    pub format: String,
}

async fn export_page(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Query(q): Query<ExportQuery>,
) -> ApiResult<Response> {
    let format = ExportFormat::parse(&q.format)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown export format `{}`", q.format)))?;

    if format.requires_pandoc() && !state.pandoc_available {
        return Err(ApiError::ServiceUnavailable(
            "pandoc is not installed on this server; reveal.js-slides export is unavailable".into(),
        ));
    }

    let pg = page::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    let env = state.tmpl.env();
    let artifact = export::render_page(
        &state.db,
        &state.design,
        &env,
        &pg.markdown,
        Some(pg.path.clone()),
        true,
        format,
    )
    .await
    .map_err(|e| {
        tracing::error!("export render failed for page {id}: {e:#}");
        ApiError::Internal("export failed".to_string())
    })?;

    let slug = export::sanitize_filename(
        pg.path
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or("export"),
    );
    let filename = format!("{slug}.{}", format.target().extension());

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, format.content_type().to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        artifact.primary.to_vec(),
    )
        .into_response())
}
