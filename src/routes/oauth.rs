use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::auth;
use crate::entity::{oauth_client, oauth_code, oauth_token};
use crate::state::AppState;

const ACCESS_TOKEN_HOURS: i64 = 1;
const CODE_MINUTES: i64 = 10;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/oauth/register", post(register))
        .route(
            "/oauth/authorize",
            get(authorize_form).post(authorize_submit),
        )
        .route("/oauth/token", post(token))
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server_metadata),
        )
}

/// Return the base URL from the Host header or fall back to localhost.
fn base_url(headers: &HeaderMap) -> String {
    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        let scheme = if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
            "http"
        } else {
            "https"
        };
        format!("{scheme}://{host}")
    } else {
        "http://localhost:3000".to_string()
    }
}

// ---------------------------------------------------------------------------
// Well-known metadata endpoints
// ---------------------------------------------------------------------------

async fn protected_resource_metadata(headers: HeaderMap) -> impl IntoResponse {
    let base = base_url(&headers);
    Json(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base]
    }))
}

async fn authorization_server_metadata(headers: HeaderMap) -> impl IntoResponse {
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
struct RegisterRequest {
    client_name: Option<String>,
    redirect_uris: Vec<String>,
}

async fn register(
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

// ---------------------------------------------------------------------------
// Authorization endpoint
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AuthorizeParams {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    state: Option<String>,
    code_challenge: String,
    code_challenge_method: String,
}

async fn authorize_form(
    State(state): State<AppState>,
    Query(params): Query<AuthorizeParams>,
) -> Response {
    // Validate the request
    if let Err(e) = validate_authorize_params(&state, &params).await {
        return (StatusCode::BAD_REQUEST, Html(e)).into_response();
    }

    let state_param = params.state.as_deref().unwrap_or("");
    let html = format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Authorize</title>
<style>
body {{ font-family: sans-serif; max-width: 400px; margin: 60px auto; }}
input {{ width: 100%; padding: 8px; margin: 4px 0 12px; box-sizing: border-box; }}
button {{ padding: 10px 20px; cursor: pointer; }}
.client {{ color: #555; }}
</style></head>
<body>
<h2>Authorize access</h2>
<p class="client"><strong>{}</strong> wants to access your site.</p>
<form method="post">
<input type="hidden" name="client_id" value="{}">
<input type="hidden" name="redirect_uri" value="{}">
<input type="hidden" name="response_type" value="code">
<input type="hidden" name="state" value="{}">
<input type="hidden" name="code_challenge" value="{}">
<input type="hidden" name="code_challenge_method" value="S256">
<label>Username<input type="text" name="username" required></label>
<label>Password<input type="password" name="password" required></label>
<button type="submit">Authorize</button>
</form>
</body></html>"#,
        html_escape(&params.client_id),
        html_escape(&params.client_id),
        html_escape(&params.redirect_uri),
        html_escape(state_param),
        html_escape(&params.code_challenge),
    );

    Html(html).into_response()
}

#[derive(Deserialize)]
struct AuthorizeSubmit {
    client_id: String,
    redirect_uri: String,
    #[allow(dead_code)]
    response_type: String,
    state: Option<String>,
    code_challenge: String,
    #[allow(dead_code)]
    code_challenge_method: String,
    username: String,
    password: String,
}

async fn authorize_submit(
    State(state): State<AppState>,
    Form(form): Form<AuthorizeSubmit>,
) -> Response {
    // Verify user credentials
    let user = crate::entity::user::Entity::find()
        .filter(crate::entity::user::Column::Username.eq(&form.username))
        .one(&state.db)
        .await;

    let user = match user {
        Ok(Some(u)) if auth::verify_password(&form.password, &u.password_hash) => u,
        _ => {
            return redirect_with_error(
                &form.redirect_uri,
                "access_denied",
                "Invalid credentials",
                form.state.as_deref(),
            );
        }
    };

    // Generate authorization code
    let code = auth::generate_token();
    let now = chrono::Utc::now().fixed_offset();
    let expires = now + chrono::Duration::minutes(CODE_MINUTES);

    let model = oauth_code::ActiveModel {
        code: Set(code.clone()),
        client_id: Set(form.client_id.clone()),
        user_id: Set(user.id),
        redirect_uri: Set(form.redirect_uri.clone()),
        code_challenge: Set(form.code_challenge.clone()),
        expires_at: Set(expires),
        used: Set(false),
        ..Default::default()
    };

    if model.insert(&state.db).await.is_err() {
        return redirect_with_error(
            &form.redirect_uri,
            "server_error",
            "Failed to create authorization code",
            form.state.as_deref(),
        );
    }

    // Redirect back with code
    let mut redirect_url = match url::Url::parse(&form.redirect_uri) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid redirect_uri").into_response(),
    };
    redirect_url.query_pairs_mut().append_pair("code", &code);
    if let Some(ref s) = form.state {
        redirect_url.query_pairs_mut().append_pair("state", s);
    }

    Redirect::to(redirect_url.as_str()).into_response()
}

// ---------------------------------------------------------------------------
// Token endpoint
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    code: Option<String>,
    #[allow(dead_code)]
    client_id: Option<String>,
    code_verifier: Option<String>,
    redirect_uri: Option<String>,
    refresh_token: Option<String>,
}

async fn token(State(state): State<AppState>, Form(req): Form<TokenRequest>) -> impl IntoResponse {
    match req.grant_type.as_str() {
        "authorization_code" => exchange_code(&state, &req).await,
        "refresh_token" => refresh(&state, &req).await,
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "unsupported_grant_type"})),
        ),
    }
}

async fn exchange_code(
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
    if code_model.used || code_model.expires_at < now {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error": "invalid_grant", "error_description": "code expired or already used"}),
            ),
        );
    }

    // Verify PKCE: SHA256(code_verifier) == code_challenge
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let computed = base64_url_encode(&hasher.finalize());
    if computed != code_model.code_challenge {
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

async fn refresh(state: &AppState, req: &TokenRequest) -> (StatusCode, Json<serde_json::Value>) {
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Validate authorization request parameters.
async fn validate_authorize_params(
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

fn redirect_with_error(
    redirect_uri: &str,
    error: &str,
    desc: &str,
    state: Option<&str>,
) -> Response {
    if let Ok(mut url) = url::Url::parse(redirect_uri) {
        url.query_pairs_mut()
            .append_pair("error", error)
            .append_pair("error_description", desc);
        if let Some(s) = state {
            url.query_pairs_mut().append_pair("state", s);
        }
        Redirect::to(url.as_str()).into_response()
    } else {
        (StatusCode::BAD_REQUEST, desc.to_owned()).into_response()
    }
}

fn base64_url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
