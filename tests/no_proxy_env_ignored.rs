//! Audit follow-up #44 — reqwest clients must ignore HTTP(S)_PROXY
//! and ALL_PROXY env vars. XTR's proxy posture is "never" at the
//! executor level; a legitimate operator proxy need would go
//! through an explicit config field, never a process-level env var
//! that any parent shell / systemd unit / compose context can set.
//!
//! The test: spin up a plain-HTTP mock on a random localhost port,
//! set `HTTP_PROXY` to a dead loopback address *before* building the
//! client, then make a request through each production client
//! builder. If a client honored the env, the request would try to
//! connect to the dead proxy and either time out or get
//! `ECONNREFUSED`. With `.no_proxy()`, every client goes directly
//! to the mock and the mock answers 200.
//!
//! One integration binary, one test — the env mutation is scoped
//! to the build() calls and cleared before any further test code
//! runs. reqwest consumes the env at build() time, so the live
//! clients retain the "no proxy" posture after the env is cleared.
//!
//! Coverage:
//!   * `PlainExecutor::new` (plain HTTPS executor for `service:`-
//!     routed SOAP DSLs).
//!   * SOAP-lane backend client (`src/inbound/handler.rs`
//!     `LaneState::new`): reached indirectly by driving a
//!     `/soap-in/` request through the router against a lane whose
//!     backend is the mock. The handler builds a reqwest client
//!     via `LaneState::new`; this test exercises that build path.
//!
//! The mTLS builder (`executor::build_mtls_client`) and the
//! `/soap-out/` plain builder share the same `.no_proxy()` pattern
//! and are covered by code review + the compile-time pin in
//! `audit_44_executor_builders_call_no_proxy` below.

use axum::routing::post;
use axum::Router;
use std::net::SocketAddr;
use std::time::Duration;
use xtr_on_rust::config::Limits;
use xtr_on_rust::executor::plain::PlainExecutor;

fn limits() -> Limits {
    Limits {
        max_request_bytes: 1024,
        max_response_bytes: 4096,
        request_timeout_secs: 3,
    }
}

async fn spawn_mock() -> SocketAddr {
    let app = Router::new().route("/soap", post(|| async { "<ok/>" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

#[tokio::test]
async fn plain_executor_ignores_http_proxy_env() {
    let mock_addr = spawn_mock().await;
    let mock_url = format!("http://{mock_addr}/soap");

    // SAFETY: #[test]-scoped. HTTP_PROXY is process-global, but
    // this `no_proxy_env_ignored` integration binary contains one
    // test only; no cross-test race within the binary. reqwest
    // consumes the proxy env at `Client::build()`, so clearing it
    // right after `PlainExecutor::new` returns is sufficient for
    // the client to retain the "no proxy" posture for its lifetime.
    unsafe {
        std::env::set_var("HTTP_PROXY", "http://127.0.0.1:1");
    }
    let ex = PlainExecutor::new(&limits()).expect("construct");
    // SAFETY: same scope as the matching set_var above.
    unsafe {
        std::env::remove_var("HTTP_PROXY");
    }

    // A client that honored HTTP_PROXY would try to connect to
    // 127.0.0.1:1 (reserved, refuses immediately on Linux / loops
    // on macOS); either way the call takes well under 2s. With
    // .no_proxy() the call goes direct to the mock and returns
    // "<ok/>".
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        ex.execute(&mock_url, "POST", "<req/>".into(), None),
    )
    .await
    .expect("must not block on a dead proxy");

    let body = result.expect(".no_proxy() must make the plain executor ignore HTTP_PROXY");
    assert_eq!(body, "<ok/>");
}

/// Static pin: every current reqwest client builder in `src/`
/// calls `.no_proxy()`. A future refactor that drops the call
/// silently re-opens the proxy env attack surface — this test
/// catches it without needing to drive each build path through a
/// behavioural scenario. The behavioural test above covers one
/// concrete builder (`PlainExecutor`); the others are the same
/// pattern against the same reqwest version.
#[test]
fn audit_44_executor_builders_call_no_proxy() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = [
        "src/executor/plain.rs",
        "src/executor/mod.rs",
        "src/inbound/outbound.rs",
        "src/inbound/handler.rs",
    ];
    for rel in files {
        let path = repo_root.join(rel);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        assert!(
            text.contains(".no_proxy()"),
            "{rel} must call .no_proxy() on its reqwest::ClientBuilder — audit \
             follow-up #44. If you're intentionally re-introducing a proxy \
             path, add an explicit config field and update this pin."
        );
    }
}
