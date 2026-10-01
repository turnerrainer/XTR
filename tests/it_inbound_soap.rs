//! Inbound SOAP lane end-to-end: SOAP client → XTR router → real
//! HTTP JSON backend → SOAP response. Uses the production router
//! (`router::build_with`) so the shared middleware stack is included.

use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tower::ServiceExt;
use xtr_on_rust::config::AppConfig;
use xtr_on_rust::dsl::loader;
use xtr_on_rust::executor::Executor;
use xtr_on_rust::inbound;
use xtr_on_rust::openapi;
use xtr_on_rust::router::{self, AppState};

const WSDL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
    xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" xmlns:xs="http://www.w3.org/2001/XMLSchema"
    xmlns:tns="urn:demo" targetNamespace="urn:demo">
  <wsdl:types>
    <xs:schema targetNamespace="urn:demo" elementFormDefault="qualified">
      <xs:complexType name="HeaderT">
        <xs:attribute name="technicalId" type="xs:string" use="required"/>
      </xs:complexType>
      <xs:element name="Check_Request"><xs:complexType><xs:sequence>
        <xs:element name="Header" type="tns:HeaderT"/>
        <xs:element name="Body"><xs:complexType><xs:sequence>
          <xs:element name="firstName" type="xs:string" maxOccurs="unbounded"/>
          <xs:element name="code" type="xs:string"/>
        </xs:sequence></xs:complexType></xs:element>
      </xs:sequence></xs:complexType></xs:element>
      <xs:element name="Check_Response"><xs:complexType><xs:sequence>
        <xs:element name="Header" type="tns:HeaderT"/>
        <xs:element name="Body"><xs:complexType><xs:sequence>
          <xs:element name="status" type="xs:string"/>
          <xs:element name="match" type="xs:string" minOccurs="0" maxOccurs="unbounded"/>
        </xs:sequence></xs:complexType></xs:element>
      </xs:sequence></xs:complexType></xs:element>
      <xs:element name="Notify"><xs:complexType><xs:sequence>
        <xs:element name="text" type="xs:string"/>
      </xs:sequence></xs:complexType></xs:element>
    </xs:schema>
  </wsdl:types>
  <wsdl:message name="CheckIn"><wsdl:part name="p" element="tns:Check_Request"/></wsdl:message>
  <wsdl:message name="CheckOut"><wsdl:part name="p" element="tns:Check_Response"/></wsdl:message>
  <wsdl:message name="NotifyIn"><wsdl:part name="p" element="tns:Notify"/></wsdl:message>
  <wsdl:portType name="P">
    <wsdl:operation name="Check"><wsdl:input message="tns:CheckIn"/><wsdl:output message="tns:CheckOut"/></wsdl:operation>
    <wsdl:operation name="Notify"><wsdl:input message="tns:NotifyIn"/></wsdl:operation>
  </wsdl:portType>
  <wsdl:binding name="B" type="tns:P">
    <soap:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/>
    <wsdl:operation name="Check"><soap:operation soapAction="urn:demo/Check"/></wsdl:operation>
    <wsdl:operation name="Notify"><soap:operation soapAction="urn:demo/Notify"/></wsdl:operation>
  </wsdl:binding>
  <wsdl:service name="S"><wsdl:port name="p" binding="tns:B">
    <soap:address location="http://TURVASERVER/cgi-bin/consumer_proxy"/>
  </wsdl:port></wsdl:service>
</wsdl:definitions>"#;

