//! Fleet strongholds §1.2 + §1.6 — access log + W3C traceparent
//! propagation.
//!
//! Runtime access-log assertions require capturing tracing output
//! which is more machinery than we want here; regression-pin the
//! response-side surface instead:
//! 1. Every response carries `traceparent` and `x-trace-id`.
//! 2. When the request supplies a valid `traceparent`, the response
//!    reuses the same trace-id (cross-service correlation).
//! 3. When the request supplies no `traceparent`, the response
//!    carries a freshly minted trace-id.
//! 4. A malformed inbound `traceparent` is ignored (fresh id).

use axum::Router;
use std::sync::Arc;
use tempfile::TempDir;
use xtr_on_rust::config::AppConfig;
use xtr_on_rust::dsl::loader;
use xtr_on_rust::executor::Executor;
use xtr_on_rust::openapi;
use xtr_on_rust::router::{self, AppState};

async fn build_xtr(dsl_root: &std::path::Path) -> Router {
    let cfg = AppConfig {
        dsl_path: dsl_root.to_path_buf(),
        xroad_instance: "ee-test".into(),
        ..Default::default()
    };
    let services = loader::load_all(&cfg.dsl_path).unwrap();
    let spec = openapi::build_spec(&services, "0.1.0-test");
    let executor = Executor::new(&cfg).unwrap();
    router::build(AppState {
        cfg: Arc::new(cfg),
        services: Arc::new(services),
        executor,
        openapi_spec: Arc::new(spec),
    })
}

async fn axum_test(
    app: Router,
    req: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    use tower::ServiceExt;
    app.oneshot(req).await.unwrap()
}

const KNOWN_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const KNOWN_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

#[tokio::test]
async fn stronghold_1_6_response_carries_traceparent_and_x_trace_id() {
    let tmp = TempDir::new().unwrap();
    let app = build_xtr(tmp.path()).await;
    let resp = axum_test(
        app,
        axum::http::Request::builder()
            .method("GET")
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert!(resp.headers().get("traceparent").is_some());
    assert!(resp.headers().get("x-trace-id").is_some());

    let tp = resp.headers().get("traceparent").unwrap().to_str().unwrap();
    let trace_id = resp.headers().get("x-trace-id").unwrap().to_str().unwrap();
    // 32-hex trace_id.
    assert_eq!(trace_id.len(), 32);
    assert!(trace_id.chars().all(|c| c.is_ascii_hexdigit()));
    // W3C traceparent shape.
    let parts: Vec<&str> = tp.split('-').collect();
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[0], "00");
    assert_eq!(parts[1], trace_id);
    assert_eq!(parts[2].len(), 16); // span_id
    assert_eq!(parts[3], "01");
}

#[tokio::test]
async fn stronghold_1_6_inbound_traceparent_is_honoured() {
    let tmp = TempDir::new().unwrap();
    let app = build_xtr(tmp.path()).await;
    let resp = axum_test(
        app,
        axum::http::Request::builder()
            .method("GET")
            .uri("/health")
            .header("traceparent", KNOWN_TRACEPARENT)
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    let trace_id = resp.headers().get("x-trace-id").unwrap().to_str().unwrap();
    assert_eq!(
        trace_id, KNOWN_TRACE,
        "inbound trace_id must be reused for cross-service correlation"
    );
}

#[tokio::test]
async fn stronghold_1_6_malformed_traceparent_is_ignored() {
    let tmp = TempDir::new().unwrap();
    let app = build_xtr(tmp.path()).await;
    let resp = axum_test(
        app,
        axum::http::Request::builder()
            .method("GET")
            .uri("/health")
            .header("traceparent", "totally not a traceparent")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    let trace_id = resp.headers().get("x-trace-id").unwrap().to_str().unwrap();
    assert_ne!(
        trace_id, KNOWN_TRACE,
        "malformed inbound header must not be trusted"
    );
    assert_eq!(trace_id.len(), 32);
    assert!(trace_id.chars().all(|c| c.is_ascii_hexdigit()));
}

#[tokio::test]
async fn stronghold_1_6_two_requests_get_distinct_trace_ids() {
    let tmp = TempDir::new().unwrap();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let app = build_xtr(tmp.path()).await;
        let resp = axum_test(
            app,
            axum::http::Request::builder()
                .method("GET")
                .uri("/health")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        ids.push(
            resp.headers()
                .get("x-trace-id")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string(),
        );
    }
    assert_ne!(ids[0], ids[1], "each request must mint a fresh trace_id");
}
