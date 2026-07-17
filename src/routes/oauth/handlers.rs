//! Axum HTTP handlers for the interactive authorize + token endpoints.
//!
//! These handlers stay thin: they extract request state and build
//! responses, delegating PKCE/DB/token-issuance logic to [`super::security`].

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Form, Json};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;
use serde_json::json;

use crate::auth;
use crate::entity::oauth_code;
use crate::state::AppState;

use super::security;

const CODE_MINUTES: i64 = 10;

// ---------------------------------------------------------------------------
// Authorization endpoint
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct AuthorizeParams {
    pub(super) client_id: String,
    pub(super) redirect_uri: String,
    pub(super) response_type: String,
    pub(super) state: Option<String>,
    pub(super) code_challenge: String,
    pub(super) code_challenge_method: String,
}

pub(super) async fn authorize_form(
    State(state): State<AppState>,
    Query(params): Query<AuthorizeParams>,
) -> Response {
    // Validate the request
    if let Err(e) = security::validate_authorize_params(&state, &params).await {
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
pub(super) struct AuthorizeSubmit {
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

pub(super) async fn authorize_submit(
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
pub(super) struct TokenRequest {
    grant_type: String,
    pub(super) code: Option<String>,
    #[allow(dead_code)]
    client_id: Option<String>,
    pub(super) code_verifier: Option<String>,
    pub(super) redirect_uri: Option<String>,
    pub(super) refresh_token: Option<String>,
}

pub(super) async fn token(
    State(state): State<AppState>,
    Form(req): Form<TokenRequest>,
) -> impl IntoResponse {
    match req.grant_type.as_str() {
        "authorization_code" => security::exchange_code(&state, &req).await,
        "refresh_token" => security::refresh(&state, &req).await,
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "unsupported_grant_type"})),
        ),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
