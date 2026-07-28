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

    /// SOAP Fault (business error) inside a transport-successful
    /// (HTTP 200) response. Task 010 — before this, faults were
    /// silently translated to a "successful" JSON body.
    #[error("upstream returned SOAP Fault ({code}): {string}")]
    UpstreamSoapFault {
        code: String,
        string: String,
        detail: Option<serde_json::Value>,
    },

    /// Inbound REST body exceeded the configured limit. Task 011.
    #[error("request body exceeds {limit} bytes")]
    RequestTooLarge { limit: usize },

    /// Upstream response exceeded the configured limit. Task 011.
    #[error("upstream response exceeds {limit} bytes")]
    UpstreamBodyTooLarge { limit: usize },

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
            Self::UpstreamSoapFault { .. } => StatusCode::BAD_GATEWAY,
            Self::RequestTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UpstreamBodyTooLarge { .. } => StatusCode::BAD_GATEWAY,
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
            Self::UpstreamSoapFault { .. } => "upstream_soap_fault",
            Self::RequestTooLarge { .. } => "request_too_large",
            Self::UpstreamBodyTooLarge { .. } => "upstream_body_too_large",
            Self::KeystoreLoadFailed(_) => "keystore_load_failed",
            Self::Internal(_) => "internal_error",
        }
    }
}

impl IntoResponse for XtrError {
    fn into_response(self) -> Response {
        tracing::warn!("request failed: {}", self);
        let status = self.status();
        let body = match &self {
            Self::UpstreamSoapFault {
                code,
                string,
                detail,
            } => json!({
                "error": "upstream_soap_fault",
                "message": self.to_string(),
                "code": code,
                "string": string,
                "detail": detail,
            }),
            Self::RequestTooLarge { limit } | Self::UpstreamBodyTooLarge { limit } => json!({
                "error": self.code(),
                "message": self.to_string(),
                "limit": limit,
            }),
            _ => json!({
                "error": self.code(),
                "message": self.to_string(),
            }),
        };
        (status, Json(body)).into_response()
    }
}
