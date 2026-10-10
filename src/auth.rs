use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;

use crate::{entity::token, state::AppState};

pub const SESSION_COOKIE: &str = "site_session";
pub const SESSION_HOURS: i64 = 24;

pub fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2 with default params and a fresh salt cannot fail")
        .to_string()
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    // The hash comes from the DB — a corrupt/legacy row must fail the login,
    // not panic the request task.
    let Ok(parsed) = PasswordHash::new(hash) else {
        tracing::error!("stored password hash is malformed");
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn generate_token() -> String {
    use rand::RngExt;
    let bytes: [u8; 32] = rand::rng().random();
    hex::encode(bytes)
}

/// Middleware: verify session cookie for /api/*. Returns 401 JSON on failure.
pub async fn require_login_api(
    State(state): State<AppState>,
    jar: CookieJar,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(nonce) = jar.get(SESSION_COOKIE).map(|c| c.value().to_string()) else {
        return unauthorized();
    };

    let Ok(Some(tok)) = token::Entity::find()
        .filter(token::Column::Nonce.eq(&nonce))
        .filter(token::Column::IsService.eq(false))
        .one(&state.db)
        .await
    else {
        return unauthorized();
    };

    if let Some(expires) = tok.expires_at {
        let now: chrono::DateTime<chrono::FixedOffset> = chrono::Utc::now().into();
        if expires < now {
            return unauthorized();
        }
    }

    req.extensions_mut().insert(tok.user_id);
    next.run(req).await
}

/// Middleware: [`require_login_api`], or else the MCP Bearer token (OAuth
/// access token or service token) — for the routes external agents may use.
pub async fn require_login_or_bearer_api(
    State(state): State<AppState>,
    jar: CookieJar,
    mut req: Request,
    next: Next,
) -> Response {
    let user_id = match is_logged_in(&state, &jar).await {
        Some(id) => id,
        None => match crate::routes::oauth::authenticate_mcp(&state, req.headers()).await {
            Ok(id) => id,
            Err(_) => return unauthorized(),
        },
    };
    req.extensions_mut().insert(user_id);
    next.run(req).await
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized" })),
    )
        .into_response()
}

/// Check if request has valid session (non-blocking, for template use).
pub async fn is_logged_in(state: &AppState, jar: &CookieJar) -> Option<i32> {
    let cookie = jar.get(SESSION_COOKIE)?;
    let nonce = cookie.value().to_string();
    let tok = token::Entity::find()
        .filter(token::Column::Nonce.eq(&nonce))
        .filter(token::Column::IsService.eq(false))
        .one(&state.db)
        .await
        .ok()
        .flatten()?;
    if let Some(expires) = tok.expires_at {
        let now: chrono::DateTime<chrono::FixedOffset> = chrono::Utc::now().into();
        if expires < now {
            return None;
        }
    }
    Some(tok.user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_round_trip() {
        let hash = hash_password("s3cret");
        assert!(verify_password("s3cret", &hash));
        assert!(!verify_password("wrong", &hash));
    }

    #[test]
    fn malformed_stored_hash_fails_instead_of_panicking() {
        assert!(!verify_password("anything", "not-a-phc-string"));
        assert!(!verify_password("anything", ""));
    }
}
