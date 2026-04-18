use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// Unified error type for the Forge framework.
///
/// Produces consistent JSON error responses across HTTP, validation,
/// auth, and internal errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A plain message error (used throughout the framework).
    /// Maps to HTTP 500 Internal Server Error.
    #[error("{0}")]
    Message(String),

    /// An HTTP error with a specific status code.
    #[error("{message}")]
    Http {
        status: u16,
        message: String,
        error_code: Option<String>,
    },

    /// Validation errors with per-field detail. Maps to HTTP 422.
    #[error("validation failed")]
    Validation(crate::validation::ValidationErrors),

    /// A "not found" error. Maps to HTTP 404.
    #[error("{0}")]
    NotFound(String),

    /// Wraps anyhow::Error for backward compatibility.
    /// Maps to HTTP 500 Internal Server Error.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Create a message error (replaces old `Error::message()`).
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// Create an HTTP error with a specific status code.
    pub fn http(status: u16, message: impl Into<String>) -> Self {
        Self::Http {
            status,
            message: message.into(),
            error_code: None,
        }
    }

    /// Create an HTTP error with a specific status code and error code.
    pub fn http_with_code(
        status: u16,
        message: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        Self::Http {
            status,
            message: message.into(),
            error_code: Some(code.into()),
        }
    }

    /// Create a 404 Not Found error.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    /// Wrap an arbitrary error.
    pub fn other<E>(error: E) -> Self
    where
        E: Into<anyhow::Error>,
    {
        Self::Other(error.into())
    }

    fn status_code(&self) -> StatusCode {
        match self {
            Error::Message(_) | Error::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Error::Http { status, .. } => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
            }
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }

    pub fn payload(&self) -> serde_json::Value {
        let status = self.status_code();
        let error_code = match self {
            Error::Http { error_code, .. } => error_code.clone(),
            _ => None,
        };

        let mut payload = serde_json::json!({
            "message": self.to_string(),
            "status": status.as_u16(),
        });

        if let Some(error_code) = error_code {
            payload["error_code"] = serde_json::Value::String(error_code);
        }

        payload
    }
}

/// The standard JSON error response body.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub message: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<crate::validation::FieldError>>,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        // Validation errors delegate to their own structured response.
        if let Error::Validation(errors) = self {
            return errors.into_response();
        }

        let status = self.status_code();
        let body = ErrorResponse {
            message: self.to_string(),
            status: status.as_u16(),
            error_code: match &self {
                Error::Http { error_code, .. } => error_code.clone(),
                _ => None,
            },
            errors: None,
        };
        (status, Json(body)).into_response()
    }
}

/// Allow `ValidationErrors` to be converted into `Error`.
impl From<crate::validation::ValidationErrors> for Error {
    fn from(errors: crate::validation::ValidationErrors) -> Self {
        Self::Validation(errors)
    }
}

/// Allow `AuthError` to be converted into `Error`.
impl From<crate::auth::AuthError> for Error {
    fn from(error: crate::auth::AuthError) -> Self {
        let status = error.status_code().as_u16();
        let message = error.message().to_string();
        let error_code = error.code().map(|code| code.as_str().to_string());
        Self::Http {
            status,
            message,
            error_code,
        }
    }
}
