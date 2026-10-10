//! Public export route (#67): `GET /<path>?format=pdf|slides`. Registered
//! at the wildcard `/{*path}`, which makes it the effective fallback for
//! every non-root path in the router — a request with no `format` query
//! param passes straight through to `catch_all` unchanged, so plain page
//! viewing must never regress. `format=<...>` reuses `public::lookup_content`
//! for the exact same menu -> page -> 404 + privacy semantics `catch_all`
//! already implements, then renders through `export::render_page` (#67).

use axum::Router;
use axum::extract::{Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::export::{self, ExportFormat};
use crate::path_util;
use crate::routes::public::{self, preview::Look};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/{*path}", get(handle))
}

#[derive(serde::Deserialize)]
struct ExportQuery {
    format: Option<String>,
}

async fn handle(
    State(state): State<AppState>,
    look: Look,
    Query(q): Query<ExportQuery>,
    req: Request,
) -> Response {
    let Some(raw_format) = q.format else {
        return public::catch_all(State(state), look, req).await;
    };
    let path = path_util::normalize(req.uri().path());
    // Every answer, errors included, is marked when it is preview output.
    look.finish(export_path(&state, &look, &raw_format, &path).await)
}

async fn export_path(state: &AppState, look: &Look, raw_format: &str, path: &str) -> Response {
    let Some(format) = ExportFormat::parse(raw_format) else {
        return (
            StatusCode::BAD_REQUEST,
            format!("unknown export format `{raw_format}`"),
        )
            .into_response();
    };

    let Some(client) = &state.mdcast else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "export render server is not configured; export is unavailable",
        )
            .into_response();
    };

    let logged_in = look.logged_in;
    let Some(content) = public::lookup_content(&state.db, path).await else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    if content.private() && !logged_in {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    let env = look.env(state);
    let artifact = match export::render_page(
        client,
        &state.db,
        &state.storage,
        look.design(state),
        &env,
        content.markdown(),
        Some(content.title()),
        logged_in,
        format,
    )
    .await
    {
        Ok(a) => a,
        Err(export::ExportError::Unavailable(msg)) => {
            tracing::warn!("export render server unavailable for `{path}`: {msg}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "export render server is unavailable; try again later",
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!("export render failed for `{path}`: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "export failed").into_response();
        }
    };

    let slug =
        export::sanitize_filename(path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("export"));
    let filename = format!("{slug}.{}", format.target().extension());

    (
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
        .into_response()
}
