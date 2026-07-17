//! PKCE verification and authorization-code / token issuance logic.
//!
//! These functions take plain values (not Axum extractors) and return
//! `Result`/plain types so they can be unit tested without spinning up
//! Axum. The thin handlers in [`super::handlers`] call into these.

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, FixedOffset};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::auth;
use crate::entity::{oauth_client, oauth_code, oauth_token};
use crate::state::AppState;

use super::base_url;
use super::handlers::{AuthorizeParams, TokenRequest};

const ACCESS_TOKEN_HOURS: i64 = 1;

/// Validate authorization request parameters.
pub(super) async fn validate_authorize_params(
    state: &AppState,
    params: &AuthorizeParams,
) -> Result<(), String> {
    if params.response_type != "code" {
        return Err("Unsupported response_type (must be 'code')".into());
    }
    if params.code_challenge_method != "S256" {
        return Err("Unsupported code_challenge_method (must be 'S256')".into());
    }

    // Verify client_id exists
    let client = oauth_client::Entity::find()
        .filter(oauth_client::Column::ClientId.eq(&params.client_id))
        .one(&state.db)
        .await
        .map_err(|e| e.to_string())?;

    let client = client.ok_or("Unknown client_id")?;

    // Verify redirect_uri is registered
    let uris: Vec<String> = serde_json::from_value(client.redirect_uris).unwrap_or_default();
    if !uris.contains(&params.redirect_uri) {
        return Err("redirect_uri not registered for this client".into());
    }

    Ok(())
}

pub(super) async fn exchange_code(
    state: &AppState,
    req: &TokenRequest,
) -> (StatusCode, Json<serde_json::Value>) {
    let code_str = match &req.code {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_request", "error_description": "code required"})),
            );
        }
    };
    let verifier = match &req.code_verifier {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({"error": "invalid_request", "error_description": "code_verifier required"}),
                ),
            );
        }
    };

    // Find the code
    let code_model = oauth_code::Entity::find()
        .filter(oauth_code::Column::Code.eq(code_str.as_str()))
        .one(&state.db)
        .await;

    let code_model = match code_model {
        Ok(Some(c)) => c,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant", "error_description": "code not found"})),
            );
        }
    };

    // Check expiration
    let now: chrono::DateTime<chrono::FixedOffset> = chrono::Utc::now().into();
    if !is_code_valid(code_model.used, code_model.expires_at, now) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error": "invalid_grant", "error_description": "code expired or already used"}),
            ),
        );
    }

    // Verify PKCE: SHA256(code_verifier) == code_challenge
    if !verify_pkce(verifier, &code_model.code_challenge) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant", "error_description": "code_verifier mismatch"})),
        );
    }

    // Verify redirect_uri matches
    if let Some(ref uri) = req.redirect_uri
        && *uri != code_model.redirect_uri
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant", "error_description": "redirect_uri mismatch"})),
        );
    }

    // Mark code as used
    let mut active: oauth_code::ActiveModel = code_model.clone().into();
    active.used = Set(true);
    let _ = active.update(&state.db).await;

    // Issue tokens
    issue_tokens(state, &code_model.client_id, code_model.user_id).await
}

pub(super) async fn refresh(
    state: &AppState,
    req: &TokenRequest,
) -> (StatusCode, Json<serde_json::Value>) {
    let refresh_str = match &req.refresh_token {
        Some(r) => r,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({"error": "invalid_request", "error_description": "refresh_token required"}),
                ),
            );
        }
    };

    let tok = oauth_token::Entity::find()
        .filter(oauth_token::Column::RefreshToken.eq(refresh_str.as_str()))
        .filter(oauth_token::Column::Revoked.eq(false))
        .one(&state.db)
        .await;

    let tok = match tok {
        Ok(Some(t)) => t,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({"error": "invalid_grant", "error_description": "invalid refresh token"}),
                ),
            );
        }
    };

    // Revoke old token
    let mut active: oauth_token::ActiveModel = tok.clone().into();
    active.revoked = Set(true);
    let _ = active.update(&state.db).await;

    // Issue new tokens
    issue_tokens(state, &tok.client_id, tok.user_id).await
}

