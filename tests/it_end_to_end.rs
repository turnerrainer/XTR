//! End-to-end integration tests.
//!
//! Spins up an axum mock upstream on 127.0.0.1:0 that plays the
//! X-Road SS role (fixture responses per DSL), points a
//! test-mode DSL at it, and exercises the full request path:
//!   HTTP POST → DSL lookup → Handlebars expand → executor →
//!   XML → JSON translate → response.
//!
//! Task 007 (`epic-testing-infrastructure/`) will refine this
//! ad-hoc mock into a proper shared fixture layer.

use axum::extract::State;
use axum::routing::post;
use axum::Router;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use xtr_on_rust::{
    config::AppConfig, dsl::loader, executor::Executor, openapi,
    router::{self, AppState},
};

/// Captures the last inbound request the mock upstream saw so
/// tests can assert on outbound headers / body.
#[derive(Clone, Default)]
struct Capture {
    body: Arc<Mutex<Option<String>>>,
    content_type: Arc<Mutex<Option<String>>>,
}

async fn mock_handler(
    State(capture): State<Capture>,
    headers: axum::http::HeaderMap,
    body: String,
) -> impl axum::response::IntoResponse {
    *capture.body.lock().unwrap() = Some(body);
    *capture.content_type.lock().unwrap() = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    (
        axum::http::StatusCode::OK,
        [("content-type", "text/xml; charset=utf-8")],
        r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
            <soap:Header><messageId>abc-123</messageId></soap:Header>
            <soap:Body><result><name>Peeter Kärp</name></result></soap:Body>
        </soap:Envelope>"#,
    )
}

/// Spin up the mock upstream. Returns its bound URL + the
/// capture handle for post-hoc assertions.
async fn spawn_mock() -> (String, Capture) {
    let capture = Capture::default();
    let app = Router::new()
        .route("/", post(mock_handler))
        .with_state(capture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), capture)
}

/// Write a DSL file into `dsl_root/<group>/<service>.yml`.
fn write_dsl(dsl_root: &std::path::Path, group: &str, service: &str, body: &str) {
    let dir = dsl_root.join(group);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{service}.yml")), body).unwrap();
}

/// Assemble the XTR router with a given DSL directory. No SS
/// configured — every DSL must have `service:` set.
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

#[tokio::test]
async fn health_returns_ok() {
    let tmp = TempDir::new().unwrap();
    let app = build_xtr(tmp.path()).await;
    let resp = axum_test(app, axum::http::Request::builder()
        .method("GET")
        .uri("/health")
        .body(axum::body::Body::empty())
        .unwrap())
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json.unwrap(), json!({"status": "ok"}));
}

#[tokio::test]
async fn openapi_lists_loaded_services() {
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "ar",
        "svc",
        "params: [reg_code]\nservice: https://x\nmethod: POST\nenvelope: <x/>\n",
    );
    let app = build_xtr(tmp.path()).await;
    let resp = axum_test(app, axum::http::Request::builder()
        .method("GET")
        .uri("/api")
        .body(axum::body::Body::empty())
        .unwrap())
        .await;
    assert_eq!(resp.status, 200);
    let spec = resp.json.unwrap();
    assert!(spec["paths"]["/ar/svc"]["post"].is_object());
}

#[tokio::test]
async fn end_to_end_request_hits_upstream_and_translates_response() {
    let (mock_url, capture) = spawn_mock().await;
    let tmp = TempDir::new().unwrap();
    let dsl = format!(
        "params: [reg_code]\nservice: {mock_url}\nmethod: POST\nenvelope: >\n  <soap:Envelope><soap:Body><q><reg_code>{{{{reg_code}}}}</reg_code></q></soap:Body></soap:Envelope>\n"
    );
    write_dsl(tmp.path(), "ar", "lookup", &dsl);
    let app = build_xtr(tmp.path()).await;

    let resp = axum_test(app, axum::http::Request::builder()
        .method("POST")
        .uri("/ar/lookup")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"reg_code": "42"}"#))
        .unwrap())
        .await;

    assert_eq!(resp.status, 200);
    let body = resp.json.unwrap();

    // Response envelope translation: both body and headers present
    assert_eq!(body["body"]["result"]["name"], "Peeter Kärp");
    assert_eq!(body["headers"]["messageId"], "abc-123");

    // Outbound wire assertions (Task 003 scope — proves it works today)
    let outbound_body = capture.body.lock().unwrap().clone().unwrap();
    assert!(outbound_body.contains("<reg_code>42</reg_code>"));
    let ct = capture.content_type.lock().unwrap().clone().unwrap();
    assert!(ct.starts_with("text/xml"), "content-type was {ct}");
}

#[tokio::test]
async fn unknown_service_returns_404_with_structured_error() {
    let tmp = TempDir::new().unwrap();
    let app = build_xtr(tmp.path()).await;
    let resp = axum_test(app, axum::http::Request::builder()
        .method("POST")
        .uri("/nope/nothing")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap())
        .await;
    assert_eq!(resp.status, 404);
    let body = resp.json.unwrap();
    assert_eq!(body["error"], "template_not_found");
    assert!(body["message"].as_str().unwrap().contains("nope/nothing"));
}

#[tokio::test]
async fn params_outside_allowlist_are_silently_dropped() {
    let (mock_url, capture) = spawn_mock().await;
    let tmp = TempDir::new().unwrap();
    let dsl = format!(
        "params: [safe]\nservice: {mock_url}\nmethod: POST\nenvelope: <x>{{{{safe}}}}|{{{{evil}}}}</x>\n"
    );
    write_dsl(tmp.path(), "svc", "test", &dsl);
    let app = build_xtr(tmp.path()).await;

    let resp = axum_test(app, axum::http::Request::builder()
        .method("POST")
        .uri("/svc/test")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"safe": "OK", "evil": "PWN"}"#))
        .unwrap())
        .await;
    assert_eq!(resp.status, 200);

    // Envelope sent upstream: safe=OK, evil= (dropped)
    let outbound = capture.body.lock().unwrap().clone().unwrap();
    assert!(outbound.contains("<x>OK|</x>"), "outbound: {outbound}");
}

// ---------- Test harness helpers ----------

struct Resp {
    status: u16,
    json: Option<serde_json::Value>,
}

/// Wrapper around tower::ServiceExt::oneshot that returns the
/// status + parsed JSON body.
async fn axum_test(app: Router, req: axum::http::Request<axum::body::Body>) -> Resp {
    use tower::ServiceExt;
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).ok();
    Resp { status, json }
}
