//! Fleet strongholds §1.2 (access log) + §1.6 (W3C traceparent).
//!
//! One INFO line per HTTP request with method, matched route pattern,
//! status, duration, and a trace_id. Trace_id sources:
//!
//! 1. Inbound `traceparent` header (W3C Trace Context) — extract the
//!    32-hex trace-id segment.
//! 2. Otherwise: mint a fresh v4 UUID.
//!
//! Response headers set:
//! - `traceparent: 00-<trace_id>-<span_id>-01`
//! - `x-trace-id: <trace_id>` (lowercased hex, no dashes)
//!
//! The matched route pattern (e.g. `/:group/:service`) is used
//! instead of the raw URI so log-cardinality stays low — one
//! entry per shape, not per attacker-chosen path.
//!
//! Format for user-controlled fields uses structured `?path`
//! (Debug) so control chars can't split the log line — see
//! Audit LOG-v1 FN-LOG-1 for the equivalent hardening on
//! error-path logs.

use axum::extract::{MatchedPath, Request};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use std::time::Instant;
use uuid::Uuid;

/// Extract the trace-id segment of a W3C `traceparent` header, or
/// `None` if the value is malformed. Format: `<ver>-<trace>-<span>-<flags>`
/// where <trace> is 32 lowercase hex chars. This is intentionally
/// strict — a spec-violating inbound is not honoured.
fn extract_trace_id(traceparent: &str) -> Option<String> {
    let parts: Vec<&str> = traceparent.split('-').collect();
    if parts.len() != 4 {
        return None;
    }
    let trace = parts[1];
    if trace.len() != 32 || !trace.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(trace.to_ascii_lowercase())
}

fn new_trace_id() -> String {
    Uuid::new_v4().simple().to_string()
}

fn new_span_id() -> String {
    // 16-hex; take the first half of a fresh UUID.
    let uuid = Uuid::new_v4().simple().to_string();
    uuid[..16].to_string()
}

pub async fn apply(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().clone();

    // Prefer the matched-path pattern (bounded cardinality) over the
    // full URI (unbounded, attacker-controlled).
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "<unmatched>".to_string());

    let trace_id = req
        .headers()
        .get("traceparent")
        .and_then(|h| h.to_str().ok())
        .and_then(extract_trace_id)
        .unwrap_or_else(new_trace_id);

    let span_id = new_span_id();

    let mut response = next.run(req).await;

    let status = response.status().as_u16();
    let duration_us = start.elapsed().as_micros() as u64;

    let traceparent = format!("00-{trace_id}-{span_id}-01");
    if let Ok(v) = HeaderValue::from_str(&traceparent) {
        response.headers_mut().insert("traceparent", v);
    }
    if let Ok(v) = HeaderValue::from_str(&trace_id) {
        response.headers_mut().insert("x-trace-id", v);
    }

    // Debug-format the route pattern so any accidental control chars
    // in an operator-supplied route can't split the line. The value
    // should always be safe (compile-time literals) but the log-side
    // hardening is free.
    tracing::info!(
        method = %method,
        route = ?route,
        status,
        duration_us,
        trace_id = %trace_id,
        "http_request_completed"
    );

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_trace_id_valid_header() {
        // Well-formed W3C traceparent.
        let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        assert_eq!(
            extract_trace_id(tp),
            Some("4bf92f3577b34da6a3ce929d0e0e4736".into())
        );
    }

    #[test]
    fn extract_trace_id_uppercase_lowered() {
        let tp = "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01";
        assert_eq!(
            extract_trace_id(tp),
            Some("4bf92f3577b34da6a3ce929d0e0e4736".into())
        );
    }

    #[test]
    fn extract_trace_id_wrong_segment_count() {
        assert_eq!(extract_trace_id("00-4bf92f3577b34da6a3ce929d0e0e4736"), None);
    }

    #[test]
    fn extract_trace_id_wrong_trace_length() {
        assert_eq!(extract_trace_id("00-4bf92f-00f067aa0ba902b7-01"), None);
    }

    #[test]
    fn extract_trace_id_non_hex_trace() {
        assert_eq!(
            extract_trace_id("00-not-hex-here-still-32chars----g-00f067aa0ba902b7-01"),
            None
        );
    }

    #[test]
    fn new_trace_id_is_32_hex() {
        let id = new_trace_id();
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
