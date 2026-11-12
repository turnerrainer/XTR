//! Raw-XML lanes end-to-end: peer → XTR router → real HTTP JSON backend
//! (inbound) and caller → XTR router → real HTTP "peer" (outbound).
//! Uses the production router (`router::build_with`) so the shared
//! middleware stack is included.

use axum::body::{to_bytes, Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tower::ServiceExt;
use xtr_on_rust::config::AppConfig;
use xtr_on_rust::dsl::loader;
use xtr_on_rust::executor::Executor;
use xtr_on_rust::openapi;
use xtr_on_rust::router::{self, AppState};
use xtr_on_rust::xmllane;

const GLOBAL_XSD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:erru"
    xmlns="urn:erru" elementFormDefault="qualified">
  <xs:complexType name="HeaderT">
    <xs:attribute name="technicalId" type="xs:string" use="required"/>
    <xs:attribute name="workflowId" type="xs:string" use="required"/>
    <xs:attribute name="to" type="xs:string"/>
  </xs:complexType>
</xs:schema>"#;

const MSG_XSD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:erru"
    xmlns="urn:erru" elementFormDefault="qualified">
  <xs:include schemaLocation="Global_Types.xsd"/>
  <xs:element name="Notify_Request"><xs:complexType><xs:sequence>
    <xs:element name="Header" type="HeaderT"/>
    <xs:element name="Body"><xs:complexType><xs:sequence>
      <xs:element name="TransportManager" maxOccurs="unbounded"><xs:complexType>
        <xs:attribute name="firstName" type="xs:string"/>
        <xs:attribute name="familyName" type="xs:string"/>
      </xs:complexType></xs:element>
    </xs:sequence><xs:attribute name="businessCaseId" type="xs:string"/></xs:complexType></xs:element>
  </xs:sequence></xs:complexType></xs:element>
  <xs:element name="Notify_Acknowledgement"><xs:complexType><xs:sequence>
    <xs:element name="Header" type="HeaderT"/>
    <xs:element name="Body"><xs:complexType><xs:attribute name="statusCode" type="xs:string"/></xs:complexType></xs:element>
  </xs:sequence></xs:complexType></xs:element>
  <xs:element name="Other_Request"><xs:complexType><xs:sequence>
    <xs:element name="Header" type="HeaderT"/>
  </xs:sequence></xs:complexType></xs:element>
</xs:schema>"#;

const NOTIFY_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Notify_Request xmlns="urn:erru"><Header technicalId="t-1" workflowId="w-1" to="EE"/><Body businessCaseId="b-1"><TransportManager firstName="Anna" familyName="Tamm"/><TransportManager firstName="Jaan" familyName="Kask"/></Body></Notify_Request>"#;

type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// JSON backend: records `(message, payload)`; `statusCode` in the
/// request header `Header/@technicalId` picks the answer.
async fn spawn_backend() -> (String, Seen) {
    let seen: Seen = Arc::default();
    async fn handle(
        State(seen): State<Seen>,
        axum::extract::Path(msg): axum::extract::Path<String>,
        Json(v): Json<Value>,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        let tid = v["request"]["Header"]["@technicalId"]
            .as_str()
            .unwrap_or("")
            .to_string();
        seen.lock().unwrap().push((msg, v));
        match tid.as_str() {
            "dup" => (StatusCode::CONFLICT, Json(json!({"code": "conflict"}))).into_response(),
            "boom" => (StatusCode::INTERNAL_SERVER_ERROR, "db password leaked").into_response(),
            "nf" => (StatusCode::NOT_FOUND, "no such route /secret/path").into_response(),
            "big" => (StatusCode::CONFLICT, "x".repeat(1000)).into_response(),
            _ => StatusCode::NO_CONTENT.into_response(),
        }
    }
    let app = Router::new()
        .route("/b/:msg", post(handle))
        .with_state(seen.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}/b"), seen)
}

type Raw = Arc<Mutex<Vec<(HeaderMap, Vec<u8>)>>>;

/// Raw-XML "peer" / forward target: records headers and exact bytes,
/// answers with `reply` (status, body).
async fn spawn_peer(status: u16, reply: &'static str) -> (String, Raw) {
    let raw: Raw = Arc::default();
    async fn handle(
        State((raw, status, reply)): State<(Raw, u16, &'static str)>,
        headers: HeaderMap,
        body: Bytes,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        raw.lock().unwrap().push((headers, body.to_vec()));
        (
            StatusCode::from_u16(status).unwrap(),
            [("content-type", "application/xml")],
            reply,
        )
            .into_response()
    }
    let app = Router::new()
        .route("/hub", post(handle))
        .with_state((raw.clone(), status, reply));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    // The URL guard refuses literal IPs; `localhost` is allow-listed in cfg_for.
    (format!("http://localhost:{}/hub", addr.port()), raw)
}

struct Env {
    app: Router,
    _xsd: TempDir,
    _dsl: TempDir,
}

fn write_lane(sidecar: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    let group = dir.path().join("erru");
    std::fs::create_dir_all(&group).unwrap();
    std::fs::write(group.join("Global_Types.xsd"), GLOBAL_XSD).unwrap();
    std::fs::write(group.join("Msg.xsd"), MSG_XSD).unwrap();
    std::fs::write(group.join("msg.xml.yaml"), sidecar).unwrap();
    dir
}

fn cfg_for(dir: &TempDir, dsl: &TempDir) -> AppConfig {
    AppConfig {
        dsl_path: dsl.path().to_path_buf(),
        wsdl_watch_dir: Some(dir.path().to_path_buf()),
        wsdl: xtr_on_rust::config::WsdlIngest {
            allow_http_upstream: true,
            upstream_host_allowlist: vec!["localhost".into()],
        },
        ..Default::default()
    }
}

#[derive(Default)]
struct Opts {
    token: Option<&'static str>,
    offline: bool,
    max_request_bytes: Option<usize>,
    max_response_bytes: Option<usize>,
    request_timeout_secs: Option<u64>,
}

fn build(sidecar: &str) -> Env {
    build_opts(sidecar, Opts::default())
}

fn build_opts(sidecar: &str, o: Opts) -> Env {
    let xsd = write_lane(sidecar);
    let dsl = TempDir::new().unwrap();
    let mut cfg = cfg_for(&xsd, &dsl);
    if let Some(n) = o.max_request_bytes {
        cfg.limits.max_request_bytes = n;
    }
    if let Some(n) = o.max_response_bytes {
        cfg.limits.max_response_bytes = n;
    }
    if let Some(n) = o.request_timeout_secs {
        cfg.limits.request_timeout_secs = n;
    }
    let token = o.token.map(|t| Arc::new(t.to_string()));
    let services = loader::load_all(&cfg.dsl_path).unwrap();
    let spec = openapi::build_spec(&services, "test");
    let cfg = Arc::new(cfg);
    let reg = xmllane::load_all(Some(xsd.path()), &cfg).unwrap();
    let lane =
        xmllane::handler::XmlLaneState::new(reg, cfg.clone(), o.offline, token.clone()).unwrap();
    let extra = xmllane::handler::outbound_router(lane.clone())
        .merge(xmllane::handler::inbound_router(lane));
    let app = router::build_with(
        AppState {
            cfg: cfg.clone(),
            services: Arc::new(services),
            executor: Executor::new(&cfg).unwrap(),
            openapi_spec: Arc::new(spec),
            inter_service_token: token,
        },
        Some(extra),
    );
    Env {
        app,
        _xsd: xsd,
        _dsl: dsl,
    }
}

async fn call(app: &Router, req: Request<Body>) -> (StatusCode, String, HeaderMap) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned(), headers)
}

