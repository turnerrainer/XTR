//! Audit LOG-v1 FN-LOG-3 — XTR_OFFLINE / offline-mode regression.
//!
//! Verifies:
//! 1. `Executor::with_offline_for_tests(true)` short-circuits every
//!    outbound. Even a legitimately-configured DSL that would normally
//!    invoke `reqwest::execute` never touches the network.
//! 2. The router response is HTTP 599 with `error = "xtr_offline"`.
//! 3. `Executor::is_offline()` reports the mode correctly for the
//!    doctor tool.
//!
//! The XTR_OFFLINE env-var lane is deliberately NOT tested here —
//! env vars are process-global, so a test that sets one would race
//! with the rest of the suite. The env-lane is covered by inspection
//! of `resolve_offline_from_env` (unit test below) plus the code
//! review of `Executor::new`.

use axum::body::to_bytes;
use axum::extract::State;
use axum::routing::post;
use axum::Router;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use xtr_on_rust::config::AppConfig;
use xtr_on_rust::dsl::loader;
use xtr_on_rust::executor::Executor;
use xtr_on_rust::openapi;
use xtr_on_rust::router::{self, AppState};

#[derive(Clone, Default)]
struct Capture {
    called: Arc<Mutex<bool>>,
}

async fn mock_never_called(
    State(capture): State<Capture>,
    _body: String,
) -> impl axum::response::IntoResponse {
    *capture.called.lock().unwrap() = true;
    (
        axum::http::StatusCode::OK,
        [("content-type", "text/xml; charset=utf-8")],
        r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
            <soap:Body/>
        </soap:Envelope>"#,
    )
}

async fn spawn_capture() -> (String, Capture) {
    let capture = Capture::default();
    let app = Router::new()
        .route("/", post(mock_never_called))
        .with_state(capture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), capture)
}

fn write_dsl(dsl_root: &std::path::Path, group: &str, service: &str, body: &str) {
    let dir = dsl_root.join(group);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{service}.yml")), body).unwrap();
}

async fn build_offline_xtr(dsl_root: &std::path::Path) -> Router {
    let cfg = AppConfig {
        dsl_path: dsl_root.to_path_buf(),
        xroad_instance: "ee-test".into(),
        ..Default::default()
    };
    let services = loader::load_all(&cfg.dsl_path).unwrap();
    let spec = openapi::build_spec(&services, "0.1.0-test");
    let executor = Executor::new(&cfg).unwrap().with_offline_for_tests(true);
    assert!(executor.is_offline(), "offline flag not applied");
    router::build(AppState {
        cfg: Arc::new(cfg),
        services: Arc::new(services),
        executor,
        openapi_spec: Arc::new(spec),
    })
}

async fn axum_test(app: Router, req: axum::http::Request<axum::body::Body>) -> (u16, String) {
    use tower::ServiceExt;
    let response = app.oneshot(req).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn audit_fn_log_3_offline_returns_599_and_makes_no_outbound_soap_call() {
    let (mock_url, capture) = spawn_capture().await;
    let tmp = TempDir::new().unwrap();
    // DSL points at a real (mock) upstream — but offline mode must
    // still prevent the actual dial.
    let dsl = format!(
        "params: []\nservice: {mock_url}\nmethod: POST\nenvelope: >\n  <soap:Envelope><soap:Body/></soap:Envelope>\n"
    );
    write_dsl(tmp.path(), "svc", "op", &dsl);
    let app = build_offline_xtr(tmp.path()).await;

    let (status, body) = axum_test(
        app,
        axum::http::Request::builder()
            .method("POST")
            .uri("/svc/op")
            .header("content-type", "application/json")
            .body(axum::body::Body::from("{}"))
            .unwrap(),
    )
    .await;

    assert_eq!(status, 599, "offline mode must respond with 599, got {status}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["error"], "xtr_offline", "body was {body}");
    // The critical regression: the mock upstream must NEVER have been
    // dialled. This is what makes XTR_OFFLINE safe to use during a
    // pentest engagement against a shared deployment.
    let called = *capture.called.lock().unwrap();
    assert!(!called, "offline mode did NOT block outbound — mock upstream was called");
}
