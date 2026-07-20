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
use axum_extra::extract::CookieJar;

use crate::export::{self, ExportFormat};
use crate::path_util;
use crate::state::AppState;
use crate::{auth, routes::public};

pub fn router() -> Router<AppState> {
    Router::new().route("/{*path}", get(handle))
}

#[derive(serde::Deserialize)]
struct ExportQuery {
    format: Option<String>,
}

async fn handle(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(q): Query<ExportQuery>,
    req: Request,
) -> Response {
    let Some(raw_format) = q.format else {
        return public::catch_all(State(state), jar, req)
            .await
            .into_response();
    };

    let Some(format) = ExportFormat::parse(&raw_format) else {
        return (
            StatusCode::BAD_REQUEST,
            format!("unknown export format `{raw_format}`"),
        )
            .into_response();
    };

    if format.requires_pandoc() && !state.pandoc_available {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "pandoc is not installed on this server; reveal.js-slides export is unavailable",
        )
            .into_response();
    }

    let path = path_util::normalize(req.uri().path());
    let logged_in = auth::is_logged_in(&state, &jar).await.is_some();

    let Some(content) = public::lookup_content(&state.db, &path).await else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    if content.private() && !logged_in {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    let env = state.tmpl.env();
    let artifact = match export::render_page(
        &state.db,
        &state.design,
        &env,
        content.markdown(),
        Some(content.title()),
        logged_in,
        format,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("export render failed for `{path}`: {e:#}");
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
        artifact.primary.to_vec(),
    )
        .into_response()
}