fn xml_post(uri: &str, body: &str) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/xml")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn inbound_sidecar(backend: &str) -> String {
    format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request: {{}}\n  Notify_Acknowledgement: {{}}\ninbound:\n  backend: {backend}\n  path: /erru/xml\n"
    )
}

// ------------------------------------------------------------ inbound

#[tokio::test]
async fn inbound_decodes_xml_and_hands_backend_json_bytes_and_digest() {
    let (backend, seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
    assert!(body.is_empty());

    let seen = seen.lock().unwrap();
    let (msg, p) = &seen[0];
    assert_eq!(msg, "Notify_Request");
    assert_eq!(p["service"], "erru/msg");
    assert_eq!(p["message"], "Notify_Request");
    assert_eq!(p["rawXml"], NOTIFY_XML);
    assert_eq!(p["request"]["Header"]["@technicalId"], "t-1");
    assert_eq!(p["request"]["Body"]["@businessCaseId"], "b-1");
    // Repeating element is an array, attributes are @-keys.
    assert_eq!(
        p["request"]["Body"]["TransportManager"][1]["@familyName"],
        "Kask"
    );
    // SHA-256 of the exact bytes received.
    use sha2::{Digest, Sha256};
    let want: String = Sha256::digest(NOTIFY_XML.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(p["sha256"], want);
}

#[tokio::test]
async fn inbound_path_alias_serves_the_same_lane() {
    let (backend, seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    let (st, _, _) = call(&env.app, xml_post("/erru/xml", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn inbound_include_raw_false_omits_the_bytes_but_keeps_digest() {
    let (backend, seen) = spawn_backend().await;
    let env = build(&format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request: {{}}\ninbound:\n  backend: {backend}\n  include_raw: false\n  success_status: 200\n"
    ));
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::OK);
    let seen = seen.lock().unwrap();
    assert!(seen[0].1.get("rawXml").is_none());
    assert!(seen[0].1["sha256"].is_string());
}

#[tokio::test]
async fn inbound_relays_backend_4xx_and_hides_backend_5xx() {
    let (backend, _seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    let dup = NOTIFY_XML.replace("t-1", "dup");
    let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", &dup)).await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert!(body.contains("conflict"));
    let boom = NOTIFY_XML.replace("t-1", "boom");
    let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", &boom)).await;
    assert_eq!(st, StatusCode::BAD_GATEWAY);
    assert!(!body.contains("password"), "backend detail leaked: {body}");
}

#[tokio::test]
async fn inbound_rejects_bad_input_before_calling_the_backend() {
    let (backend, seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    let cases: Vec<(&str, Vec<u8>, StatusCode)> = vec![
        ("not xml", b"hello".to_vec(), StatusCode::BAD_REQUEST),
        (
            "doctype",
            br#"<!DOCTYPE x [<!ENTITY a "b">]><Notify_Request xmlns="urn:erru"/>"#.to_vec(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "unknown root",
            br#"<Other_Request xmlns="urn:erru"><Header technicalId="a" workflowId="b"/></Other_Request>"#
                .to_vec(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "wrong namespace",
            br#"<Notify_Request xmlns="urn:other"/>"#.to_vec(),
            StatusCode::BAD_REQUEST,
        ),
        ("not utf-8", vec![0xff, 0xfe, 0x3c, 0x61, 0x2f, 0x3e], StatusCode::BAD_REQUEST),
    ];
    for (name, body, want) in cases {
        let req = Request::post("/xml-in/erru/msg")
            .body(Body::from(body))
            .unwrap();
        let (st, _, _) = call(&env.app, req).await;
        assert_eq!(st, want, "case {name}");
    }
    assert!(
        seen.lock().unwrap().is_empty(),
        "backend must not be called"
    );
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/nope", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn inbound_forward_relays_exact_bytes_and_status_to_the_old_system() {
    let (backend, seen) = spawn_backend().await;
    let (old, raw) = spawn_peer(202, "").await;
    let env = build(&format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request: {{}}\n  Notify_Acknowledgement:\n    forward: {old}\ninbound:\n  backend: {backend}\n"
    ));
    let ack = r#"<Notify_Acknowledgement xmlns="urn:erru"><Header technicalId="a" workflowId="b"/><Body statusCode="OK"/></Notify_Acknowledgement>"#;
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", ack)).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert!(
        seen.lock().unwrap().is_empty(),
        "forwarded message must not hit the backend"
    );
    {
        let raw = raw.lock().unwrap();
        assert_eq!(raw[0].1, ack.as_bytes());
        assert_eq!(raw[0].0["content-type"], "application/xml");
    }
    // The other message type is still handled here.
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

// ----------------------------------------------------------- outbound

fn outbound_sidecar(url: &str) -> String {
    format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request: {{}}\n  Other_Request:\n    timeout_secs: 3\noutbound:\n  url: {url}\n  timeout_secs: 5\n"
    )
}

fn notify_json() -> Value {
    json!({
        "Header": {"@technicalId": "t-9", "@workflowId": "w-9", "@to": "DE"},
        "Body": {"@businessCaseId": "b-9", "TransportManager": [
            {"@firstName": "A <&>", "@familyName": "B"}
        ]}
    })
}

fn json_post(uri: &str, v: &Value) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(v.to_string()))
        .unwrap()
}

#[tokio::test]
async fn outbound_posts_raw_xml_and_returns_status_and_decoded_reply() {
    let ack = r#"<Notify_Acknowledgement xmlns="urn:erru"><Header technicalId="a" workflowId="w-9"/><Body statusCode="OK"/></Notify_Acknowledgement>"#;
    let (hub, raw) = spawn_peer(202, ack).await;
    let env = build(&outbound_sidecar(&hub));
    let (st, body, _) = call(
        &env.app,
        json_post("/xml-out/erru/msg/Notify_Request", &notify_json()),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["status"], 202);
    assert_eq!(v["response"]["message"], "Notify_Acknowledgement");
    assert_eq!(v["response"]["content"]["Body"]["@statusCode"], "OK");

    let raw = raw.lock().unwrap();
    let sent = String::from_utf8(raw[0].1.clone()).unwrap();
    assert_eq!(
        raw[0].0["content-type"], "application/xml; charset=utf-8",
        "raw XML, not SOAP"
    );
    assert!(
        sent.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"),
        "{sent}"
    );
    assert!(!sent.contains("Envelope"), "no SOAP envelope: {sent}");
    assert!(sent.contains("technicalId=\"t-9\""), "{sent}");
    assert!(
        sent.contains("firstName=\"A &lt;&amp;&gt;\""),
        "escaped: {sent}"
    );
}

#[tokio::test]
async fn outbound_reports_upstream_status_instead_of_failing() {
    let (hub, _raw) = spawn_peer(500, "<html>oops</html>").await;
    let env = build(&outbound_sidecar(&hub));
    let (st, body, _) = call(
        &env.app,
        json_post("/xml-out/erru/msg/Notify_Request", &notify_json()),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["status"], 500);
    assert!(v["response"].is_null());
    assert_eq!(v["body"], "<html>oops</html>");
}

#[tokio::test]
async fn outbound_refuses_unknown_message_and_bad_json() {
    let (hub, raw) = spawn_peer(202, "").await;
    let env = build(&outbound_sidecar(&hub));
    let (st, _, _) = call(
        &env.app,
        json_post("/xml-out/erru/msg/Notify_Acknowledgement", &json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let req = Request::post("/xml-out/erru/msg/Notify_Request")
        .body(Body::from("[1]"))
        .unwrap();
    let (st, _, _) = call(&env.app, req).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(raw.lock().unwrap().is_empty());
}

// ------------------------------------------------------- configuration

fn load_err(sidecar: &str) -> String {
    let xsd = write_lane(sidecar);
    let dsl = TempDir::new().unwrap();
    let cfg = cfg_for(&xsd, &dsl);
    xmllane::load_all(Some(xsd.path()), &cfg).unwrap_err()
}

#[test]
fn bad_sidecars_refuse_boot_with_a_clear_reason() {
    let cases = [
        ("schemas: [Missing.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b'}\n", "not found"),
        ("schemas: [Msg.xsd]\nmessages: {Nope: {}}\ninbound: {backend: 'http://b'}\n", "not a global element"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\n", "neither"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {}\n", "no backend"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b', path: '/health'}\n", "reserved"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b', success_status: 500}\n", "2xx"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\noutbound: {}\n", "no peer URL"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\noutbound: {url: 'http://localhost:1/x', timeout_secs: 9999}\n", "timeout_secs"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {forward: 'http://f', backend: 'http://b'}}\ninbound: {}\n", "either"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\nbogus: 1\ninbound: {backend: 'http://b'}\n", "unknown field"),
    ];
    for (sidecar, want) in cases {
        let e = load_err(sidecar);
        assert!(
            e.contains(want),
            "expected {want:?} in: {e}\nfor sidecar: {sidecar}"
        );
    }
}

// ------------------------------------------------- review follow-ups

/// Raw peer answering with arbitrary bytes, optionally after a delay.
async fn spawn_raw_peer(status: u16, reply: Vec<u8>, delay_ms: u64) -> String {
    async fn handle(
        State((status, reply, delay)): State<(u16, Vec<u8>, u64)>,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        (StatusCode::from_u16(status).unwrap(), reply).into_response()
    }
    let app = Router::new()
        .route("/hub", post(handle))
        .with_state((status, reply, delay_ms));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://localhost:{}/hub", addr.port())
}

#[tokio::test]
async fn inbound_hides_non_verdict_4xx_from_the_peer() {
    let (backend, _seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    // A 404 from the backend is a wrong path, not a verdict on the message.
    let nf = NOTIFY_XML.replace("t-1", "nf");
    let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", &nf)).await;
    assert_eq!(st, StatusCode::BAD_GATEWAY);
    assert!(!body.contains("secret"), "{body}");

    // `relay_statuses` is configurable.
    let env = build(&format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request: {{}}\ninbound:\n  backend: {backend}\n  relay_statuses: [404]\n"
    ));
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", &nf)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let dup = NOTIFY_XML.replace("t-1", "dup");
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", &dup)).await;
    assert_eq!(st, StatusCode::BAD_GATEWAY, "409 no longer listed");
}

#[tokio::test]
async fn forward_hides_target_5xx_and_relays_target_4xx() {
    let (backend, _seen) = spawn_backend().await;
    let (broken, _) = spawn_peer(500, "stack trace at old-host.internal").await;
    let env = build(&format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request:\n    forward: {broken}\ninbound:\n  backend: {backend}\n"
    ));
    let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::BAD_GATEWAY);
    assert!(!body.contains("old-host"), "{body}");

    let (rejecting, _) = spawn_peer(422, "<Error/>").await;
    let env = build(&format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request:\n    forward: {rejecting}\ninbound:\n  backend: {backend}\n"
    ));
    let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body, "<Error/>");
}

#[tokio::test]
async fn forward_does_not_relay_404_and_401_by_default() {
    let (backend, _s) = spawn_backend().await;
    for code in [404u16, 401] {
        let (peer, _) = spawn_peer(code, "<E/>").await;
        let env = build(&format!(
            "schemas: [Msg.xsd]\nmessages:\n  Notify_Request:\n    forward: {peer}\ninbound:\n  backend: {backend}\n"
        ));
        let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", NOTIFY_XML)).await;
        assert_eq!(st, StatusCode::BAD_GATEWAY, "forward {code}");
        assert!(!body.contains("<E/>"));
    }
}

#[tokio::test]
async fn forward_success_with_over_limit_body_is_still_a_success() {
    let (backend, _s) = spawn_backend().await;
    let big: &'static str = Box::leak("x".repeat(500).into_boxed_str());
    let (peer, _) = spawn_peer(202, big).await;
    let dir = format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request:\n    forward: {peer}\ninbound:\n  backend: {backend}\n"
    );
    let env = build_opts(
        &dir,
        Opts {
            max_response_bytes: Some(100),
            ..Default::default()
        },
    );
    let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert!(body.len() <= 100, "oversized body must be dropped");
}

