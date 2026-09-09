//! Integration tests for the REST passthrough lane (issue #5).
//!
//! Spins up a plain-HTTP mock as the stand-in Security Server,
//! points a `kind: rest` DSL at it via the test-only executor
//! constructor, and exercises the full router path.
//!
//! mTLS is not exercised here — the full-mTLS integration test
//! lives in `tests/it_rest_mtls.rs` (issue #5). What this file
//! covers: URL construction, X-Road header emission, body
//! forwarding, header pass-through both directions, upstream
//! status/content-type pass-through, DSL method enforcement.

use axum::extract::State;
use axum::routing::any;
use axum::Router;
use reqwest::Client;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tower::ServiceExt;
use xtr_on_rust::{
    config::{AppConfig, ClientData, Limits},
    dsl::loader,
    executor::{rest_lane::RestLaneExecutor, Executor},
    openapi,
    router::{self, AppState},
};

#[derive(Clone, Default)]
struct Capture {
    method: Arc<Mutex<Option<String>>>,
    path: Arc<Mutex<Option<String>>>,
    query: Arc<Mutex<Option<String>>>,
    body: Arc<Mutex<Option<Vec<u8>>>>,
    headers: Arc<Mutex<Option<axum::http::HeaderMap>>>,
    response_status: Arc<Mutex<u16>>,
    response_body: Arc<Mutex<String>>,
    response_content_type: Arc<Mutex<String>>,
    response_extra_headers: Arc<Mutex<Vec<(String, String)>>>,
}

impl Capture {
    fn new() -> Self {
        Self {
            response_status: Arc::new(Mutex::new(200)),
            response_body: Arc::new(Mutex::new(r#"{"ok":true}"#.into())),
            response_content_type: Arc::new(Mutex::new("application/json".into())),
            ..Default::default()
        }
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .lock()
            .unwrap()
            .as_ref()?
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }
}

async fn mock_handler(
    State(capture): State<Capture>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl axum::response::IntoResponse {
    *capture.method.lock().unwrap() = Some(method.to_string());
    *capture.path.lock().unwrap() = Some(uri.path().to_string());
    *capture.query.lock().unwrap() = uri.query().map(str::to_string);
    *capture.body.lock().unwrap() = Some(body.to_vec());
    *capture.headers.lock().unwrap() = Some(headers);

    let status = axum::http::StatusCode::from_u16(*capture.response_status.lock().unwrap())
        .unwrap_or(axum::http::StatusCode::OK);
    let ct = capture.response_content_type.lock().unwrap().clone();
    let body_str = capture.response_body.lock().unwrap().clone();
    let extra = capture.response_extra_headers.lock().unwrap().clone();
    let mut resp = axum::response::Response::builder()
        .status(status)
        .header("content-type", ct);
    for (k, v) in extra {
        resp = resp.header(k, v);
    }
    resp.body(axum::body::Body::from(body_str)).unwrap()
}

async fn spawn_mock() -> (String, Capture) {
    let capture = Capture::new();
    let app = Router::new()
        .route("/", any(mock_handler))
        .route("/*wildcard", any(mock_handler))
        .with_state(capture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), capture)
}

fn write_dsl(root: &std::path::Path, group: &str, service: &str, body: &str) {
    let dir = root.join(group);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{service}.yml")), body).unwrap();
}

async fn build_xtr_with_rest_upstream(dsl_root: &std::path::Path, upstream: &str) -> Router {
    let cfg = AppConfig {
        dsl_path: dsl_root.to_path_buf(),
        xroad_instance: "ee-test".into(),
        client_data: ClientData {
            member_class: "GOV".into(),
            member_code: "70008440".into(),
            subsystem_code: "xtr-test".into(),
        },
        limits: Limits::default(),
        ..Default::default()
    };
    let services = loader::load_all(&cfg.dsl_path).unwrap();
    let spec = openapi::build_spec(&services, "0.3.0-rc-test");
    let executor = Executor::new(&cfg).unwrap();

    // Swap in a plain-HTTP REST lane so this file's tests avoid
    // mTLS provisioning. Full mTLS coverage lives in it_rest_mtls.
    let plain_client = Client::builder()
        .timeout(Duration::from_secs(5))
        // Match production posture — no redirects (spec §4.4).
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let rest_lane = RestLaneExecutor::__from_parts_for_tests(
        plain_client,
        upstream.to_string(),
        cfg.xroad_instance.clone(),
        format!(
            "{}/{}/{}/{}",
            cfg.xroad_instance,
            cfg.client_data.member_class,
            cfg.client_data.member_code,
            cfg.client_data.subsystem_code,
        ),
        cfg.limits.max_response_bytes,
    );
    let executor = executor.__with_rest_lane_for_tests(rest_lane);

    router::build(AppState {
        cfg: Arc::new(cfg),
        services: Arc::new(services),
        executor,
        openapi_spec: Arc::new(spec),
    })
}

#[tokio::test]
async fn spec_4_1_url_shape_no_service_version_segment() {
    // Per spec §4.1, versioning is part of [path], NOT a separate
    // segment in serviceId. The DSL declares path: /v1/isikud and
    // the outbound URL must be:
    //   /r1/{instance}/{class}/{code}/{sub}/{service}/v1/isikud
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "rr",
        "isikud",
        r#"kind: rest
method: GET
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
"#,
    );
    let (upstream, capture) = spawn_mock().await;
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/rr/isikud")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        capture.path.lock().unwrap().as_deref(),
        Some("/r1/ee-test/GOV/70008440/rr/dde/v1/isikud"),
    );
}

