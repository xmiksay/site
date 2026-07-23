use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rust_embed::Embed;
use site::config::Config;
use site::design::build_static_response;
use site::state::AppState;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Embed)]
#[folder = "client/dist"]
struct AdminAssets;

#[tokio::main]
async fn main() {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("site=debug,tower_http=debug,info")),
        )
        .init();

    let config = Config::from_env();
    // Migrations run inside create_state, before the assistant engine reads
    // the schema.
    let state = site::state::create_state(&config).await;

    use site::routes::{api, mcp, oauth, public};

    let app = Router::new()
        .merge(mcp::router())
        .merge(oauth::router())
        .nest("/files", public::images::router())
        .nest("/tag", public::tags::router())
        .nest("/search", public::search::router())
        .merge(public::sitemap::router())
        .merge(public::export::router())
        .nest("/api", api::router(state.clone()))
        .route("/admin", get(admin_index))
        .route("/admin/{*path}", get(admin_static))
        .route("/assets/{*path}", get(serve_static))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .fallback(get(public::catch_all))
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("http://localhost:{port}");
    axum::serve(listener, app).await.unwrap();
}

async fn admin_index() -> Response {
    serve_admin_asset("index.html")
}

async fn admin_static(Path(path): Path<String>) -> Response {
    if let Some(resp) = AdminAssets::get(&path).map(|file| build_admin_asset_response(&path, file))
    {
        return resp;
    }
    serve_admin_asset("index.html")
}

fn serve_admin_asset(path: &str) -> Response {
    match AdminAssets::get(path) {
        Some(file) => build_admin_asset_response(path, file),
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

fn build_admin_asset_response(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime.as_ref().to_string())],
        file.data.to_vec(),
    )
        .into_response()
}

/// Serve a runtime static resource from the design bundle's `assets/` folder
/// (override → baked). The `/assets/<path>` route maps to `assets/<path>` within
/// the bundle, alongside the template-engine-owned `templates/` folder.
async fn serve_static(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    let key = format!("assets/{path}");
    match state.design.load(&key) {
        Some(data) => build_static_response(&key, data),
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}