#[tokio::test]
async fn forward_without_content_type_defaults_to_application_xml() {
    let (backend, _s) = spawn_backend().await;
    let (old, raw) = spawn_peer(202, "").await;
    let env = build(&format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request:\n    forward: {old}\ninbound:\n  backend: {backend}\n"
    ));
    let req = Request::post("/xml-in/erru/msg")
        .body(Body::from(NOTIFY_XML))
        .unwrap();
    let (st, _, _) = call(&env.app, req).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert_eq!(raw.lock().unwrap()[0].0["content-type"], "application/xml");
}

#[tokio::test]
async fn inbound_accepts_a_utf8_bom() {
    let (backend, seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    let with_bom = String::from("\u{feff}") + NOTIFY_XML;
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", &with_bom)).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn inbound_rejects_oversized_bodies_with_413() {
    let (backend, seen) = spawn_backend().await;
    let env = build_opts(
        &inbound_sidecar(&backend),
        Opts {
            max_request_bytes: Some(64),
            ..Default::default()
        },
    );
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn offline_mode_blocks_both_directions_and_forward() {
    let (backend, seen) = spawn_backend().await;
    let (hub, raw) = spawn_peer(202, "").await;
    let sidecar = format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request: {{}}\n  Other_Request:\n    forward: {hub}\ninbound:\n  backend: {backend}\noutbound:\n  url: {hub}\n"
    );
    let env = build_opts(
        &sidecar,
        Opts {
            offline: true,
            ..Default::default()
        },
    );
    let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let other = r#"<Other_Request xmlns="urn:erru"><Header technicalId="a" workflowId="b"/></Other_Request>"#;
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", other)).await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    let (st, _, _) = call(
        &env.app,
        json_post("/xml-out/erru/msg/Notify_Request", &notify_json()),
    )
    .await;
    assert_eq!(st.as_u16(), 599);
    assert!(seen.lock().unwrap().is_empty());
    assert!(raw.lock().unwrap().is_empty());
}

