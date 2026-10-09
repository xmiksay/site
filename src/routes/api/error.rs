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
        // Full detail goes to the log only — DbErr strings carry table/column
        // names and SQL fragments that must not reach clients.
        tracing::error!("DB error: {err}");
        Self::Internal("internal error".into())
    }
}

impl From<crate::storage::Error> for ApiError {
    fn from(err: crate::storage::Error) -> Self {
        use crate::storage::Error;
        match err {
            Error::Unavailable(e) => {
                tracing::error!("storage unavailable: {e}");
                Self::ServiceUnavailable("storage unavailable".into())
            }
            Error::Db(db) => db.into(),
            Error::InvalidHash(_) => {
                tracing::error!("{err}");
                Self::Internal("internal error".into())
            }
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_error_is_not_leaked_to_clients() {
        let err = DbErr::Custom("relation \"secret_table\" does not exist".into());
        let api: ApiError = err.into();
        match api {
            ApiError::Internal(msg) => {
                assert_eq!(msg, "internal error");
                assert!(!msg.contains("secret_table"));
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn storage_outage_is_503() {
        let err = crate::storage::Error::Unavailable(object_store::Error::Generic {
            store: "S3",
            source: "connection refused".into(),
        });
        let api: ApiError = err.into();
        assert_eq!(api.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(api.message(), "storage unavailable");
    }
}
