//! Regression pin — slow-body attack must be cut off by the
//! handler-level `TimeoutLayer` (fleet stronghold §6.2, wired in
//! `router::build`).
//!
//! Threat: a client sends an HTTP `Content-Length` header that
//! promises N bytes of body but delivers them at 1 byte per second
//! (or never). Without a handler timeout, axum's `Bytes` extractor
//! keeps the future alive indefinitely — holding a connection slot
//! and a partial buffer, cheap DoS surface.
//!
//! `router::build` wraps every route in a `TimeoutLayer` at
//! `request_timeout_secs + 5`. Body extraction happens inside the
//! wrapped future, so the layer fires before body collection
//! completes → HTTP 504 GATEWAY_TIMEOUT and the connection is
//! released. This test opens a raw TCP connection so it can send
//! a truncated body — no HTTP client abstraction that would
//! auto-complete the body for us.
//!
//! Traced from h2ck.me NEXT-TASKS v1 §T-19.

use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
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

#[tokio::test]
async fn slow_body_returns_504_via_handler_timeout_layer() {
    // request_timeout_secs=1 → handler_timeout = 6s (1 + 5 grace).
    // Test waits ~8s for the response, well past the timeout.
    let tmp = TempDir::new().unwrap();
    write_dsl(
        tmp.path(),
        "ar",
        "lookup",
        "params: []\nservice: https://example.invalid/\nmethod: POST\nenvelope: <x/>\n",
    );
    let cfg = AppConfig {
        dsl_path: tmp.path().to_path_buf(),
        limits: Limits {
            max_request_bytes: 1024 * 1024,
            max_response_bytes: 1024 * 1024,
            request_timeout_secs: 1,
        },
        ..Default::default()
    };
    let services = loader::load_all(&cfg.dsl_path).unwrap();
    let spec = openapi::build_spec(&services, "0.1.0-test");
    let executor = Executor::new(&cfg).unwrap();
    let app = router::build(AppState {
        cfg: Arc::new(cfg),
        services: Arc::new(services),
        executor,
        openapi_spec: Arc::new(spec),
        inter_service_token: None,
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Raw HTTP/1.1 request: declare a 1000-byte body but never
    // send it. axum's body extractor will wait for the promised
    // bytes; the TimeoutLayer at 6s must cut in and return 504.
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST /ar/lookup HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: 1000\r\n\
         \r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    // Send 5 bytes, then stall — do NOT send the remaining 995.
    stream.write_all(b"{\"a\":").await.unwrap();
    stream.flush().await.unwrap();

    // Read the whole response (with an outer safety timeout so the
    // test itself can't hang forever if the layer regresses).
    let read_fut = async {
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        buf
    };
    let buf = tokio::time::timeout(Duration::from_secs(15), read_fut)
        .await
        .expect("no HTTP response within 15s — TimeoutLayer regressed");

    let head = std::str::from_utf8(&buf).unwrap_or("<non-utf8 response>");
    assert!(
        head.starts_with("HTTP/1.1 504"),
        "expected 504 GATEWAY_TIMEOUT status line, got:\n{head}"
    );
}