#[tokio::test]
async fn xml_out_is_token_gated_and_xml_in_is_not() {
    let (backend, _s) = spawn_backend().await;
    let (hub, _raw) = spawn_peer(202, "").await;
    let sidecar = format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request: {{}}\ninbound:\n  backend: {backend}\noutbound:\n  url: {hub}\n"
    );
    let env = build_opts(
        &sidecar,
        Opts {
            token: Some("s3cret"),
            ..Default::default()
        },
    );
    let (st, _, _) = call(
        &env.app,
        json_post("/xml-out/erru/msg/Notify_Request", &notify_json()),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let req = Request::post("/xml-out/erru/msg/Notify_Request")
        .header("authorization", "Bearer s3cret")
        .body(Body::from(notify_json().to_string()))
        .unwrap();
    let (st, body, _) = call(&env.app, req).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    // The peer cannot send a bearer token: inbound stays open.
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn outbound_keeps_the_peer_status_for_odd_bodies() {
    // Latin-1 body (invalid UTF-8) with a 202: the peer accepted it.
    let hub = spawn_raw_peer(202, vec![0x3c, 0x61, 0xe4, 0x2f, 0x3e], 0).await;
    let env = build(&outbound_sidecar(&hub));
    let (st, body, _) = call(
        &env.app,
        json_post("/xml-out/erru/msg/Notify_Request", &notify_json()),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["status"], 202);
    assert_eq!(v["bodyTruncated"], false);

    // Oversized body: status kept, body cut and flagged.
    let hub = spawn_raw_peer(200, vec![b'x'; 5000], 0).await;
    let env = build_opts(
        &outbound_sidecar(&hub),
        Opts {
            max_response_bytes: Some(100),
            ..Default::default()
        },
    );
    let (st, body, _) = call(
        &env.app,
        json_post("/xml-out/erru/msg/Notify_Request", &notify_json()),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["status"], 200);
    assert_eq!(v["bodyTruncated"], true);
    assert_eq!(v["body"].as_str().unwrap().len(), 100);
    assert!(v["response"].is_null());
}

#[tokio::test]
async fn outbound_distinguishes_timeout_and_unreachable_without_leaking_the_url() {
    let slow = spawn_raw_peer(202, vec![], 3000).await;
    let env = build(&format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request:\n    timeout_secs: 1\noutbound:\n  url: {slow}\n"
    ));
    let (st, _, _) = call(
        &env.app,
        json_post("/xml-out/erru/msg/Notify_Request", &notify_json()),
    )
    .await;
    assert_eq!(st, StatusCode::GATEWAY_TIMEOUT);

    // Nothing listens on this port.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let env = build(&outbound_sidecar(&format!("http://localhost:{port}/hub")));
    let (st, body, _) = call(
        &env.app,
        json_post("/xml-out/erru/msg/Notify_Request", &notify_json()),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_GATEWAY);
    assert!(body.contains("upstream_unreachable"), "{body}");
    assert!(!body.contains("localhost"), "peer URL leaked: {body}");
}

