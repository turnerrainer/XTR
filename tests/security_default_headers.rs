//! Fleet stronghold §5.1 — every response carries the five default
//! security headers. Regression pin: adding a new route that skips
//! the middleware, or dropping the middleware entirely, must flip
//! these tests red.

use axum::body::to_bytes;
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

async fn axum_test(app: Router, req: axum::http::Request<axum::body::Body>) -> axum::response::Response {
    use tower::ServiceExt;
    app.oneshot(req).await.unwrap()
}

const EXPECTED_HEADERS: &[(&str, &str)] = &[
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

#[tokio::test]
async fn stronghold_5_1_health_response_carries_all_five_headers() {
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
    assert_eq!(resp.status().as_u16(), 200);
    for (name, expected) in EXPECTED_HEADERS {
        let actual = resp
            .headers()
            .get(*name)
            .unwrap_or_else(|| panic!("missing header: {name}"))
            .to_str()
            .unwrap();
        assert_eq!(actual, *expected, "wrong value for header {name}");
    }
}

#[tokio::test]
async fn stronghold_5_1_openapi_response_carries_all_five_headers() {
    let tmp = TempDir::new().unwrap();
    let app = build_xtr(tmp.path()).await;
    let resp = axum_test(
        app,
        axum::http::Request::builder()
            .method("GET")
            .uri("/api")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    for (name, _) in EXPECTED_HEADERS {
        assert!(
            resp.headers().get(*name).is_some(),
            "openapi response missing header {name}"
        );
    }
    // Drain body to satisfy hyper.
    let _ = to_bytes(resp.into_body(), 65_536).await;
}

#[tokio::test]
async fn stronghold_5_1_template_not_found_error_carries_all_five_headers() {
    // 404 path is a different Response type — assert the middleware
    // reaches it too, because error paths are exactly where a
    // misconfigured proxy tends to omit its own header set.
    let tmp = TempDir::new().unwrap();
    let app = build_xtr(tmp.path()).await;
    let resp = axum_test(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/nope/nothing")
            .header("content-type", "application/json")
            .body(axum::body::Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 404);
    for (name, _) in EXPECTED_HEADERS {
        assert!(
            resp.headers().get(*name).is_some(),
            "404 error response missing header {name}"
        );
    }
}
