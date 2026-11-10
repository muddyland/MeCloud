//! Errors, and how each one reaches the client.
//!
//! Every error body is `{"detail": ...}`, the shape the frontend and the
//! desktop client already read.

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::session::ClearSession;

#[derive(Debug)]
pub enum AppError {
    /// A response decided here: a status, a detail, maybe headers.
    Http {
        status: StatusCode,
        detail: Value,
        headers: Vec<(&'static str, String)>,
    },
    /// An upstream (Stalwart) call answered with an error, or with something
    /// unusable. `status` is the upstream HTTP status when there was one.
    Upstream {
        message: String,
        status: Option<u16>,
    },
    /// The upstream could not be reached at all: refused, timed out, DNS.
    Transport(String),
}

impl AppError {
    pub fn http(status: u16, detail: impl Into<String>) -> AppError {
        AppError::Http {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            detail: Value::String(detail.into()),
            headers: Vec::new(),
        }
    }

    pub fn with_header(mut self, name: &'static str, value: impl Into<String>) -> AppError {
        if let AppError::Http { headers, .. } = &mut self {
            headers.push((name, value.into()));
        }
        self
    }

    pub fn upstream(message: impl Into<String>, status: Option<u16>) -> AppError {
        AppError::Upstream {
            message: message.into(),
            status,
        }
    }

    /// A 422 in FastAPI's shape: a list of what was wrong, and where.
    pub fn invalid(loc: &[&str], kind: &str, msg: impl Into<String>) -> AppError {
        AppError::Http {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail: json!([{ "type": kind, "loc": loc, "msg": msg.into() }]),
            headers: Vec::new(),
        }
    }

    pub fn missing(location: &str, field: &str) -> AppError {
        AppError::invalid(&[location, field], "missing", "Field required")
    }

    pub fn unauthenticated() -> AppError {
        AppError::http(401, "Not authenticated")
    }

    pub fn session_expired() -> AppError {
        AppError::http(401, "Session expired, please log in again")
    }

    pub fn upstream_status(&self) -> Option<u16> {
        match self {
            AppError::Upstream { status, .. } => *status,
            _ => None,
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Http { status, detail, .. } => write!(f, "HTTP {status}: {detail}"),
            AppError::Upstream { message, .. } => f.write_str(message),
            AppError::Transport(message) => f.write_str(message),
        }
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> AppError {
        if err.is_decode() {
            // A 2xx whose body is not the JSON it should be.
            AppError::upstream("The mail server returned a malformed response", None)
        } else {
            AppError::Transport(error_chain(&err))
        }
    }
}

/// An error and its causes on one line: reqwest's own message alone is
/// usually just "error sending request".
pub fn error_chain(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        out.push_str(": ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

pub fn detail(status: StatusCode, detail: &str) -> Response {
    (status, Json(json!({ "detail": detail }))).into_response()
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Http {
                status,
                detail,
                headers,
            } => {
                let mut response = (status, Json(json!({ "detail": detail }))).into_response();
                for (name, value) in headers {
                    if let Ok(value) = HeaderValue::from_str(&value) {
                        response.headers_mut().insert(name, value);
                    }
                }
                response
            }
            AppError::Upstream { message, status } => match status {
                // A revoked or expired token. Mapped to 401, which makes the
                // client sign in again, rather than a 502 it retries forever.
                Some(401 | 403) => {
                    let mut response = detail(
                        StatusCode::UNAUTHORIZED,
                        "Session expired, please log in again",
                    );
                    response.extensions_mut().insert(ClearSession);
                    response
                }
                // Statuses the client can act on pass through; the rest
                // collapse to 502 so upstream internals are not echoed back.
                Some(code @ (400 | 404 | 413 | 503)) => {
                    detail(StatusCode::from_u16(code).unwrap(), &message)
                }
                _ => {
                    tracing::error!("Upstream error: {message}");
                    detail(StatusCode::BAD_GATEWAY, "Upstream service error")
                }
            },
            AppError::Transport(message) => {
                tracing::error!("Upstream transport error: {message}");
                detail(StatusCode::GATEWAY_TIMEOUT, "Upstream service unavailable")
            }
        }
    }
}

pub type Result<T, E = AppError> = std::result::Result<T, E>;
