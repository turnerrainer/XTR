//! Integration tests for the XTR_INTER_SERVICE_TOKEN bearer gate.
//! Traced from h2ck.me NEXT-TASKS v1 §T-8.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::Value;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;
use xtr_on_rust::{
    config::{AppConfig, Limits},
    dsl::loader,
    executor::Executor,
    openapi,
    router::{self, AppState},
};

fn write_dsl(dsl_root: &std::path::Path, group: &str, service: &str, body: &str) {
    let dir = dsl_root.join(group);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{service}.yml")), body).unwrap();
}

fn build_app_with_token(dsl_root: &std::path::Path, token: Option<&str>) -> Router {
    let cfg = AppConfig {
        dsl_path: dsl_root.to_path_buf(),
        xroad_instance: "ee-test".into(),
        limits: Limits::default(),
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
        inter_service_token: token.map(|s| Arc::new(s.to_string())),
    })
}

async fn send(app: Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&body_bytes).unwrap_or(Value::Null);
    (status, json)
}

fn dsl_service(mock_url: &str) -> String {
    format!("params: []\nservice: {mock_url}\nmethod: POST\nenvelope: <x/>\n")
}

#[tokio::test]
async fn gate_off_by_default_no_bearer_required() {
    // When XTR_INTER_SERVICE_TOKEN is unset (inter_service_token=None
    // in AppState), /:group/:service must accept unauthenticated
    // requests — backward-compat with 0.4.x.
    let tmp = TempDir::new().unwrap();
    write_dsl(tmp.path(), "ar", "svc", &dsl_service("https://x.invalid/"));
    let app = build_app_with_token(tmp.path(), None);
    let (status, _) = send(
        app,
        Request::builder()
            .method("POST")
            .uri("/ar/svc")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    // Not 401 — token gate is off. (Real upstream is unreachable so
    // the invoke returns 5xx, but that's a different layer; the
    // point here is "not 401".)
    assert_ne!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn gate_on_missing_bearer_returns_401_structured() {
    let tmp = TempDir::new().unwrap();
    write_dsl(tmp.path(), "ar", "svc", &dsl_service("https://x.invalid/"));
    let app = build_app_with_token(tmp.path(), Some("s3cret-32-bytes-of-hex-material"));
    let (status, body) = send(
        app,
        Request::builder()
            .method("POST")
            .uri("/ar/svc")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");
    // Message must mention the env var by name so operators debug
    // fast from the response body alone.
    let msg = body["message"].as_str().unwrap();
    assert!(
        msg.contains("XTR_INTER_SERVICE_TOKEN"),
        "message was: {msg}"
    );
}

#[tokio::test]
async fn gate_on_wrong_bearer_returns_401() {
    let tmp = TempDir::new().unwrap();
    write_dsl(tmp.path(), "ar", "svc", &dsl_service("https://x.invalid/"));
    let app = build_app_with_token(tmp.path(), Some("correct-token"));
    let (status, _) = send(
        app,
        Request::builder()
            .method("POST")
            .uri("/ar/svc")
            .header("content-type", "application/json")
            .header("authorization", "Bearer wrong-token")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn gate_on_correct_bearer_passes_through() {
    let tmp = TempDir::new().unwrap();
    write_dsl(tmp.path(), "ar", "svc", &dsl_service("https://x.invalid/"));
    let app = build_app_with_token(tmp.path(), Some("correct-token"));
    let (status, _) = send(
        app,
        Request::builder()
            .method("POST")
            .uri("/ar/svc")
            .header("content-type", "application/json")
            .header("authorization", "Bearer correct-token")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    // Passed the gate; downstream fails at the invalid upstream, but
    // the assertion is "not 401" — not-gated is the whole point.
    assert_ne!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn gate_never_applies_to_health() {
    let tmp = TempDir::new().unwrap();
    let app = build_app_with_token(tmp.path(), Some("correct-token"));
    let (status, _) = send(
        app,
        Request::builder()
            .method("GET")
            .uri("/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn gate_never_applies_to_api() {
    // /api is separately gated by observability.expose_openapi
    // (audit-v2 F-XTR-1). The bearer layer must not fire on it —
    // orchestrators / spec-crawlers use /api without auth headers.
    let tmp = TempDir::new().unwrap();
    let app = build_app_with_token(tmp.path(), Some("correct-token"));
    let (status, _) = send(
        app,
        Request::builder()
            .method("GET")
            .uri("/api")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn gate_on_bearer_without_prefix_returns_401() {
    // Header value that isn't `Bearer <token>` (e.g. bare token,
    // Basic auth, wrong scheme) must be rejected — no scheme-
    // sniffing that could accept a wrong shape.
    let tmp = TempDir::new().unwrap();
    write_dsl(tmp.path(), "ar", "svc", &dsl_service("https://x.invalid/"));
    let app = build_app_with_token(tmp.path(), Some("correct-token"));
    let (status, _) = send(
        app,
        Request::builder()
            .method("POST")
            .uri("/ar/svc")
            .header("content-type", "application/json")
            .header("authorization", "correct-token") // no "Bearer " prefix
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