async fn issue_tokens(
    state: &AppState,
    client_id: &str,
    user_id: i32,
) -> (StatusCode, Json<serde_json::Value>) {
    let access = auth::generate_token();
    let refresh = auth::generate_token();
    let now = chrono::Utc::now().fixed_offset();
    let expires = now + chrono::Duration::hours(ACCESS_TOKEN_HOURS);

    let model = oauth_token::ActiveModel {
        access_token: Set(access.clone()),
        refresh_token: Set(refresh.clone()),
        client_id: Set(client_id.to_owned()),
        user_id: Set(user_id),
        expires_at: Set(expires),
        revoked: Set(false),
        ..Default::default()
    };

    match model.insert(&state.db).await {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({
                "access_token": access,
                "token_type": "Bearer",
                "expires_in": ACCESS_TOKEN_HOURS * 3600,
                "refresh_token": refresh
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "server_error", "error_description": e.to_string()})),
        ),
    }
}

/// Authenticate an MCP request via OAuth access token or legacy service token.
/// Returns the user_id on success.
pub async fn authenticate_mcp(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<i32, (StatusCode, String)> {
    let nonce = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            let resource_meta =
                format!("{}/.well-known/oauth-protected-resource", base_url(headers));
            (
                StatusCode::UNAUTHORIZED,
                format!("Bearer resource_metadata=\"{resource_meta}\""),
            )
        })?;

    // Try OAuth access token first
    let now: chrono::DateTime<chrono::FixedOffset> = chrono::Utc::now().into();
    if let Ok(Some(tok)) = oauth_token::Entity::find()
        .filter(oauth_token::Column::AccessToken.eq(nonce))
        .filter(oauth_token::Column::Revoked.eq(false))
        .one(&state.db)
        .await
        && tok.expires_at > now
    {
        return Ok(tok.user_id);
    }

    // Fall back to legacy service token
    if let Ok(Some(tok)) = crate::entity::token::Entity::find()
        .filter(crate::entity::token::Column::Nonce.eq(nonce))
        .filter(crate::entity::token::Column::IsService.eq(true))
        .one(&state.db)
        .await
    {
        return Ok(tok.user_id);
    }

    let resource_meta = format!("{}/.well-known/oauth-protected-resource", base_url(headers));
    Err((
        StatusCode::UNAUTHORIZED,
        format!("Bearer error=\"invalid_token\", resource_metadata=\"{resource_meta}\""),
    ))
}

// ---------------------------------------------------------------------------
// Pure helpers (no I/O — safe to unit test directly)
// ---------------------------------------------------------------------------

/// A stored authorization code is usable only if unused and not yet expired.
fn is_code_valid(
    used: bool,
    expires_at: DateTime<FixedOffset>,
    now: DateTime<FixedOffset>,
) -> bool {
    !used && expires_at >= now
}

/// PKCE verification: SHA256(code_verifier), base64url-encoded (no padding),
/// must equal the code_challenge stored at authorization time.
fn verify_pkce(verifier: &str, code_challenge: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    base64_url_encode(&hasher.finalize()) == code_challenge
}

fn base64_url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_pkce_matches_correct_verifier() {
        let verifier = "test-code-verifier-1234567890";
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let challenge = base64_url_encode(&hasher.finalize());

        assert!(verify_pkce(verifier, &challenge));
        assert!(!verify_pkce(verifier, "wrong-challenge"));
        assert!(!verify_pkce("wrong-verifier", &challenge));
    }

    #[test]
    fn is_code_valid_rejects_used_or_expired() {
        let now: DateTime<FixedOffset> = chrono::Utc::now().into();
        let future = now + chrono::Duration::minutes(5);
        let past = now - chrono::Duration::minutes(5);

        assert!(is_code_valid(false, future, now));
        assert!(!is_code_valid(true, future, now));
        assert!(!is_code_valid(false, past, now));
    }
}
