//! Inter-service bearer token gate — fleet-wide pattern.
//!
//! When `XTR_INTER_SERVICE_TOKEN` is set at boot, every request to
//! `/:group/:service` must carry `Authorization: Bearer <TOKEN>`.
//! Missing or wrong → HTTP 401 with a structured JSON error.
//! `/health` and `/api` are unconditionally exempt: the former is
//! a liveness probe pulled by orchestrators without an auth
//! configuration, the latter is separately gated by
//! `observability.expose_openapi` (audit-v2 F-XTR-1).
//!
//! Default off (token = None) preserves 0.4.x behaviour — XTR is
//! commonly deployed behind Ruuter, which is the intended caller
//! auth boundary in that layout. Operators exposing XTR directly
//! (public/hostile networks) MUST set the token per
//! `SECURITY.md` §"Standalone deployment hardening".
//!
//! Token equality uses `subtle::ConstantTimeEq` so a timing side-
//! channel doesn't leak the correct prefix byte-by-byte. Length
//! mismatch bails early — length is not a security-critical secret
//! (an attacker who can guess the length can just try each length
//! independently).
//!
//! Traced from h2ck.me NEXT-TASKS v1 §T-8. Public-launch prereq.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// Env var read once at boot. `None` if unset or empty (both
/// suppress the gate). Trimmed of whitespace so a stray `\n` from
/// `docker exec ... echo` piping doesn't create a token that fails
/// every subsequent comparison.
pub fn load_from_env() -> Option<Arc<String>> {
    match std::env::var("XTR_INTER_SERVICE_TOKEN") {
        Ok(v) => {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(Arc::new(trimmed.to_string()))
            }
        }
        Err(_) => None,
    }
}

/// Middleware. Applied to `/:group/:service` only — `/health` and
/// `/api` are on different route branches and never see this layer.
/// State-carried token is captured in a closure at router-build
/// time via `middleware::from_fn_with_state`.
pub async fn apply(
    axum::extract::State(token): axum::extract::State<Option<Arc<String>>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let Some(expected) = token else {
        return next.run(req).await;
    };
    let presented = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    match presented {
        Some(t) if constant_time_eq(t.as_bytes(), expected.as_bytes()) => next.run(req).await,
        _ => unauthorized(),
    }
}

/// Constant-time bytes compare. Length mismatch bails early — the
/// security-critical property is that a MATCHING-length wrong token
/// doesn't leak "correct prefix length" via timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "unauthorized",
            "message": "missing or invalid Authorization: Bearer token \
                        (XTR_INTER_SERVICE_TOKEN gate is active)",
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_equal_slices() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(&[0u8; 64], &[0u8; 64]));
    }

    #[test]
    fn constant_time_eq_rejects_length_mismatch() {
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
        assert!(!constant_time_eq(b"", b"a"));
    }

    #[test]
    fn constant_time_eq_rejects_content_mismatch() {
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"xyz"));
    }

    #[test]
    fn load_from_env_returns_none_when_unset() {
        // SAFETY: single-threaded per #[test]; env var name unique
        // to this test — no cross-test contention.
        unsafe {
            std::env::remove_var("XTR_INTER_SERVICE_TOKEN");
        }
        assert!(load_from_env().is_none());
    }

    #[test]
    fn load_from_env_treats_empty_and_whitespace_as_none() {
        // SAFETY: as above — unique env-var name for this test path.
        unsafe {
            std::env::set_var("XTR_INTER_SERVICE_TOKEN", "");
        }
        assert!(load_from_env().is_none());
        // SAFETY: as above.
        unsafe {
            std::env::set_var("XTR_INTER_SERVICE_TOKEN", "   \n\t ");
        }
        assert!(load_from_env().is_none());
        // SAFETY: as above — cleanup after test to avoid leaking
        // state into a subsequent test that reads the same var.
        unsafe {
            std::env::remove_var("XTR_INTER_SERVICE_TOKEN");
        }
    }
}
