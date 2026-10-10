use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sea_orm::DbErr;
use serde::Serialize;

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Unauthorized,
    NotFound,
    Conflict(String),
    Internal(String),
    ServiceUnavailable(String),
    Unprocessable(String),
    /// A rejection a client acts on by kind: `code` names it and `details`
    /// lists its items (failing templates, conflicting paths), so clients
    /// need not parse `message`, which stays the human-readable text.
    Detailed {
        status: StatusCode,
        message: String,
        code: &'static str,
        details: Vec<String>,
    },
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'a str>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    details: &'a [String],
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
            Self::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Detailed { status, .. } => *status,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::BadRequest(msg)
            | Self::Conflict(msg)
            | Self::Internal(msg)
            | Self::ServiceUnavailable(msg)
            | Self::Unprocessable(msg)
            | Self::Detailed { message: msg, .. } => msg.clone(),
            Self::Unauthorized => "unauthorized".to_string(),
            Self::NotFound => "not found".to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, details) = match &self {
            Self::Detailed { code, details, .. } => (Some(*code), details.as_slice()),
            _ => (None, &[][..]),
        };
        let body = ErrorBody {
            error: self.message(),
            code,
            details,
        };
        (self.status(), Json(body)).into_response()
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
            Error::InvalidKey(key) => Self::BadRequest(format!("invalid path {key:?}")),
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

    #[tokio::test]
    async fn detailed_errors_carry_code_and_details() {
        let api = ApiError::Detailed {
            status: StatusCode::CONFLICT,
            message: "busy".into(),
            code: "conflict",
            details: vec!["templates/a.html".into()],
        };
        let body = axum::body::to_bytes(api.into_response().into_body(), 1024)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(
            json,
            serde_json::json!({ "error": "busy", "code": "conflict", "details": ["templates/a.html"] })
        );

        let plain = ApiError::NotFound.into_response().into_body();
        let body = axum::body::to_bytes(plain, 1024).await.expect("body");
        assert_eq!(&body[..], br#"{"error":"not found"}"#);
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