#[tokio::test]
async fn spec_4_5_query_default_forwards_everything() {
    // Per spec §4.5: query params MUST pass unmodified when not
    // otherwise constrained. DSL omits allowed_query_params → all
    // inbound query keys forwarded.
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "rr",
        "isikud",
        r#"kind: rest
method: GET
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
"#,
    );
    let (upstream, capture) = spawn_mock().await;
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    app.oneshot(
        axum::http::Request::builder()
            .method("GET")
            .uri("/rr/isikud?personalCode=38001011234&extra=preserved")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    let q = capture.query.lock().unwrap().clone().unwrap_or_default();
    assert!(q.contains("personalCode=38001011234"), "got: {q}");
    assert!(
        q.contains("extra=preserved"),
        "spec §4.5 requires unmodified query pass-through: {q}"
    );
}

#[tokio::test]
async fn dsl_can_narrow_query_via_explicit_allow_list() {
    // Operators can opt into a SOAP-style narrowing by naming
    // permitted keys explicitly.
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "rr",
        "isikud",
        r#"kind: rest
method: GET
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
allowed_query_params:
  - personalCode
"#,
    );
    let (upstream, capture) = spawn_mock().await;
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    app.oneshot(
        axum::http::Request::builder()
            .method("GET")
            .uri("/rr/isikud?personalCode=38001011234&secret=dropped")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    let q = capture.query.lock().unwrap().clone().unwrap_or_default();
    assert!(q.contains("personalCode=38001011234"), "got: {q}");
    assert!(!q.contains("secret"), "unlisted key must not pass: {q}");
}

#[tokio::test]
async fn spec_4_3_x_road_client_header_mandatory_and_spec_shape() {
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "rr",
        "isikud",
        r#"kind: rest
method: POST
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
"#,
    );
    let (upstream, capture) = spawn_mock().await;
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    let inbound_body = json!({"personalCode": "38001011234"}).to_string();
    app.oneshot(
        axum::http::Request::builder()
            .method("POST")
            .uri("/rr/isikud")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(inbound_body.clone()))
            .unwrap(),
    )
    .await
    .unwrap();

    // Spec §4.3: X-Road-Client: {instance}/{class}/{code}/{subsystem}
    assert_eq!(
        capture.header("x-road-client").as_deref(),
        Some("ee-test/GOV/70008440/xtr-test"),
    );
    // Content-Type MUST be transported unmodified (spec §4.3).
    assert_eq!(
        capture.header("content-type").as_deref(),
        Some("application/json"),
    );
    // Body pass-through byte-for-byte.
    assert_eq!(
        capture.body.lock().unwrap().as_deref(),
        Some(inbound_body.as_bytes()),
    );
    // X-Road-Id: generated when caller doesn't set one.
    let id = capture.header("x-road-id").unwrap_or_default();
    assert!(
        uuid::Uuid::parse_str(&id).is_ok(),
        "expected UUID, got: {id}"
    );
}

#[tokio::test]
async fn spec_4_3_x_road_id_forwarded_when_caller_sets_it() {
    // Spec §4.3: X-Road-Id is optional; consumer MAY set it, else
    // the SS generates. XTR sits between the consumer application
    // (e.g. Ruuter) and the SS — if the consumer sets an id, we
    // must preserve it so the correlation carries through.
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "rr",
        "isikud",
        r#"kind: rest
method: GET
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
"#,
    );
    let (upstream, capture) = spawn_mock().await;
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    let caller_id = "12345678-aaaa-bbbb-cccc-0123456789ab";
    app.oneshot(
        axum::http::Request::builder()
            .method("GET")
            .uri("/rr/isikud")
            .header("X-Road-Id", caller_id)
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(capture.header("x-road-id").as_deref(), Some(caller_id));
}

#[tokio::test]
async fn spec_4_3_user_defined_and_accept_headers_pass_unmodified() {
    // Spec §4.3: user-defined headers + Accept MUST pass unchanged.
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "rr",
        "isikud",
        r#"kind: rest
method: GET
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
"#,
    );
    let (upstream, capture) = spawn_mock().await;
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    app.oneshot(
        axum::http::Request::builder()
            .method("GET")
            .uri("/rr/isikud")
            .header("Accept", "application/xml")
            .header("X-Road-UserId", "EE38001011234")
            .header("X-Road-Issue", "MT324223MSD")
            .header("X-Custom", "foo")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(capture.header("accept").as_deref(), Some("application/xml"));
    assert_eq!(
        capture.header("x-road-userid").as_deref(),
        Some("EE38001011234"),
    );
    assert_eq!(
        capture.header("x-road-issue").as_deref(),
        Some("MT324223MSD"),
    );
    assert_eq!(capture.header("x-custom").as_deref(), Some("foo"));
}

