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

    /// Method not allowed for this template. Emitted when a SOAP
    /// DSL (POST-only by contract) receives a GET/PUT/etc. — REST
    /// DSLs accept any method and never trigger this.
    #[error("method {method} not allowed for {group}/{service}")]
    MethodNotAllowed {
        method: String,
        group: String,
        service: String,
    },

    /// Audit v1 FN3 — inbound JSON body could not be parsed OR was
    /// not a top-level object. Emitted for SOAP DSL invocations so
    /// XTR cannot be used as an amplifier: attackers previously
    /// could send garbage on the XTR wire and force a real upstream
    /// call (potentially with the operator's mTLS identity) that
    /// returned 500. Post-fix: 400 is returned before any outbound
    /// call.
    #[error("invalid JSON body: {reason}")]
    InvalidJsonBody { reason: String },

    /// Audit LOG-v1 FN-LOG-3 — every outbound call was refused
    /// because XTR_OFFLINE is set. Test-safety / pentest-safety
    /// mode: no real upstream was contacted for this request.
    /// Response uses a non-standard 599 to make it unambiguous
    /// that the gate fired here (not on the upstream).
    #[error("outbound blocked: XTR_OFFLINE is set (test-safety mode)")]
    OfflineMode,

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
            Self::MethodNotAllowed { .. } => StatusCode::METHOD_NOT_ALLOWED,
            Self::InvalidJsonBody { .. } => StatusCode::BAD_REQUEST,
            // 599 is non-standard but distinctive — signals "the XTR
            // offline gate fired", not any upstream state.
            Self::OfflineMode => StatusCode::from_u16(599).unwrap(),
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
            Self::MethodNotAllowed { .. } => "method_not_allowed",
            Self::InvalidJsonBody { .. } => "invalid_json_body",
            Self::OfflineMode => "xtr_offline",
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

