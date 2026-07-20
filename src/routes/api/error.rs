use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sea_orm::DbErr;
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Unauthorized,
    NotFound,
    Conflict(String),
    Internal(String),
    ServiceUnavailable(String),
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::ServiceUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::BadRequest(msg)
            | Self::Conflict(msg)
            | Self::Internal(msg)
            | Self::ServiceUnavailable(msg) => msg.clone(),
            Self::Unauthorized => "unauthorized".to_string(),
            Self::NotFound => "not found".to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status(), Json(json!({ "error": self.message() }))).into_response()
    }
}

impl From<DbErr> for ApiError {
    fn from(err: DbErr) -> Self {
        tracing::error!("DB error: {err}");
        Self::Internal(err.to_string())
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