#[tokio::test]
async fn direction_limits_what_each_side_accepts() {
    let (backend, seen) = spawn_backend().await;
    let (hub, raw) = spawn_peer(202, "").await;
    let env = build(&format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request:\n    direction: in\n  Other_Request:\n    direction: out\ninbound:\n  backend: {backend}\noutbound:\n  url: {hub}\n"
    ));
    // `out`-only message cannot arrive.
    let other = r#"<Other_Request xmlns="urn:erru"><Header technicalId="a" workflowId="b"/></Other_Request>"#;
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", other)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // `in`-only message cannot be sent.
    let (st, _, _) = call(
        &env.app,
        json_post("/xml-out/erru/msg/Notify_Request", &notify_json()),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(seen.lock().unwrap().is_empty());
    assert!(raw.lock().unwrap().is_empty());
    // The permitted directions work.
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", NOTIFY_XML)).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    let (st, _, _) = call(
        &env.app,
        json_post(
            "/xml-out/erru/msg/Other_Request",
            &json!({"Header": {"@technicalId": "a", "@workflowId": "b"}}),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

#[test]
fn more_bad_sidecars_refuse_boot() {
    let cases = [
        // direction conflicts
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {direction: out, backend: 'http://b'}}\ninbound: {backend: 'http://b'}\n", "direction: out"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {direction: in, timeout_secs: 3}}\ninbound: {backend: 'http://b'}\noutbound: {url: 'http://localhost:1/x'}\n", "direction: in"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {url: 'http://localhost:1/x'}}\ninbound: {backend: 'http://b'}\n", "need an `outbound:`"),
        // direction vs sections
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {direction: in}}\noutbound: {url: 'http://localhost:1/x'}\n", "no `inbound:`"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {direction: out}}\ninbound: {backend: 'http://b'}\n", "no `outbound:`"),
        // SSRF guard on peer URLs
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\noutbound: {url: 'http://127.0.0.1:9/x'}\n", "Notify_Request"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\noutbound: {url: 'http://not-allowlisted.example/x'}\n", "Notify_Request"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\noutbound: {url: 'ftp://localhost/x'}\n", "Notify_Request"),
        // content type, schema names, relay statuses
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\noutbound: {url: 'http://localhost:1/x', content_type: \"a\\u00e4\"}\n", "content_type"),
        ("schemas: [sub/Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b'}\n", "plain file names"),
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b', relay_statuses: [500]}\n", "relay_statuses"),
        // schema problems
        ("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'ftp://b'}\n", "only http/https"),
    ];
    for (sidecar, want) in cases {
        let e = load_err(sidecar);
        assert!(
            e.contains(want),
            "expected {want:?} in: {e}\nfor sidecar: {sidecar}"
        );
    }
}

