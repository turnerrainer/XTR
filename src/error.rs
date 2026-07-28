//! Structured errors. Every variant maps to a specific HTTP status
//! (via `IntoResponse`, wired in Phase F).
//!
//! Fixes JVM bug #9 (`400 + e.getCause()` — often null, often
//! unserialisable).

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
