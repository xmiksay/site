use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::auth;
use crate::entity::{token, user};
use crate::routes::api::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
}

#[derive(serde::Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(serde::Serialize)]
pub struct UserResponse {
    pub user_id: i32,
    pub username: String,
}

pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<LoginRequest>,
) -> ApiResult<(CookieJar, Json<UserResponse>)> {
    let user = user::Entity::find()
        .filter(user::Column::Username.eq(&req.username))
        .one(&state.db)
        .await?
        .ok_or(ApiError::Unauthorized)?;

    if !auth::verify_password(&req.password, &user.password_hash) {
        return Err(ApiError::Unauthorized);
    }

    let nonce = auth::generate_token();
    let expires_at = chrono::Utc::now() + chrono::Duration::hours(auth::SESSION_HOURS);

    token::ActiveModel {
        nonce: Set(nonce.clone()),
        user_id: Set(user.id),
        expires_at: Set(Some(expires_at.into())),
        is_service: Set(false),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;

    let cookie = Cookie::build((auth::SESSION_COOKIE, nonce))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Lax)
        .build();

    Ok((
        jar.add(cookie),
        Json(UserResponse {
            user_id: user.id,
            username: user.username,
        }),
    ))
}

pub async fn logout(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    if let Some(cookie) = jar.get(auth::SESSION_COOKIE) {
        let nonce = cookie.value().to_string();
        token::Entity::delete_many()
            .filter(token::Column::Nonce.eq(nonce))
            .exec(&state.db)
            .await
            .ok();
    }
    let removal = Cookie::build(auth::SESSION_COOKIE).path("/").build();
    (jar.remove(removal), StatusCode::NO_CONTENT)
}

pub async fn me(State(state): State<AppState>, jar: CookieJar) -> ApiResult<Json<UserResponse>> {
    let user_id = auth::is_logged_in(&state, &jar)
        .await
        .ok_or(ApiError::Unauthorized)?;
    let user = user::Entity::find_by_id(user_id)
        .one(&state.db)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    Ok(Json(UserResponse {
        user_id: user.id,
        username: user.username,
    }))
}