/// Audit v1 F-XTR-3 — cap on attacker-controlled path segments
/// (`group` / `service` / `method`) that get echoed back in
/// `TemplateNotFound` and `MethodNotAllowed` error bodies. Bounds the
/// response size an unauth caller can force from a single request so
/// XTR isn't usable as a response-amplification primitive.
pub const ECHOED_PATH_MAX: usize = 256;

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
                // Audit LOG-v1 FN-LOG-5 + RUNTIME FN2 residual: the
                // upstream picks these strings, so an attacker who can
                // force a specific fault picks what lands in our log.
                // Use `?` (Debug) instead of `%` (Display) so control
                // chars (CR/LF/ESC) render as escape sequences and
                // can't split the log line or smuggle ANSI into a
                // SIEM stream.
                tracing::warn!(
                    fault_code = ?code,
                    fault_string = ?string,
                    fault_detail = ?detail,
                    "upstream SOAP fault (full detail)"
                );
            }
            // Audit LOG-v1 FN-LOG-1 (HIGH): TemplateNotFound { group, service }
            // etc. embed user-controlled path segments. Using `{}` here
            // Display-formats those strings verbatim into the log line,
            // enabling CRLF log injection via `POST /x/y%0d%0aFAKE`.
            // Fix: use structured `?self` (Debug) — Rust's Debug on String
            // quotes and escapes control chars, so raw \r\n renders as
            // the literal escape sequence and cannot split the log line.
            _ => tracing::warn!(error.kind = %self.code(), error = ?self, "request failed"),
        }

        let status = self.status();
        let body = match &self {
            Self::UpstreamSoapFault {
                code,
                string,
                detail,
            } => {
                // Audit RUNTIME FN2 residual: even if the caller
                // wants raw fault detail (expose_soap_fault_detail=true),
                // control chars in the fault fields are never legitimate
                // — strip them before they land in the JSON response
                // body. Detail passes through unchanged since it's a
                // structured JSON subtree already; the risk is on the
                // free-form `code` / `string` strings.
                let sanitised_code = sanitize_fault_field(code);
                if expose_soap_fault_detail {
                    json!({
                        "error": "upstream_soap_fault",
                        "message": self.to_string(),
                        "code": sanitised_code,
                        "string": sanitize_fault_field(string),
                        "detail": detail,
                    })
                } else {
                    let cleaned = sanitize_fault_field(string);
                    let truncated = truncate_chars(&cleaned, SOAP_FAULT_STRING_MAX);
                    json!({
                        "error": "upstream_soap_fault",
                        "message": format!("upstream returned SOAP Fault ({sanitised_code})"),
                        "code": sanitised_code,
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
            // Audit v1 F-XTR-3: clip attacker-controlled path segments
            // so a caller cannot force an arbitrarily-large response
            // body by POST /aaaa…10MB/aaaa…10MB. Also emit the clipped
            // values as top-level fields so structured consumers can
            // pattern-match without parsing the free-form `message`.
            Self::TemplateNotFound { group, service } => {
                let group = truncate_chars(group, ECHOED_PATH_MAX);
                let service = truncate_chars(service, ECHOED_PATH_MAX);
                json!({
                    "error": self.code(),
                    "message": format!("template not found: {group}/{service}"),
                    "group": group,
                    "service": service,
                })
            }
            Self::MethodNotAllowed {
                method,
                group,
                service,
            } => {
                let method = truncate_chars(method, ECHOED_PATH_MAX);
                let group = truncate_chars(group, ECHOED_PATH_MAX);
                let service = truncate_chars(service, ECHOED_PATH_MAX);
                json!({
                    "error": self.code(),
                    "message": format!("method {method} not allowed for {group}/{service}"),
                    "method": method,
                    "group": group,
                    "service": service,
                })
            }
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

/// Audit RUNTIME FN2 residual / LOG FN-LOG-5 — a SOAP fault's
/// `code` / `faultstring` come verbatim from the upstream and are
/// echoed to the REST caller. A malicious upstream (or one whose
/// error path was smuggled via SSRF) can pack CRLF or ANSI escape
/// sequences into these fields to poison downstream log-shippers or
/// terminal renderers that display JSON error bodies. Well-formed
/// SOAP faults never carry raw control chars, so replacing them with
/// U+FFFD (REPLACEMENT CHARACTER) is loss-less for the legitimate
/// case and defensive for the malicious one. Tab is kept because
/// some legitimate multi-word error text uses it.
fn sanitize_fault_field(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\t' => c,
            c if c.is_control() => '\u{FFFD}',
            _ => c,
        })
        .collect()
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
    async fn audit_f_xtr_3_template_not_found_body_clips_long_group_and_service() {
        // An unauth caller sends POST /<10 KB>/<10 KB>. Without a
        // per-segment cap the response would be ~20 KB per request —
        // a small amplifier. Post-fix: each segment is capped and the
        // body stays bounded.
        let long_group = "g".repeat(10_000);
        let long_service = "s".repeat(10_000);
        let err = XtrError::TemplateNotFound {
            group: long_group.clone(),
            service: long_service.clone(),
        };
        let resp = err.into_response_with_options(false);
        let body = body_json(resp).await;
        let g = body["group"].as_str().unwrap();
        let s = body["service"].as_str().unwrap();
        assert!(
            g.chars().count() <= ECHOED_PATH_MAX + 16,
            "group not clipped: {} chars",
            g.chars().count()
        );
        assert!(
            s.chars().count() <= ECHOED_PATH_MAX + 16,
            "service not clipped: {} chars",
            s.chars().count()
        );
        assert!(g.ends_with("… (truncated)"));
        assert!(s.ends_with("… (truncated)"));
    }

    #[tokio::test]
    async fn audit_f_xtr_3_method_not_allowed_body_clips_all_three_fields() {
        let err = XtrError::MethodNotAllowed {
            method: "M".repeat(10_000),
            group: "G".repeat(10_000),
            service: "S".repeat(10_000),
        };
        let resp = err.into_response_with_options(false);
        let body = body_json(resp).await;
        for k in ["method", "group", "service"] {
            let v = body[k].as_str().unwrap();
            assert!(
                v.chars().count() <= ECHOED_PATH_MAX + 16,
                "{} not clipped: {} chars",
                k,
                v.chars().count()
            );
            assert!(v.ends_with("… (truncated)"), "{k} missing marker");
        }
    }

    #[tokio::test]
    async fn audit_f_xtr_3_short_paths_pass_through_unchanged() {
        // Regression: the clip must not add "… (truncated)" markers
        // to normal short values, only to actually-oversized ones.
        let err = XtrError::TemplateNotFound {
            group: "ariregister".into(),
            service: "lihtandmed_v3".into(),
        };
        let resp = err.into_response_with_options(false);
        let body = body_json(resp).await;
        assert_eq!(body["group"], "ariregister");
        assert_eq!(body["service"], "lihtandmed_v3");
        assert!(!body["message"].as_str().unwrap().contains("truncated"));
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

    #[tokio::test]
    async fn audit_fn2_faultstring_control_chars_replaced_with_u_fffd() {
        // A malicious upstream returns a fault with CRLF + NUL + ANSI
        // ESC packed into the faultstring. Without sanitisation, those
        // bytes would land verbatim in the JSON response body — where
        // a downstream terminal / log-shipper could misparse them.
        let err = XtrError::UpstreamSoapFault {
            code: "Server".into(),
            string: "auth failed\r\nFORGED-LINE\x00\x1b[31mred".into(),
            detail: None,
        };
        let resp = err.into_response_with_options(false);
        let body = body_json(resp).await;
        let out = body["string"].as_str().unwrap();
        assert!(!out.contains('\r'), "raw CR must not survive: {out:?}");
        assert!(!out.contains('\n'), "raw LF must not survive: {out:?}");
        assert!(!out.contains('\x00'), "raw NUL must not survive: {out:?}");
        assert!(
            !out.contains('\x1b'),
            "raw ANSI ESC must not survive: {out:?}"
        );
        // The visible message content survives; only control chars flip.
        assert!(out.contains("auth failed"));
        assert!(out.contains("FORGED-LINE"));
        assert!(out.contains("red"));
        assert!(out.contains('\u{FFFD}'), "expected replacement char");
    }

    #[tokio::test]
    async fn audit_fn2_faultcode_control_chars_replaced() {
        let err = XtrError::UpstreamSoapFault {
            code: "Sender\r\nX-Forged: 1".into(),
            string: "ok".into(),
            detail: None,
        };
        let resp = err.into_response_with_options(false);
        let body = body_json(resp).await;
        let code = body["code"].as_str().unwrap();
        assert!(!code.contains('\r'));
        assert!(!code.contains('\n'));
        assert!(code.contains("Sender"));
    }

    #[tokio::test]
    async fn audit_fn2_expose_detail_still_sanitises_fields() {
        // Even in expose_soap_fault_detail=true mode, control chars
        // are stripped — the flag is about detail visibility, not
        // giving the upstream a byte-transparent channel into the caller.
        let err = XtrError::UpstreamSoapFault {
            code: "Server\x1b[0m".into(),
            string: "err\r\n".into(),
            detail: Some(json!({ "trace": "at Foo:1" })),
        };
        let resp = err.into_response_with_options(true);
        let body = body_json(resp).await;
        assert!(!body["code"].as_str().unwrap().contains('\x1b'));
        assert!(!body["string"].as_str().unwrap().contains('\r'));
        assert_eq!(body["detail"], json!({ "trace": "at Foo:1" }));
    }

    #[test]
    fn sanitize_fault_field_preserves_tab_and_unicode() {
        // Tab is kept — legitimate multi-word error text uses it for
        // alignment. Unicode punctuation and accents pass through.
        let s = "field\tvalue — ümlaut ☑";
        assert_eq!(sanitize_fault_field(s), s);
    }

    #[test]
    fn sanitize_fault_field_replaces_all_c0_c1_control_chars() {
        // C0 (0x00-0x1F except \t) and DEL (0x7F).
        for c in (0u8..=0x1f).chain(std::iter::once(0x7fu8)) {
            if c == b'\t' {
                continue;
            }
            let s: String = std::iter::once(c as char).collect();
            let out = sanitize_fault_field(&s);
            assert_eq!(
                out, "\u{FFFD}",
                "expected 0x{c:02x} to become U+FFFD, got {out:?}"
            );
        }
    }
}
