//! Well-known discovery endpoints and RFC 7591 dynamic client registration.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use sea_orm::{ActiveModelTrait, Set};
use serde::Deserialize;
use serde_json::json;

use crate::auth;
use crate::entity::oauth_client;
use crate::state::AppState;

use super::base_url;

// ---------------------------------------------------------------------------
// Well-known metadata endpoints
// ---------------------------------------------------------------------------

pub(super) async fn protected_resource_metadata(headers: HeaderMap) -> impl IntoResponse {
    let base = base_url(&headers);
    Json(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base]
    }))
}

pub(super) async fn authorization_server_metadata(headers: HeaderMap) -> impl IntoResponse {
    let base = base_url(&headers);
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"]
    }))
}

// ---------------------------------------------------------------------------
// Dynamic Client Registration (RFC 7591)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct RegisterRequest {
    client_name: Option<String>,
    redirect_uris: Vec<String>,
}

pub(super) async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    if req.redirect_uris.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error": "invalid_client_metadata", "error_description": "redirect_uris required"}),
            ),
        );
    }

    // Validate all redirect URIs are valid URLs
    for uri in &req.redirect_uris {
        if url::Url::parse(uri).is_err() {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({"error": "invalid_client_metadata", "error_description": format!("invalid redirect_uri: {uri}")}),
                ),
            );
        }
    }

    let client_id = auth::generate_token();
    let now = chrono::Utc::now().fixed_offset();

    let model = oauth_client::ActiveModel {
        client_id: Set(client_id.clone()),
        client_name: Set(req.client_name.clone()),
        redirect_uris: Set(serde_json::to_value(&req.redirect_uris).unwrap()),
        created_at: Set(now),
        ..Default::default()
    };

    match model.insert(&state.db).await {
        Ok(_) => (
            StatusCode::CREATED,
            Json(json!({
                "client_id": client_id,
                "client_name": req.client_name,
                "redirect_uris": req.redirect_uris,
                "token_endpoint_auth_method": "none"
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "server_error", "error_description": e.to_string()})),
        ),
    }
}