type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// Backend: records every call; `Check` answers from the request, a
/// `code` of "FAULT" yields a backend-declared fault.
async fn spawn_backend() -> (String, Seen) {
    let seen: Seen = Arc::default();
    async fn check(State(seen): State<Seen>, Json(v): Json<Value>) -> axum::response::Response {
        use axum::response::IntoResponse;
        seen.lock().unwrap().push(("Check".into(), v.clone()));
        let req = &v["request"];
        if req["Body"]["code"] == "FAULT" {
            return Json(json!({"fault": {"code": "Client", "string": "rejected by backend"}}))
                .into_response();
        }
        if req["Body"]["code"] == "BOOM" {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"message": "connection to db-internal-7:5432 refused"})),
            )
                .into_response();
        }
        Json(json!({
            "Header": {"@technicalId": req["Header"]["@technicalId"]},
            "Body": {"match": req["Body"]["firstName"], "status": "OK <&>"}
        }))
        .into_response()
    }
    async fn notify(State(seen): State<Seen>, Json(v): Json<Value>) -> StatusCode {
        seen.lock().unwrap().push(("Notify".into(), v));
        StatusCode::NO_CONTENT
    }
    let app = Router::new()
        .route("/b/Check", post(check))
        .route("/b/Notify", post(notify))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/b"), seen)
}

async fn build(backend: &str, outbound: bool) -> (Router, TempDir, TempDir) {
    build_with_sidecar(&format!(
        "dsl: {outbound}\ninbound:\n  backend: {backend}\n"
    ))
    .await
}

async fn build_with_sidecar(sidecar: &str) -> (Router, TempDir, TempDir) {
    build_full(sidecar, None, false).await
}

async fn build_full(
    sidecar: &str,
    token: Option<&str>,
    expose_soap_fault_detail: bool,
) -> (Router, TempDir, TempDir) {
    let wsdl_dir = TempDir::new().unwrap();
    let dsl_dir = TempDir::new().unwrap();
    let group = wsdl_dir.path().join("demo");
    std::fs::create_dir_all(&group).unwrap();
    std::fs::write(group.join("Svc.wsdl"), WSDL).unwrap();
    std::fs::write(group.join("Svc.soap.yaml"), sidecar).unwrap();
    let cfg = AppConfig {
        dsl_path: dsl_dir.path().to_path_buf(),
        wsdl_watch_dir: Some(wsdl_dir.path().to_path_buf()),
        wsdl: xtr_on_rust::config::WsdlIngest {
            allow_http_upstream: true,
            upstream_host_allowlist: vec![],
        },
        expose_soap_fault_detail,
        ..Default::default()
    };
    xtr_on_rust::wsdl::ingest_all(wsdl_dir.path(), dsl_dir.path(), &cfg.wsdl, &cfg.client_data)
        .unwrap();
    let services = loader::load_all(&cfg.dsl_path).unwrap();
    let spec = openapi::build_spec(&services, "test");
    let cfg = Arc::new(cfg);
    let registry = inbound::load_all(Some(wsdl_dir.path()), &cfg).unwrap();
    let lane = inbound::handler::LaneState::new(
        registry,
        cfg.clone(),
        false,
        token.map(|t| Arc::new(t.to_string())),
    )
    .unwrap();
    let extra = inbound::handler::outbound_router(lane.clone())
        .merge(inbound::handler::inbound_router(lane));
    let app = router::build_with(
        AppState {
            cfg: cfg.clone(),
            services: Arc::new(services),
            executor: Executor::new(&cfg).unwrap(),
            openapi_spec: Arc::new(spec),
            inter_service_token: None,
        },
        Some(extra),
    );
    (app, wsdl_dir, dsl_dir)
}

async fn call(app: &Router, req: Request<Body>) -> (StatusCode, String, axum::http::HeaderMap) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap(), headers)
}

fn soap(action: &str, body: &str) -> Request<Body> {
    Request::post("/soap-in/demo/Svc")
        .header("content-type", "text/xml; charset=utf-8")
        .header("SOAPAction", format!("\"{action}\""))
        .body(Body::from(format!(
            r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" xmlns:x="urn:x-road" xmlns:d="urn:demo"><s:Header><x:id>42</x:id></s:Header><s:Body>{body}</s:Body></s:Envelope>"#
        )))
        .unwrap()
}

