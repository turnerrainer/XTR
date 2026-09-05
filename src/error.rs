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
        self.into_response_with_options(false)
    }
}

/// Audit-v1 H3 — cap on `faultstring` exposed to REST callers when
/// the upstream SOAP fault leaks server internals into that field.
/// Full text is always logged via `tracing::warn!` for operators.
pub const SOAP_FAULT_STRING_MAX: usize = 200;

impl XtrError {
    /// Render as an HTTP response. When `expose_soap_fault_detail`
    /// is false (the default), upstream SOAP fault `detail` blocks
    /// and any `faultstring` beyond `SOAP_FAULT_STRING_MAX` chars
    /// are stripped from the client-visible body. The full,
    /// untruncated fault is emitted at `warn!` level so operators
    /// still have it for debugging. Set the flag true only inside
    /// trusted environments where callers should see raw upstream
    /// diagnostics.
    pub fn into_response_with_options(self, expose_soap_fault_detail: bool) -> Response {
        // Always log the full error for the operator, including
        // fault detail — the flag only affects the response body.
        match &self {
            Self::UpstreamSoapFault {
                code,
                string,
                detail,
            } => {
                tracing::warn!(
                    fault_code = %code,
                    fault_string = %string,
                    fault_detail = ?detail,
                    "upstream SOAP fault (full detail)"
                );
            }
            _ => tracing::warn!("request failed: {}", self),
        }

        let status = self.status();
        let body = match &self {
            Self::UpstreamSoapFault {
                code,
                string,
                detail,
            } => {
                if expose_soap_fault_detail {
                    json!({
                        "error": "upstream_soap_fault",
                        "message": self.to_string(),
                        "code": code,
                        "string": string,
                        "detail": detail,
                    })
                } else {
                    let truncated = truncate_chars(string, SOAP_FAULT_STRING_MAX);
                    json!({
                        "error": "upstream_soap_fault",
                        "message": format!("upstream returned SOAP Fault ({code})"),
                        "code": code,
                        "string": truncated,
                        // `detail` deliberately omitted from the
                        // client response — full contents live in
                        // the `tracing::warn!` above.
                    })
                }
            }
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

/// Byte-based truncation would slice mid-codepoint on multibyte
/// Estonian characters; use char count instead. Adds "… (truncated)"
/// only when trimming actually happened so short strings look normal.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}… (truncated)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), 65536).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn audit_h3_soap_fault_detail_stripped_by_default() {
        let err = XtrError::UpstreamSoapFault {
            code: "Server".into(),
            string: "backend unavailable".into(),
            detail: Some(json!({ "stack": "at internal.jsp:42" })),
        };
        let resp = err.into_response_with_options(false);
        let body = body_json(resp).await;
        assert!(body.get("detail").is_none(), "detail must not leak: {body}");
        assert_eq!(body["code"], "Server");
        assert_eq!(body["string"], "backend unavailable");
    }

    #[tokio::test]
    async fn audit_h3_long_faultstring_truncated() {
        let big = "x".repeat(500);
        let err = XtrError::UpstreamSoapFault {
            code: "Server".into(),
            string: big.clone(),
            detail: None,
        };
        let resp = err.into_response_with_options(false);
        let body = body_json(resp).await;
        let out = body["string"].as_str().unwrap();
        assert!(
            out.ends_with("… (truncated)"),
            "should end with truncation marker: {out}"
        );
        assert!(out.chars().count() < big.chars().count());
    }

    #[tokio::test]
    async fn audit_h3_opt_in_exposes_detail() {
        let err = XtrError::UpstreamSoapFault {
            code: "Server".into(),
            string: "x".repeat(300),
            detail: Some(json!({ "why": "diag" })),
        };
        let resp = err.into_response_with_options(true);
        let body = body_json(resp).await;
        assert_eq!(body["detail"], json!({ "why": "diag" }));
        // Full string returned, no truncation marker.
        assert!(!body["string"].as_str().unwrap().contains("truncated"));
    }
}