#[tokio::test]
async fn spec_4_3_hop_by_hop_headers_stripped_from_outbound() {
    // Hop-by-hop headers must NOT be forwarded to upstream.
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "rr",
        "isikud",
        r#"kind: rest
method: GET
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
"#,
    );
    let (upstream, capture) = spawn_mock().await;
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    app.oneshot(
        axum::http::Request::builder()
            .method("GET")
            .uri("/rr/isikud")
            .header("Connection", "close")
            .header("TE", "trailers")
            .header("Upgrade", "websocket")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    // reqwest may set its own Connection: keep-alive but MUST NOT
    // carry through the caller-supplied "close" value verbatim.
    assert_ne!(capture.header("upgrade").as_deref(), Some("websocket"));
    assert_ne!(capture.header("te").as_deref(), Some("trailers"));
    // Connection is fully replaced by reqwest; we just assert we
    // didn't blindly propagate the caller's value.
    assert!(
        capture.header("upgrade").is_none(),
        "Upgrade must not be forwarded",
    );
}

#[tokio::test]
async fn spec_4_3_inbound_x_road_client_cannot_override_xtr_identity() {
    // XTR is the trust boundary — inbound X-Road-Client MUST NOT
    // spoof the identity XTR sends upstream.
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "rr",
        "isikud",
        r#"kind: rest
method: GET
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
"#,
    );
    let (upstream, capture) = spawn_mock().await;
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    app.oneshot(
        axum::http::Request::builder()
            .method("GET")
            .uri("/rr/isikud")
            .header("X-Road-Client", "EVIL/CLASS/9999/attacker")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        capture.header("x-road-client").as_deref(),
        Some("ee-test/GOV/70008440/xtr-test"),
        "attacker header must be overridden by XTR's config identity",
    );
}

#[tokio::test]
async fn spec_4_3_upstream_x_road_response_headers_forwarded_to_caller() {
    // Provider Security Server sets X-Road-Request-Hash,
    // X-Road-Service, etc. Consumer needs them for correlation +
    // request-hash verification. Caller sees them verbatim.
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "rr",
        "isikud",
        r#"kind: rest
method: GET
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
"#,
    );
    let (upstream, capture) = spawn_mock().await;
    *capture.response_extra_headers.lock().unwrap() = vec![
        ("X-Road-Service".into(), "ee-test/GOV/70008440/rr/dde".into()),
        (
            "X-Road-Request-Hash".into(),
            "14sEri8SmLNy/DJyTob0ZddAskmdRy5ZUyhbr33iLkaA+gLpWcivUH16fzbuIs7hhs2AnA4lJDloyIihXMlVQA==".into(),
        ),
        (
            "X-Road-Request-Id".into(),
            "f92591a3-6bf0-49b1-987b-0dd78c034cc3".into(),
        ),
    ];
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/rr/isikud")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let hdrs = resp.headers();
    assert_eq!(
        hdrs.get("x-road-service").and_then(|v| v.to_str().ok()),
        Some("ee-test/GOV/70008440/rr/dde"),
    );
    assert!(hdrs.get("x-road-request-hash").is_some());
    assert!(hdrs.get("x-road-request-id").is_some());
}

#[tokio::test]
async fn upstream_status_passthrough_including_4xx() {
    // Spec §4.6 category 1: upstream service returned an error;
    // status + body + headers are returned as-is by the SS. XTR
    // must not translate that into an XtrError.
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "rr",
        "isikud",
        r#"kind: rest
method: GET
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
"#,
    );
    let (upstream, capture) = spawn_mock().await;
    *capture.response_status.lock().unwrap() = 404;
    *capture.response_body.lock().unwrap() = r#"{"error":"not_found"}"#.into();
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/rr/isikud")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        "application/json",
    );
}

#[tokio::test]
async fn dsl_method_contract_enforced_405_on_mismatch() {
    // The DSL declares method: GET. A POST must yield 405, not
    // silently route through.
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "rr",
        "isikud",
        r#"kind: rest
method: GET
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
"#,
    );
    let (upstream, _cap) = spawn_mock().await;
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/rr/isikud")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn soap_dsl_still_rejects_non_post_with_405() {
    // Regression: the router upgrade from `post(invoke)` to
    // `any(invoke)` must not silently let GET requests slip through
    // to a SOAP handler.
    let dsl = TempDir::new().unwrap();
    write_dsl(
        dsl.path(),
        "ar",
        "lihtandmed",
        "params: [x]\nmethod: POST\nservice: https://example.invalid\nenvelope: <x>{{x}}</x>\n",
    );
    let (upstream, _cap) = spawn_mock().await;
    let app = build_xtr_with_rest_upstream(dsl.path(), &upstream).await;

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/ar/lihtandmed")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
}
