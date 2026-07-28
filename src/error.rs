//! Structured errors. Every variant maps to a specific HTTP
//! status via `IntoResponse`.
//!
//! Fixes JVM bug #9 — Spring version returned `400` + `e.getCause()`
//! (often null, often not JSON-serialisable).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum XtrError {
    #[error("template not found: {group}/{service}")]
    TemplateNotFound { group: String, service: String },

    #[error("handlebars expansion failed: {0}")]
    HandlebarsError(String),

    #[error("upstream HTTP error {status}: {body}")]
    UpstreamHttpError { status: u16, body: String },

    #[error("upstream request timed out")]
    UpstreamTimeout,

    #[error("failed to parse upstream XML: {0}")]
    XmlParseError(String),

    #[error("keystore load failed: {0}")]
    KeystoreLoadFailed(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl XtrError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::TemplateNotFound { .. } => StatusCode::NOT_FOUND,
            Self::HandlebarsError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::UpstreamHttpError { .. } => StatusCode::BAD_GATEWAY,
            Self::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
            Self::XmlParseError(_) => StatusCode::BAD_GATEWAY,
            Self::KeystoreLoadFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::TemplateNotFound { .. } => "template_not_found",
            Self::HandlebarsError(_) => "template_expansion_failed",
            Self::UpstreamHttpError { .. } => "upstream_http_error",
            Self::UpstreamTimeout => "upstream_timeout",
            Self::XmlParseError(_) => "upstream_xml_parse_error",
            Self::KeystoreLoadFailed(_) => "keystore_load_failed",
            Self::Internal(_) => "internal_error",
        }
    }
}

impl IntoResponse for XtrError {
    fn into_response(self) -> Response {
        tracing::warn!("request failed: {}", self);
        let status = self.status();
        let body = json!({
            "error": self.code(),
            "message": self.to_string(),
        });
        (status, Json(body)).into_response()
    }
}