#[test]
fn duplicate_endpoint_names_and_paths_refuse_boot() {
    let dir = write_lane("schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b', path: /erru/xml}\n");
    // `erru/msg2` with the same alias path.
    std::fs::write(
        dir.path().join("erru/msg2.xml.yaml"),
        "schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b', path: /erru/xml}\n",
    )
    .unwrap();
    let dsl = TempDir::new().unwrap();
    let cfg = cfg_for(&dir, &dsl);
    let e = xmllane::load_all(Some(dir.path()), &cfg).unwrap_err();
    assert!(e.contains("already used"), "{e}");

    // `erru/a/x` and `erru/a-x` both map to /xml-in/erru/a-x.
    let dir = write_lane(
        "schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b'}\n",
    );
    let nested = dir.path().join("erru/a");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("Global_Types.xsd"), GLOBAL_XSD).unwrap();
    std::fs::write(nested.join("Msg.xsd"), MSG_XSD).unwrap();
    let sc = "schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b'}\n";
    std::fs::write(nested.join("x.xml.yaml"), sc).unwrap();
    std::fs::write(dir.path().join("erru/a-x.xml.yaml"), sc).unwrap();
    let dsl = TempDir::new().unwrap();
    let cfg = cfg_for(&dir, &dsl);
    let e = xmllane::load_all(Some(dir.path()), &cfg).unwrap_err();
    assert!(e.contains("already taken"), "{e}");
}

#[test]
fn schema_loading_edge_cases() {
    // Listing a file that an earlier entry already includes is fine.
    let ok = "schemas: [Msg.xsd, Global_Types.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b'}\n";
    let xsd = write_lane(ok);
    let dsl = TempDir::new().unwrap();
    xmllane::load_all(Some(xsd.path()), &cfg_for(&xsd, &dsl)).unwrap();

    // Unresolved type reference.
    let xsd = write_lane(ok);
    std::fs::write(
        xsd.path().join("erru/Global_Types.xsd"),
        GLOBAL_XSD.replace(
            "</xs:schema>",
            r#"<xs:element name="Bad" type="MissingT"/></xs:schema>"#,
        ),
    )
    .unwrap();
    let e = xmllane::load_all(Some(xsd.path()), &cfg_for(&xsd, &dsl)).unwrap_err();
    assert!(e.contains("MissingT"), "{e}");

    // Two namespaces define the same global element name.
    let xsd = write_lane(ok);
    std::fs::write(
        xsd.path().join("erru/Msg.xsd"),
        MSG_XSD.replace(
            "<xs:include schemaLocation=\"Global_Types.xsd\"/>",
            "<xs:include schemaLocation=\"Global_Types.xsd\"/><xs:import namespace=\"urn:two\" schemaLocation=\"Two.xsd\"/>",
        ),
    )
    .unwrap();
    std::fs::write(
        xsd.path().join("erru/Two.xsd"),
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:two" elementFormDefault="qualified"><xs:element name="Notify_Request" type="xs:string"/></xs:schema>"#,
    )
    .unwrap();
    let e = xmllane::load_all(Some(xsd.path()), &cfg_for(&xsd, &dsl)).unwrap_err();
    assert!(e.contains("namespaces"), "{e}");
}

#[test]
fn alias_shadowing_a_dsl_endpoint_is_reported_and_lanes_are_counted() {
    let sidecar = "schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b', path: /erru/xml}\noutbound: {url: 'http://localhost:1/x'}\n";
    let xsd = write_lane(sidecar);
    let dsl = TempDir::new().unwrap();
    let cfg = cfg_for(&xsd, &dsl);
    let reg = xmllane::load_all(Some(xsd.path()), &cfg).unwrap();
    assert!(reg.alias_conflicts(|_, _| false, false).is_empty());
    let c = reg.alias_conflicts(|g, n| (g, n) == ("erru", "xml"), false);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(c[0].contains("/erru/xml"));
    // Doctor-side count reads the YAML only.
    let sum = xmllane::count_lanes(Some(xsd.path()));
    assert_eq!((sum.inbound, sum.outbound), (1, 1));
}

// ------------------------------------------------- review fixes (issue #40)

#[tokio::test]
async fn inbound_rejects_root_in_a_foreign_namespace() {
    let (backend, seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    // Right local name, wrong namespace.
    let foreign = NOTIFY_XML.replace("urn:erru", "urn:other");
    let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", &foreign)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("unsupported_message"), "{body}");
    assert!(
        seen.lock().unwrap().is_empty(),
        "backend must not be called"
    );
}

#[tokio::test]
async fn inbound_relayed_4xx_over_limit_is_cut() {
    let (backend, _seen) = spawn_backend().await;
    let env = build_opts(
        &inbound_sidecar(&backend),
        Opts {
            max_response_bytes: Some(64),
            ..Default::default()
        },
    );
    let big = NOTIFY_XML.replace("t-1", "big");
    let (st, body, headers) = call(&env.app, xml_post("/xml-in/erru/msg", &big)).await;
    // The status stays. The body is cut at the limit, not dropped.
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(body.len(), 64, "{body}");
    assert_eq!(headers["content-type"], "application/json");
}

#[test]
fn alias_on_separate_inbound_port_does_not_clash_with_dsl() {
    let sidecar = "schemas: [Msg.xsd]\nmessages: {Notify_Request: {}}\ninbound: {backend: 'http://b', path: /erru/xml}\n";
    let xsd = write_lane(sidecar);
    let dsl = TempDir::new().unwrap();
    let cfg = cfg_for(&xsd, &dsl);
    let reg = xmllane::load_all(Some(xsd.path()), &cfg).unwrap();
    let dsl_has = |g: &str, n: &str| (g, n) == ("erru", "xml");
    // Shared listener: the alias would shadow the DSL endpoint.
    assert_eq!(reg.alias_conflicts(dsl_has, false).len(), 1);
    // Separate `inbound.port`: the alias is on its own listener. No clash.
    assert!(reg.alias_conflicts(dsl_has, true).is_empty());
}

#[test]
fn inbound_path_rejects_bare_lane_prefixes() {
    for p in ["/xml-in", "/xml-out", "/xml-in/", "/xml-out/x"] {
        let e = load_err(&format!(
            "schemas: [Msg.xsd]\nmessages: {{Notify_Request: {{}}}}\ninbound: {{backend: 'http://b', path: {p}}}\n"
        ));
        assert!(e.contains("reserved prefix"), "{p}: {e}");
    }
}

#[tokio::test]
async fn forward_to_own_inbound_is_refused_as_a_loop() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // XTR forwards a message to its own /xml-in. The loop must stop at once.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (backend, _seen) = spawn_backend().await;
    let env = build_opts(
        &format!(
            "schemas: [Msg.xsd]\nmessages:\n  Notify_Request:\n    forward: http://127.0.0.1:{port}/xml-in/erru/msg\ninbound:\n  backend: {backend}\n"
        ),
        Opts {
            request_timeout_secs: Some(2),
            ..Default::default()
        },
    );

    // Direct check: a request that carries the guard header is refused.
    let mut req = xml_post("/xml-in/erru/msg", NOTIFY_XML);
    req.headers_mut()
        .insert("x-xtr-forwarded", "1".parse().unwrap());
    let (st, body, _) = call(&env.app, req).await;
    assert_eq!(st, StatusCode::LOOP_DETECTED, "{body}");
    assert!(body.contains("loop_detected"), "{body}");

    // End to end: count the hits on a real listener. A runaway loop is cut at 20.
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = env.app.clone().layer(axum::middleware::from_fn(
        move |req: Request<Body>, next: axum::middleware::Next| {
            let counter = counter.clone();
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) >= 20 {
                    return axum::response::IntoResponse::into_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                    );
                }
                next.run(req).await
            }
        },
    ));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/xml-in/erru/msg"))
        .header("content-type", "application/xml")
        .body(NOTIFY_XML)
        .send()
        .await
        .unwrap();
    // The first hop forwards. The second hop sees the guard header and answers 508.
    // The first hop reports the failed forward target as 502.
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "loop must stop after one hop"
    );
}

