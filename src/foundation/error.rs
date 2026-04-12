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
    Http { status: u16, message: String },

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
        }
    }
}

/// The standard JSON error response body.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub message: String,
    pub status: u16,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let message = self.to_string();
        let body = ErrorResponse {
            message,
            status: status.as_u16(),
        };
        (status, Json(body)).into_response()
    }
}

/// Allow `ValidationErrors` to be converted into `Error`.
impl From<crate::validation::ValidationErrors> for Error {
    fn from(errors: crate::validation::ValidationErrors) -> Self {
        Self::Http {
            status: 422,
            message: errors.to_string(),
        }
    }
}

/// Allow `AuthError` to be converted into `Error`.
impl From<crate::auth::AuthError> for Error {
    fn from(error: crate::auth::AuthError) -> Self {
        let (status, message) = match &error {
            crate::auth::AuthError::Unauthorized(msg) => (401, msg.clone()),
            crate::auth::AuthError::Forbidden(msg) => (403, msg.clone()),
            crate::auth::AuthError::Internal(msg) => (500, msg.clone()),
        };
        Self::Http { status, message }
    }
}
