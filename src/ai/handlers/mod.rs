pub mod mcp_servers;
pub mod models;
pub mod permissions;
pub mod providers;
pub mod sessions;

use axum::Router;
use axum::routing::{get, patch, post};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sessions", get(sessions::list).post(sessions::create))
        .route(
            "/sessions/{id}",
            get(sessions::read)
                .patch(sessions::update)
                .delete(sessions::delete_one),
        )
        .route("/sessions/{id}/messages", post(sessions::send_message))
        .route(
            "/sessions/{id}/messages/{message_id}/approve",
            post(sessions::approve),
        )
        .route("/sessions/{id}/compact", post(sessions::compact))
        .route(
            "/mcp-servers",
            get(mcp_servers::list).post(mcp_servers::create),
        )
        .route(
            "/mcp-servers/{id}",
            patch(mcp_servers::update).delete(mcp_servers::delete_one),
        )
        .route("/providers", get(providers::list).post(providers::create))
        .route(
            "/providers/{id}",
            get(providers::read)
                .patch(providers::update)
                .delete(providers::delete_one),
        )
        .route("/models", get(models::list).post(models::create))
        .route(
            "/models/{id}",
            patch(models::update).delete(models::delete_one),
        )
        .route(
            "/permissions",
            get(permissions::list).post(permissions::create),
        )
        .route(
            "/permissions/{id}",
            patch(permissions::update).delete(permissions::delete_one),
        )
}
