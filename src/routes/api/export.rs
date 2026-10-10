//! Admin/API export route (#67): `GET /api/export/pages/{id}?format=pdf|slides`.
//! Gated by `require_login_api` (nested under the `protected` router in
//! `src/routes/api/mod.rs`), so unlike the public route it needs no separate
//! privacy check — any logged-in user can export any page.

use axum::Router;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum_extra::extract::CookieJar;
use sea_orm::EntityTrait;

use crate::entity::page;
use crate::export::{self, ExportFormat};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::routes::public::preview::Look;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/pages/{id}", get(export_page))
}

#[derive(serde::Deserialize)]
pub struct ExportQuery {
    pub format: String,
}

/// `Extension<i32>` is the session `require_login_api` already checked.
async fn export_page(
    State(state): State<AppState>,
    Extension(_user): Extension<i32>,
    jar: CookieJar,
    Path(id): Path<i32>,
    Query(q): Query<ExportQuery>,
) -> ApiResult<Response> {
    // In draft preview the export uses the draft's mdcast templates and brand.
    let look = Look::logged_in(&state, &jar).await?;
    // Every answer, errors included, is marked when it is preview output.
    let resp = export_by_id(&state, &look, id, &q.format).await;
    Ok(look.finish(resp.into_response()))
}

async fn export_by_id(state: &AppState, look: &Look, id: i32, raw: &str) -> ApiResult<Response> {
    let format = ExportFormat::parse(raw)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown export format `{raw}`")))?;

    let Some(client) = &state.mdcast else {
        return Err(ApiError::ServiceUnavailable(
            "export render server is not configured; export is unavailable".into(),
        ));
    };

    let pg = page::Entity::find_by_id(id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    let env = look.env(state);
    let artifact = export::render_page(
        client,
        &state.db,
        &state.storage,
        look.design(state),
        &env,
        &pg.markdown,
        Some(pg.path.clone()),
        true,
        format,
    )
    .await
    .map_err(|e| match e {
        export::ExportError::Unavailable(msg) => {
            tracing::warn!("export render server unavailable for page {id}: {msg}");
            ApiError::ServiceUnavailable(
                "export render server is unavailable; try again later".into(),
            )
        }
        e => {
            tracing::error!("export render failed for page {id}: {e}");
            ApiError::Internal("export failed".to_string())
        }
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
        artifact.bytes.to_vec(),
    )
        .into_response())
}
