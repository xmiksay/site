pub mod auth;
pub mod error;
pub mod export;
pub mod files;
pub mod galleries;
pub mod markdown;
pub mod menu;
pub mod pages;
pub mod paths;
pub mod tags;
pub mod tokens;
pub mod users;

use axum::Router;
use axum::middleware::from_fn_with_state;

use crate::state::AppState;

pub fn router(state: AppState) -> Router<AppState> {
    let protected = Router::new()
        .nest("/pages", pages::router())
        .nest("/tags", tags::router())
        .nest("/files", files::router())
        .nest("/galleries", galleries::router())
        .nest("/markdown", markdown::router())
        .nest("/menu", menu::router())
        .nest("/paths", paths::router())
        .nest("/tokens", tokens::router())
        .nest("/users", users::router())
        .nest("/export", export::router())
        .nest("/assistant", crate::ai::handlers::router())
        .nest("/ws", crate::routes::ws::router())
        .route_layer(from_fn_with_state(state, crate::auth::require_login_api));

    Router::new().nest("/auth", auth::router()).merge(protected)
}