#[tokio::test]
async fn forwarded_header_on_a_backend_message_is_accepted() {
    // A chain XTR A -> XTR B works when B sends the message to its backend.
    let (backend, seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    let mut req = xml_post("/xml-in/erru/msg", NOTIFY_XML);
    req.headers_mut()
        .insert("x-xtr-forwarded", "1".parse().unwrap());
    let (st, body, _) = call(&env.app, req).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{body}");
    assert_eq!(seen.lock().unwrap().len(), 1);
}

// ------------------------------------------------------------ declared encoding

fn with_decl(decl: &str) -> String {
    format!(
        r#"{decl}<Notify_Request xmlns="urn:erru"><Header technicalId="t-1" workflowId="w-1"/><Body businessCaseId="b-1"><TransportManager/></Body></Notify_Request>"#
    )
}

/// MOVEHUB ERRU XML Message Reference v2.06, §5.7.2: messages are UTF-8 only. A declaration
/// of another encoding is refused, so XTR and a forward target read the same
/// bytes the same way.
#[tokio::test]
async fn inbound_refuses_a_declared_encoding_other_than_utf8() {
    let (backend, seen) = spawn_backend().await;
    let (hub, raw) = spawn_peer(202, "").await;
    let sidecar = format!(
        "schemas: [Msg.xsd]\nmessages:\n  Notify_Request: {{}}\n  Notify_Acknowledgement:\n    forward: {hub}\ninbound:\n  backend: {backend}\n"
    );
    let env = build(&sidecar);
    for decl in [
        r#"<?xml version="1.0" encoding="UTF-7"?>"#,
        r#"<?xml version="1.0" encoding="ISO-8859-1"?>"#,
        r#"<?xml version="1.0" encoding="UTF-16"?>"#,
        r#"<?xml version='1.0' encoding='windows-1252'?>"#,
        r#"<?xml version="1.0" encoding = "latin1" standalone="yes" ?>"#,
    ] {
        let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", &with_decl(decl))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{decl}: {body}");
        assert!(body.contains("unsupported_encoding"), "{decl}: {body}");
    }
    // The forward path refuses it too.
    let fwd = with_decl(r#"<?xml version="1.0" encoding="ISO-8859-1"?>"#)
        .replace("Notify_Request", "Notify_Acknowledgement");
    let (st, _, _) = call(&env.app, xml_post("/xml-in/erru/msg", &fwd)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(
        seen.lock().unwrap().is_empty(),
        "backend must not be called"
    );
    assert!(
        raw.lock().unwrap().is_empty(),
        "forward target must not be called"
    );
}

#[tokio::test]
async fn inbound_accepts_utf8_declarations_in_any_case_and_no_declaration() {
    let (backend, seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    let ok = [
        r#"<?xml version="1.0" encoding="UTF-8"?>"#,
        r#"<?xml version="1.0" encoding="utf-8"?>"#,
        r#"<?xml version='1.0' encoding='utf8'?>"#,
        r#"<?xml version="1.0"?>"#,
        "",
    ];
    for decl in ok {
        let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", &with_decl(decl))).await;
        assert_eq!(st, StatusCode::ACCEPTED, "{decl}: {body}");
    }
    assert_eq!(seen.lock().unwrap().len(), ok.len());
}

// ------------------------------------------------------------ symlink cycles (#43)

/// A symlink that points to a parent directory must not create phantom lanes.
#[cfg(unix)]
#[test]
fn symlink_cycle_in_the_lane_directory_creates_no_phantom_lanes() {
    let xsd = write_lane("schemas: [Msg.xsd]\nmessages:\n  Notify_Request: {}\ninbound:\n  backend: http://localhost:1/b\n");
    std::os::unix::fs::symlink(".", xsd.path().join("erru").join("loop")).unwrap();
    let dsl = TempDir::new().unwrap();
    let cfg = cfg_for(&xsd, &dsl);
    let reg = xmllane::load_all(Some(xsd.path()), &cfg).unwrap();
    assert_eq!(reg.len(), 1, "one sidecar, one lane");
    assert_eq!(xmllane::count_lanes(Some(xsd.path())).inbound, 1);
}

// ------------------------------------------------------------ well-formedness of the prolog

/// The checks sit in the parser, so a form that is not well-formed XML is
/// refused, not only the plain `encoding="X"` form. ERRU Reference §6: the
/// receiver must check that a message is well-formed XML.
#[tokio::test]
async fn inbound_refuses_prolog_forms_that_hide_or_break_the_declaration() {
    let (backend, seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    let root = r#"<Notify_Request xmlns="urn:erru"><Header technicalId="t-1" workflowId="w-1"/><Body businessCaseId="b-1"><TransportManager/></Body></Notify_Request>"#;
    let bom = "\u{feff}";
    let cases: Vec<(&str, String)> = vec![
        (
            "space before decl",
            format!(r#" <?xml version="1.0" encoding="UTF-7"?>{root}"#),
        ),
        (
            "newline before decl",
            format!("\n<?xml version=\"1.0\" encoding=\"UTF-7\"?>{root}"),
        ),
        (
            "comment before decl",
            format!(r#"<!--c--><?xml version="1.0" encoding="UTF-7"?>{root}"#),
        ),
        (
            "pi before decl",
            format!(r#"<?xml-foo?><?xml version="1.0" encoding="UTF-7"?>{root}"#),
        ),
        (
            "encoding inside version",
            format!(r#"<?xml version="encoding='utf-8'" encoding="UTF-7"?>{root}"#),
        ),
        (
            "glued attributes",
            format!(r#"<?xml version="1.0"encoding="UTF-7"?>{root}"#),
        ),
        (
            "unquoted encoding",
            format!(r#"<?xml version="1.0" encoding=UTF-7?>{root}"#),
        ),
        (
            "double BOM",
            format!(r#"{bom}{bom}<?xml version="1.0" encoding="UTF-7"?>{root}"#),
        ),
        (
            "upper-case XML target",
            format!(r#"<?XML version="1.0" encoding="UTF-7"?>{root}"#),
        ),
        (
            "decl after root",
            format!(r#"{root}<?xml version="1.0" encoding="UTF-7"?>"#),
        ),
        (
            "second decl",
            format!(r#"<?xml version="1.0"?><?xml version="1.0" encoding="UTF-7"?>{root}"#),
        ),
        ("text before root", format!("junk{root}")),
        ("bad version", format!(r#"<?xml version="2.0"?>{root}"#)),
    ];
    for (name, xml) in &cases {
        let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", xml)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{name}: {body}");
    }
    // UTF-16LE without BOM and without declaration: NUL bytes, not XML.
    let utf16: Vec<u8> = root.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let req = Request::post("/xml-in/erru/msg")
        .body(Body::from(utf16))
        .unwrap();
    let (st, _, _) = call(&env.app, req).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(
        seen.lock().unwrap().is_empty(),
        "backend must not be called"
    );
}

#[tokio::test]
async fn inbound_still_accepts_one_bom_and_a_well_formed_prolog() {
    let (backend, seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    let root = r#"<Notify_Request xmlns="urn:erru"><Header technicalId="t-1" workflowId="w-1"/><Body businessCaseId="b-1"><TransportManager/></Body></Notify_Request>"#;
    let ok = [
        format!("\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>{root}"),
        format!("<?xml version=\"1.1\" encoding=\"utf8\" standalone=\"yes\"?><!--c-->\n{root}\n<!--end-->\n"),
        format!("<?xml-stylesheet href=\"x\"?>{root}"),
        format!("\n  {root}  \n"),
    ];
    for xml in &ok {
        let (st, body, _) = call(&env.app, xml_post("/xml-in/erru/msg", xml)).await;
        assert_eq!(st, StatusCode::ACCEPTED, "{xml}: {body}");
    }
    assert_eq!(seen.lock().unwrap().len(), ok.len());
}

/// XML 1.0 §2.8: the declaration has `version`, then an optional `encoding`,
/// then an optional `standalone`, each once, in lower case. Nothing else.
#[tokio::test]
async fn inbound_refuses_declarations_that_break_the_declaration_grammar() {
    let (backend, seen) = spawn_backend().await;
    let env = build(&inbound_sidecar(&backend));
    let root = r#"<Notify_Request xmlns="urn:erru"><Header technicalId="t-1" workflowId="w-1"/><Body businessCaseId="b-1"><TransportManager/></Body></Notify_Request>"#;
    let decls = [
        r#"<?xml version="1.0" encoding="UTF-8" encoding="UTF-7"?>"#,
        r#"<?xml version="1.0" Encoding="UTF-7"?>"#,
        r#"<?xml version="1.0" encoding="UTF-8" foo="x"?>"#,
        r#"<?xml version="1.0" standalone="yes" encoding="UTF-8"?>"#,
        r#"<?xml encoding="UTF-8" version="1.0"?>"#,
        r#"<?xml version="1.0" version="1.1"?>"#,
        r#"<?xml version="1.0" standalone="maybe"?>"#,
        r#"<?xml encoding="UTF-8"?>"#,
        r#"<?xml?>"#,
    ];
    for decl in decls {
        let (st, body, _) = call(
            &env.app,
            xml_post("/xml-in/erru/msg", &format!("{decl}{root}")),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{decl}: {body}");
    }
    assert!(seen.lock().unwrap().is_empty());
}
