//! Fleet stronghold §5.1 — default security headers on every response.
//!
//! Cheap defense-in-depth: XTR is normally reached via a reverse
//! proxy that would set its own headers, but a dev deploy without
//! the proxy or a misconfigured proxy could omit them. The bytes on
//! each response are a few hundred; the cost is negligible.
//!
//! Headers set:
//! - `Content-Security-Policy: default-src 'none'; frame-ancestors 'none'`
//!   XTR serves JSON, never HTML — the strictest default is correct.
//! - `Strict-Transport-Security: max-age=63072000; includeSubDomains; preload`
//!   Standard 2-year HSTS. Ignored on plain-HTTP responses by browsers;
//!   surfaces when a browser accidentally lands on /health via TLS.
//! - `X-Frame-Options: DENY`
//! - `X-Content-Type-Options: nosniff`
//! - `Referrer-Policy: no-referrer`
//!
//! The middleware never overwrites a header set upstream — a REST
//! DSL that passes through a header of the same name from the
//! upstream keeps that value. This is important for the REST
//! passthrough lane (`X-Content-Type-Options` may already be set
//! by the X-Road upstream).

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

const SECURITY_HEADERS: &[(&str, &str)] = &[
    (
        "content-security-policy",
        "default-src 'none'; frame-ancestors 'none'",
    ),
    (
        "strict-transport-security",
        "max-age=63072000; includeSubDomains; preload",
    ),
    ("x-frame-options", "DENY"),
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "no-referrer"),
];

pub async fn apply(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        // `HeaderName::from_static` requires lowercase; each entry
        // above is already lowercase. Skip on any downstream that
        // has already set the header (REST passthrough case).
        if let Ok(hn) = HeaderName::from_bytes(name.as_bytes()) {
            if h.contains_key(&hn) {
                continue;
            }
            if let Ok(hv) = HeaderValue::from_str(value) {
                h.insert(hn, hv);
            }
        }
    }
    response
}