#[tokio::test]
async fn soap_request_round_trips_through_json_backend() {
    let (backend, seen) = spawn_backend().await;
    let (app, _w, _d) = build(&backend, true).await;
    let (status, body, headers) = call(
        &app,
        soap(
            "urn:demo/Check",
            r#"<d:Check_Request><d:Header technicalId="007"/><d:Body><d:firstName>Alice</d:firstName><d:code>0012</d:code></d:Body></d:Check_Request>"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(headers["content-type"], "text/xml; charset=utf-8");
    // Shared middleware applies to the inbound routes too.
    assert!(headers.contains_key("x-content-type-options"));

    // Backend saw attributes, a schema-declared array (1 item) and
    // verbatim strings ("007", "0012" are not coerced to numbers).
    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    let v = &calls[0].1;
    assert_eq!(v["operation"], "Check");
    assert_eq!(v["soapAction"], "urn:demo/Check");
    assert_eq!(v["header"], json!({"id": "42"}));
    assert_eq!(
        v["request"],
        json!({"Header": {"@technicalId": "007"}, "Body": {"firstName": ["Alice"], "code": "0012"}})
    );

    // Response: header echoed, output element qualified, schema order
    // (status before match) regardless of JSON key order, escaping.
    assert!(
        body.contains(r#"<SOAP-ENV:Header><x:id xmlns:x="urn:x-road">42</x:id></SOAP-ENV:Header>"#),
        "{body}"
    );
    assert!(body.contains(
        r#"<Check_Response xmlns="urn:demo"><Header technicalId="007"/><Body><status>OK &lt;&amp;&gt;</status><match>Alice</match></Body></Check_Response>"#
    ), "{body}");
}

#[tokio::test]
async fn backend_fault_becomes_soap_fault() {
    let (backend, _seen) = spawn_backend().await;
    let (app, _w, _d) = build(&backend, true).await;
    let (status, body, _) = call(
        &app,
        soap(
            "urn:demo/Check",
            r#"<d:Check_Request><d:Header technicalId="1"/><d:Body><d:firstName>A</d:firstName><d:code>FAULT</d:code></d:Body></d:Check_Request>"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        body.contains(
            "<faultcode>SOAP-ENV:Client</faultcode><faultstring>rejected by backend</faultstring>"
        ),
        "{body}"
    );
}

#[tokio::test]
async fn one_way_operation_returns_202() {
    let (backend, seen) = spawn_backend().await;
    let (app, _w, _d) = build(&backend, true).await;
    let (status, body, _) = call(
        &app,
        soap(
            "urn:demo/Notify",
            "<d:Notify><d:text>hi</d:text></d:Notify>",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(seen.lock().unwrap()[0].0, "Notify");
}

#[tokio::test]
async fn rejects_unknown_operation_wrong_action_soap12_and_doctype() {
    let (backend, seen) = spawn_backend().await;
    let (app, _w, _d) = build(&backend, true).await;
    let (s, b, _) = call(&app, soap("", "<d:Nope/>")).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        b.contains("no operation of this service accepts {urn:demo}Nope"),
        "{b}"
    );

    let (_, b, _) = call(&app, soap("urn:demo/Notify", r#"<d:Check_Request/>"#)).await;
    assert!(b.contains("does not match operation Check"), "{b}");

    let req = Request::post("/soap-in/demo/Svc")
        .body(Body::from(r#"<e:Envelope xmlns:e="http://www.w3.org/2003/05/soap-envelope"><e:Body/></e:Envelope>"#))
        .unwrap();
    let (_, b, _) = call(&app, req).await;
    assert!(b.contains("SOAP-ENV:VersionMismatch"), "{b}");

    let req = Request::post("/soap-in/demo/Svc")
        .body(Body::from(r#"<!DOCTYPE x [<!ENTITY a "b">]><x/>"#))
        .unwrap();
    let (_, b, _) = call(&app, req).await;
    assert!(b.contains("DOCTYPE is not allowed"), "{b}");

    assert!(
        seen.lock().unwrap().is_empty(),
        "no backend call on client errors"
    );
}

#[tokio::test]
async fn serves_wsdl_with_rewritten_address() {
    let (backend, _seen) = spawn_backend().await;
    let (app, _w, _d) = build(&backend, true).await;
    let req = Request::get("/soap-in/demo/Svc?wsdl")
        .header("host", "xtr.example:8080")
        .body(Body::empty())
        .unwrap();
    let (s, b, _) = call(&app, req).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        b.contains(r#"location="http://xtr.example:8080/soap-in/demo/Svc""#),
        "{b}"
    );
    assert!(!b.contains("TURVASERVER"));
}

#[tokio::test]
async fn direction_is_chosen_per_wsdl() {
    let (backend, _seen) = spawn_backend().await;
    // outbound: true → the same WSDL also yields outbound DSL endpoints.
    let (app, _w, d) = build(&backend, true).await;
    assert!(d.path().join("demo/Check.yml").exists());
    let req = Request::get("/api").body(Body::empty()).unwrap();
    let (_, b, _) = call(&app, req).await;
    assert!(b.contains("/demo/Check"));
    // outbound: false → inbound only.
    let (_app, _w, d) = build(&backend, false).await;
    assert!(!d.path().join("demo/Check.yml").exists());
}

// ------------------------------------------------------------ outbound

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    // `localhost` (not 127.0.0.1): the URL guard rejects literal loopback IPs.
    format!("http://localhost:{port}")
}

/// SOAP peer that records the raw request and answers with a fixed
/// envelope (or a Fault when the body mentions FAULT).
async fn spawn_soap_peer() -> (String, Arc<Mutex<Vec<(String, String)>>>) {
    type Seen = Arc<Mutex<Vec<(String, String)>>>;
    let seen: Seen = Arc::default();
    async fn h(
        State(seen): State<Seen>,
        headers: axum::http::HeaderMap,
        body: String,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        let action = headers
            .get("soapaction")
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        seen.lock().unwrap().push((action, body.clone()));
        let (status, xml) = if body.contains("FAULT") {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><s:Fault><faultcode>s:Server</faultcode><faultstring>peer says no</faultstring></s:Fault></s:Body></s:Envelope>"#,
            )
        } else {
            (
                StatusCode::OK,
                r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><r:Check_Response xmlns:r="urn:demo"><r:Header technicalId="t-9"/><r:Body><r:status>FOUND</r:status><r:match>Bob</r:match></r:Body></r:Check_Response></s:Body></s:Envelope>"#,
            )
        };
        (status, [("content-type", "text/xml")], xml).into_response()
    }
    let app = Router::new()
        .route("/peer", post(h))
        .with_state(seen.clone());
    (format!("{}/peer", serve(app).await), seen)
}

fn json_post(uri: &str, v: Value) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(v.to_string()))
        .unwrap()
}

#[tokio::test]
async fn outbound_json_to_schema_aware_soap_and_back() {
    let (peer, seen) = spawn_soap_peer().await;
    let (app, _w, _d) = build_with_sidecar(&format!("outbound:\n  url: {peer}\n")).await;
    let (s, b, _) = call(
        &app,
        json_post(
            "/soap-out/demo/Svc/Check",
            json!({"Header": {"@technicalId": "007"}, "Body": {"code": "0012", "firstName": ["Alice", "Bob"]}}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let (action, sent) = seen.lock().unwrap()[0].clone();
    assert_eq!(action, "\"urn:demo/Check\"");
    assert!(sent.contains(
        r#"<Check_Request xmlns="urn:demo"><Header technicalId="007"/><Body><firstName>Alice</firstName><firstName>Bob</firstName><code>0012</code></Body></Check_Request>"#
    ), "{sent}");
    let v: Value = serde_json::from_str(&b).unwrap();
    // `match` is maxOccurs=unbounded → array even with one item.
    assert_eq!(
        v["response"],
        json!({"Header": {"@technicalId": "t-9"}, "Body": {"status": "FOUND", "match": ["Bob"]}})
    );
}

#[tokio::test]
async fn outbound_soap_fault_uses_standard_error_shape() {
    let (peer, _seen) = spawn_soap_peer().await;
    let (app, _w, _d) = build_with_sidecar(&format!("outbound:\n  url: {peer}\n")).await;
    let (s, b, _) = call(
        &app,
        json_post(
            "/soap-out/demo/Svc/Check",
            json!({"Body": {"code": "FAULT"}}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_GATEWAY, "{b}");
    let v: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(v["error"], "upstream_soap_fault");
    assert!(b.contains("peer says no"), "{b}");
}

#[tokio::test]
async fn outbound_adds_xroad_header_when_configured() {
    let (peer, seen) = spawn_soap_peer().await;
    let (app, _w, _d) = build_with_sidecar(&format!(
        "outbound:\n  url: {peer}\n  xroad_service: {{member_class: GOV, member_code: \"70000001\", subsystem_code: demo}}\n"
    ))
    .await;
    let req = Request::post("/soap-out/demo/Svc/Check")
        .header("x-road-userid", "EE10000000001")
        .body(Body::from("{}"))
        .unwrap();
    let (s, b, _) = call(&app, req).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let sent = seen.lock().unwrap()[0].1.clone();
    assert!(
        sent.contains("<id:serviceCode>Check</id:serviceCode>"),
        "{sent}"
    );
    assert!(
        sent.contains("<id:memberCode>70000001</id:memberCode>"),
        "{sent}"
    );
    assert!(
        sent.contains(
            "<xrd:userId xmlns:xrd=\"http://x-road.eu/xsd/xroad.xsd\">EE10000000001</xrd:userId>"
        ),
        "{sent}"
    );
}

#[tokio::test]
async fn unknown_outbound_op_is_404_and_no_inbound_without_section() {
    let (peer, _seen) = spawn_soap_peer().await;
    let (app, _w, _d) = build_with_sidecar(&format!("outbound:\n  url: {peer}\n")).await;
    let (s, _, _) = call(&app, json_post("/soap-out/demo/Svc/Nope", json!({}))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // No inbound section → no provider endpoint either.
    let (s, _, _) = call(&app, soap("", "<d:Notify/>")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// The whole point: one WSDL, both directions. XTR's own outbound
/// lane calls XTR's own inbound lane, which calls the JSON backend.
#[tokio::test]
async fn one_wsdl_full_loop_outbound_to_inbound() {
    let (backend, seen) = spawn_backend().await;
    // Pass 1: find the port we'll serve on, then build with it.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (app, _w, _d) = build_with_sidecar(&format!(
        "dsl: false\ninbound:\n  backend: {backend}\noutbound:\n  url: http://localhost:{port}/soap-in/demo/Svc\n"
    ))
    .await;
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let resp = reqwest::Client::new()
        .post(format!("http://localhost:{port}/soap-out/demo/Svc/Check"))
        .json(&json!({"Header": {"@technicalId": "loop-1"}, "Body": {"firstName": ["Carol", "Dave"], "code": "7"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(
        v["response"],
        json!({"Header": {"@technicalId": "loop-1"}, "Body": {"status": "OK <&>", "match": ["Carol", "Dave"]}})
    );
    assert_eq!(
        seen.lock().unwrap()[0].1["request"]["Body"]["firstName"],
        json!(["Carol", "Dave"])
    );
}

#[tokio::test]
async fn outbound_is_gated_by_inter_service_token_inbound_is_not() {
    let (peer, _seen) = spawn_soap_peer().await;
    let (backend, _s) = spawn_backend().await;
    let (app, _w, _d) = build_full(
        &format!("inbound:\n  backend: {backend}\noutbound:\n  url: {peer}\n"),
        Some("s3cret"),
        false,
    )
    .await;
    let (s, _, _) = call(&app, json_post("/soap-out/demo/Svc/Check", json!({}))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let req = Request::post("/soap-out/demo/Svc/Check")
        .header("authorization", "Bearer s3cret")
        .body(Body::from("{}"))
        .unwrap();
    let (s, _, _) = call(&app, req).await;
    assert_eq!(s, StatusCode::OK);
    // SOAP peers can't send bearer tokens — inbound stays reachable.
    let (s, _, _) = call(
        &app,
        soap("urn:demo/Notify", "<d:Notify><d:text>x</d:text></d:Notify>"),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn backend_5xx_detail_is_hidden_unless_exposed() {
    let (backend, _seen) = spawn_backend().await;
    let body = r#"<d:Check_Request><d:Header technicalId="1"/><d:Body><d:firstName>A</d:firstName><d:code>BOOM</d:code></d:Body></d:Check_Request>"#;
    let sidecar = format!("inbound:\n  backend: {backend}\n");

    let (app, _w, _d) = build_full(&sidecar, None, false).await;
    let (s, b, _) = call(&app, soap("urn:demo/Check", body)).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(b.contains("<faultcode>SOAP-ENV:Server</faultcode><faultstring>backend returned HTTP 500</faultstring>"), "{b}");
    assert!(!b.contains("db-internal"), "internal detail leaked: {b}");

    let (app, _w, _d) = build_full(&sidecar, None, true).await;
    let (_, b, _) = call(&app, soap("urn:demo/Check", body)).await;
    assert!(
        b.contains("backend returned HTTP 500: connection to db-internal-7:5432 refused"),
        "{b}"
    );
}

#[test]
fn switching_to_dsl_false_retires_previously_generated_dsls() {
    let wsdl_dir = TempDir::new().unwrap();
    let dsl_dir = TempDir::new().unwrap();
    let g = wsdl_dir.path().join("demo");
    std::fs::create_dir_all(&g).unwrap();
    std::fs::write(g.join("Svc.wsdl"), WSDL).unwrap();
    let cfg = AppConfig::default();
    xtr_on_rust::wsdl::ingest_all(wsdl_dir.path(), dsl_dir.path(), &cfg.wsdl, &cfg.client_data)
        .unwrap();
    assert!(dsl_dir.path().join("demo/Check.yml").exists());
    assert!(dsl_dir.path().join("demo/Notify.yml").exists());
    // A hand-written override (no marker) at a generated path survives.
    std::fs::write(
        dsl_dir.path().join("demo/Notify.yml"),
        "method: POST\nenvelope: x\n",
    )
    .unwrap();
    std::fs::write(
        g.join("Svc.soap.yaml"),
        "dsl: false\ninbound:\n  backend: http://b\n",
    )
    .unwrap();
    xtr_on_rust::wsdl::ingest_all(wsdl_dir.path(), dsl_dir.path(), &cfg.wsdl, &cfg.client_data)
        .unwrap();
    assert!(
        !dsl_dir.path().join("demo/Check.yml").exists(),
        "generated DSL retired"
    );
    assert!(
        dsl_dir.path().join("demo/Notify.yml").exists(),
        "hand-written DSL kept"
    );
    let services = loader::load_all(dsl_dir.path()).unwrap();
    assert!(!services.contains_key(&("demo".to_string(), "Check".to_string())));
}

fn ingest(wsdl_dir: &TempDir, dsl_dir: &TempDir) {
    let cfg = AppConfig::default();
    xtr_on_rust::wsdl::ingest_all(wsdl_dir.path(), dsl_dir.path(), &cfg.wsdl, &cfg.client_data)
        .unwrap();
}

#[test]
fn dsl_false_cleanup_leaves_same_named_endpoints_of_other_wsdls_alone() {
    // demo/a.wsdl (active) and demo/z.wsdl (dsl: false) declare the same
    // operations → same DSL file names. z is processed after a.
    let wsdl_dir = TempDir::new().unwrap();
    let dsl_dir = TempDir::new().unwrap();
    let g = wsdl_dir.path().join("demo");
    std::fs::create_dir_all(&g).unwrap();
    std::fs::write(g.join("a.wsdl"), WSDL).unwrap();
    std::fs::write(g.join("z.wsdl"), WSDL).unwrap();
    std::fs::write(g.join("z.soap.yaml"), "dsl: false\n").unwrap();
    ingest(&wsdl_dir, &dsl_dir);
    ingest(&wsdl_dir, &dsl_dir);
    let services = loader::load_all(dsl_dir.path()).unwrap();
    assert_eq!(
        services.len(),
        2,
        "a's Check + Notify must survive z's cleanup"
    );
    let text = std::fs::read_to_string(dsl_dir.path().join("demo/Check.yml")).unwrap();
    assert!(
        text.lines().nth(1) == Some("# source: demo/a.wsdl"),
        "{text}"
    );
}

#[test]
fn dsl_false_retires_endpoints_of_operations_removed_from_the_wsdl() {
    let wsdl_dir = TempDir::new().unwrap();
    let dsl_dir = TempDir::new().unwrap();
    let g = wsdl_dir.path().join("demo");
    std::fs::create_dir_all(&g).unwrap();
    std::fs::write(g.join("Svc.wsdl"), WSDL).unwrap();
    ingest(&wsdl_dir, &dsl_dir);
    assert!(dsl_dir.path().join("demo/Notify.yml").exists());
    // Drop Notify from the contract, then switch the WSDL to dsl: false.
    let without_notify = WSDL
        .replace(r#"<wsdl:operation name="Notify"><wsdl:input message="tns:NotifyIn"/></wsdl:operation>"#, "")
        .replace(r#"<wsdl:operation name="Notify"><soap:operation soapAction="urn:demo/Notify"/></wsdl:operation>"#, "");
    assert!(!without_notify.contains(r#"operation name="Notify""#));
    std::fs::write(g.join("Svc.wsdl"), without_notify).unwrap();
    std::fs::write(g.join("Svc.soap.yaml"), "dsl: false\n").unwrap();
    ingest(&wsdl_dir, &dsl_dir);
    assert!(!dsl_dir.path().join("demo/Check.yml").exists());
    assert!(
        !dsl_dir.path().join("demo/Notify.yml").exists(),
        "removed op's DSL retired too"
    );
    assert!(loader::load_all(dsl_dir.path()).unwrap().is_empty());
}

#[test]
fn dsl_false_still_retires_generated_files_from_older_versions() {
    // Files generated before provenance existed: marker only, no source line.
    let wsdl_dir = TempDir::new().unwrap();
    let dsl_dir = TempDir::new().unwrap();
    let g = wsdl_dir.path().join("demo");
    std::fs::create_dir_all(&g).unwrap();
    std::fs::write(g.join("Svc.wsdl"), WSDL).unwrap();
    ingest(&wsdl_dir, &dsl_dir);
    for f in ["Check", "Notify"] {
        let p = dsl_dir.path().join(format!("demo/{f}.yml"));
        let t = std::fs::read_to_string(&p).unwrap();
        let legacy: Vec<&str> = t.lines().filter(|l| !l.starts_with("# source: ")).collect();
        std::fs::write(&p, legacy.join("\n") + "\n").unwrap();
    }
    std::fs::write(g.join("Svc.soap.yaml"), "dsl: false\n").unwrap();
    ingest(&wsdl_dir, &dsl_dir);
    assert!(loader::load_all(dsl_dir.path()).unwrap().is_empty());
}
