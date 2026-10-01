//! Publishing an existing REST/JSON backend as an X-Road SOAP service.
//!
//! Synthetic contract in the shape of a typical X-Road v4 (SOAP)
//! provider WSDL (`PersonCheck`): unqualified schema,
//! `<request>`/`<response>` wrappers, X-Road headers declared in the
//! binding, `<xrd:version>`. The backend behaves like a Ruuter flow
//! written for the X-Road REST protocol: a guard on the `X-Road-Client`
//! HTTP header, flat JSON input, result wrapped as `{"response": "<JSON>"}`,
//! 400/403 with `{"error","message"}` on rejection.

use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tower::ServiceExt;
use xtr_on_rust::config::AppConfig;
use xtr_on_rust::executor::Executor;
use xtr_on_rust::inbound;
use xtr_on_rust::router::{self, AppState};

const WSDL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<wsdl:definitions name="demo" targetNamespace="http://demo.x-road.eu" xmlns:tns="http://demo.x-road.eu"
    xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" xmlns:xrd="http://x-road.eu/xsd/xroad.xsd"
    xmlns:id="http://x-road.eu/xsd/identifiers" xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/">
  <wsdl:types>
    <xs:schema targetNamespace="http://demo.x-road.eu" xmlns:xs="http://www.w3.org/2001/XMLSchema">
      <xs:import namespace="http://x-road.eu/xsd/xroad.xsd" schemaLocation="http://x-road.eu/xsd/xroad.xsd"/>
      <xs:element name="PersonCheck"><xs:complexType><xs:sequence>
        <xs:element name="request" type="tns:PersonCheckRequestType"/>
      </xs:sequence></xs:complexType></xs:element>
      <xs:element name="PersonCheckResponse"><xs:complexType><xs:sequence>
        <xs:element name="request" type="tns:PersonCheckRequestType"/>
        <xs:element name="response" type="tns:PersonCheckResponseType"/>
      </xs:sequence></xs:complexType></xs:element>
      <xs:complexType name="PersonCheckRequestType"><xs:sequence>
        <xs:element name="personCode" type="xs:string"/>
      </xs:sequence></xs:complexType>
      <xs:complexType name="PersonCheckResponseType"><xs:sequence>
        <xs:element name="checks"><xs:complexType><xs:sequence>
          <xs:element name="item" minOccurs="0" maxOccurs="unbounded"><xs:complexType><xs:sequence>
            <xs:element name="date" type="xs:dateTime"/>
            <xs:element name="title" type="xs:string"/>
          </xs:sequence></xs:complexType></xs:element>
        </xs:sequence></xs:complexType></xs:element>
      </xs:sequence></xs:complexType>
    </xs:schema>
  </wsdl:types>
  <wsdl:message name="requestheader">
    <wsdl:part name="client" element="xrd:client"/><wsdl:part name="service" element="xrd:service"/>
    <wsdl:part name="userId" element="xrd:userId"/><wsdl:part name="id" element="xrd:id"/>
    <wsdl:part name="protocolVersion" element="xrd:protocolVersion"/>
  </wsdl:message>
  <wsdl:message name="PersonCheckInputMessage"><wsdl:part name="body" element="tns:PersonCheck"/></wsdl:message>
  <wsdl:message name="PersonCheckOutputMessage"><wsdl:part name="body" element="tns:PersonCheckResponse"/></wsdl:message>
  <wsdl:portType name="demo_porttype">
    <wsdl:operation name="PersonCheck">
      <wsdl:input message="tns:PersonCheckInputMessage"/><wsdl:output message="tns:PersonCheckOutputMessage"/>
    </wsdl:operation>
  </wsdl:portType>
  <wsdl:binding name="demo_binding" type="tns:demo_porttype">
    <soap:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/>
    <wsdl:operation name="PersonCheck">
      <soap:operation soapAction=""/>
      <xrd:version>v1</xrd:version>
      <wsdl:input>
        <soap:header message="tns:requestheader" part="client" use="literal"/>
        <soap:body use="literal"/>
      </wsdl:input>
      <wsdl:output><soap:body use="literal"/></wsdl:output>
    </wsdl:operation>
  </wsdl:binding>
  <wsdl:service name="demoService"><wsdl:port name="demoServicePort" binding="tns:demo_binding">
    <soap:address location="http://TURVASERVER/cgi-bin/consumer_proxy"/>
  </wsdl:port></wsdl:service>
</wsdl:definitions>"#;

const SIDECAR: &str = r#"dsl: false
inbound:
  operations:
    PersonCheck: {backend: "{BACKEND}/demo/person-check"}
  payload: request
  request_pointer: /request
  response_pointer: /response
  response_wrap: {request: request, response: backend}
"#;

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<(HeaderMap, Value)>>>);

/// Ruuter-flow look-alike (guard + validation + wrapped string result).
async fn spawn_backend() -> (String, Seen) {
    async fn person_check(
        State(seen): State<Seen>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> axum::response::Response {
        seen.0.lock().unwrap().push((headers.clone(), body.clone()));
        let client = headers
            .get("x-road-client")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if client.split('/').filter(|p| !p.is_empty()).count() != 4 {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error": "FORBIDDEN", "message": "X-Road-Client header is missing or has invalid format"})),
            )
                .into_response();
        }
        let code = body["personCode"].as_str().unwrap_or("");
        if code.len() != 11 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "INVALID_PARAMETER", "message": "personCode must be 11 digits"})),
            )
                .into_response();
        }
        let result = json!({"checks": {"item": [{"title": "Roadside check", "date": "2026-09-01T10:00:00"}]}});
        Json(json!({"response": result.to_string()})).into_response()
    }
    let seen = Seen::default();
    let app = Router::new()
        .route("/demo/person-check", post(person_check))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

struct Fixture {
    app: Router,
    isolated: Router,
    _dir: TempDir,
}

async fn build(backend: &str) -> Fixture {
    let dir = TempDir::new().unwrap();
    let g = dir.path().join("demo");
    std::fs::create_dir_all(&g).unwrap();
    std::fs::write(g.join("person.wsdl"), WSDL).unwrap();
    std::fs::write(
        g.join("person.soap.yaml"),
        SIDECAR.replace("{BACKEND}", backend),
    )
    .unwrap();
    let cfg = Arc::new(AppConfig {
        dsl_path: dir.path().join("DSL"),
        ..Default::default()
    });
    let registry = inbound::load_all(Some(dir.path()), &cfg).unwrap();
    let lane = inbound::handler::LaneState::new(registry, cfg.clone(), false, None).unwrap();
    let state = AppState {
        cfg: cfg.clone(),
        services: Arc::new(Default::default()),
        executor: Executor::new(&cfg).unwrap(),
        openapi_spec: Arc::new(json!({})),
        inter_service_token: None,
    };
    Fixture {
        app: router::build_with(
            state.clone(),
            Some(
                inbound::handler::outbound_router(lane.clone())
                    .merge(inbound::handler::inbound_router(lane.clone())),
            ),
        ),
        isolated: router::build_isolated(state, inbound::handler::inbound_router(lane)),
        _dir: dir,
    }
}

const XROAD_HEADER: &str = r#"<xrd:client xmlns:xrd="http://x-road.eu/xsd/xroad.xsd" xmlns:id="http://x-road.eu/xsd/identifiers" id:objectType="SUBSYSTEM"><id:xRoadInstance>ee-dev</id:xRoadInstance><id:memberClass>GOV</id:memberClass><id:memberCode>70000001</id:memberCode><id:subsystemCode>consumer</id:subsystemCode></xrd:client><xrd:service xmlns:xrd="http://x-road.eu/xsd/xroad.xsd" xmlns:id="http://x-road.eu/xsd/identifiers" id:objectType="SERVICE"><id:xRoadInstance>ee-dev</id:xRoadInstance><id:memberClass>GOV</id:memberClass><id:memberCode>70000000</id:memberCode><id:subsystemCode>demo</id:subsystemCode><id:serviceCode>PersonCheck</id:serviceCode><id:serviceVersion>v1</id:serviceVersion></xrd:service><xrd:id xmlns:xrd="http://x-road.eu/xsd/xroad.xsd">4894e35d-bf0f-44a6-867a-8e51f1daa7e0</xrd:id><xrd:userId xmlns:xrd="http://x-road.eu/xsd/xroad.xsd">EE10000000001</xrd:userId><xrd:protocolVersion xmlns:xrd="http://x-road.eu/xsd/xroad.xsd">4.0</xrd:protocolVersion>"#;

fn soap(header: &str, person_code: &str) -> Request<Body> {
    Request::post("/soap-in/demo/person")
        .header("content-type", "text/xml; charset=utf-8")
        .header("SOAPAction", "\"\"")
        .body(Body::from(format!(
            r#"<SOAP-ENV:Envelope xmlns:SOAP-ENV="http://schemas.xmlsoap.org/soap/envelope/"><SOAP-ENV:Header>{header}</SOAP-ENV:Header><SOAP-ENV:Body><ns1:PersonCheck xmlns:ns1="http://demo.x-road.eu"><request><personCode>{person_code}</personCode></request></ns1:PersonCheck></SOAP-ENV:Body></SOAP-ENV:Envelope>"#
        )))
        .unwrap()
}

async fn call(app: &Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn xroad_soap_request_reaches_rest_style_backend_unchanged() {
    let (backend, seen) = spawn_backend().await;
    let f = build(&backend).await;
    let (status, body) = call(&f.app, soap(XROAD_HEADER, "10000000001")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (headers, json_in) = seen.0.lock().unwrap()[0].clone();
    // request_pointer=/request + payload=request → the flow's own shape.
    assert_eq!(json_in, json!({"personCode": "10000000001"}));
    // X-Road SOAP header → X-Road REST protocol headers (what the guard reads).
    assert_eq!(headers["x-road-client"], "ee-dev/GOV/70000001/consumer");
    assert_eq!(
        headers["x-road-service"],
        "ee-dev/GOV/70000000/demo/PersonCheck"
    );
    assert_eq!(headers["x-road-id"], "4894e35d-bf0f-44a6-867a-8e51f1daa7e0");
    assert_eq!(headers["x-road-userid"], "EE10000000001");

    // response_pointer unwraps the JSON string, response_wrap rebuilds the
    // X-Road v4 `{request, response}` shape; unqualified children.
    assert!(
        body.contains(r#"<tns:PersonCheckResponse xmlns:tns="http://demo.x-road.eu"><request><personCode>10000000001</personCode></request><response><checks><item><date>2026-09-01T10:00:00</date><title>Roadside check</title></item></checks></response></tns:PersonCheckResponse>"#),
        "{body}"
    );
    // Header echoed with its own namespaces intact.
    assert!(body.contains(r#"<xrd:id xmlns:xrd="http://x-road.eu/xsd/xroad.xsd">4894e35d-bf0f-44a6-867a-8e51f1daa7e0</xrd:id>"#), "{body}");
    assert!(
        body.contains(r#"<id:serviceCode>PersonCheck</id:serviceCode>"#),
        "{body}"
    );
}

#[tokio::test]
async fn backend_rejections_map_to_client_faults() {
    let (backend, _seen) = spawn_backend().await;
    let f = build(&backend).await;
    // No X-Road header → the guard's 403 → SOAP Client fault with its message.
    let (s, b) = call(&f.app, soap("", "10000000001")).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(b.contains("<faultcode>SOAP-ENV:Client</faultcode>"), "{b}");
    assert!(
        b.contains("HTTP 403: X-Road-Client header is missing"),
        "{b}"
    );
    // Validation 400 → Client fault.
    let (_, b) = call(&f.app, soap(XROAD_HEADER, "1")).await;
    assert!(b.contains("<faultcode>SOAP-ENV:Client</faultcode>"), "{b}");
    assert!(b.contains("personCode must be 11 digits"), "{b}");
}

#[tokio::test]
async fn isolated_inbound_listener_exposes_only_soap_in_and_health() {
    let (backend, _seen) = spawn_backend().await;
    let f = build(&backend).await;
    let (s, _) = call(&f.isolated, soap(XROAD_HEADER, "10000000001")).await;
    assert_eq!(s, StatusCode::OK);
    for (method, path) in [("GET", "/health")] {
        let (s, _) = call(
            &f.isolated,
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{path}");
    }
    for path in [
        "/soap-out/demo/person/PersonCheck",
        "/api",
        "/ariregister/lihtandmed_v3",
    ] {
        let (s, _) = call(
            &f.isolated,
            Request::post(path).body(Body::from("{}")).unwrap(),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::NOT_FOUND,
            "{path} must not be served on the inbound listener"
        );
    }
}

#[tokio::test]
async fn wsdl_address_rewrite_survives_unusual_formatting() {
    // Address with surrounding whitespace in the attribute: the textual
    // fast path can't match it → DOM rewrite fallback.
    let (backend, _seen) = spawn_backend().await;
    let dir = TempDir::new().unwrap();
    let g = dir.path().join("demo");
    std::fs::create_dir_all(&g).unwrap();
    std::fs::write(
        g.join("person.wsdl"),
        WSDL.replace(
            r#"location="http://TURVASERVER/cgi-bin/consumer_proxy""#,
            "location = \"http://TURVASERVER/cgi-bin/consumer_proxy\"",
        ),
    )
    .unwrap();
    std::fs::write(
        g.join("person.soap.yaml"),
        SIDECAR.replace("{BACKEND}", &backend),
    )
    .unwrap();
    let cfg = Arc::new(AppConfig::default());
    let lane = inbound::handler::LaneState::new(
        inbound::load_all(Some(dir.path()), &cfg).unwrap(),
        cfg.clone(),
        false,
        None,
    )
    .unwrap();
    let app: Router = inbound::handler::inbound_router(lane);
    let req = Request::get("/soap-in/demo/person?wsdl")
        .header("host", "xtr.example")
        .header("x-forwarded-proto", "https")
        .body(Body::empty())
        .unwrap();
    let (s, b) = call(&app, req).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        b.contains(r#"location="https://xtr.example/soap-in/demo/person""#),
        "{b}"
    );
    assert!(!b.contains("TURVASERVER"), "{b}");
    // The rewritten contract must still be a usable WSDL: QName-valued
    // attributes (element="tns:…", type="tns:…") keep their prefixes.
    let reparsed = inbound::contract::load(&b, &|_| None).expect("served WSDL must re-parse");
    let op = &reparsed.operations[0];
    assert_eq!(op.name, "PersonCheck");
    assert_eq!(op.input.ns.as_deref(), Some("http://demo.x-road.eu"));
    assert_eq!(
        reparsed.address.as_deref(),
        Some("https://xtr.example/soap-in/demo/person")
    );
}

#[tokio::test]
async fn non_ascii_xroad_header_value_still_reaches_backend() {
    let (backend, seen) = spawn_backend().await;
    let f = build(&backend).await;
    let hdr = format!(
        r#"{XROAD_HEADER}<xrd:issue xmlns:xrd="http://x-road.eu/xsd/xroad.xsd">Ticket ÄÖÜ-42 café</xrd:issue>"#
    );
    let (status, body) = call(&f.app, soap(&hdr, "10000000001")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (headers, _) = seen.0.lock().unwrap()[0].clone();
    assert_eq!(
        headers["x-road-issue"].as_bytes(),
        "Ticket ÄÖÜ-42 café".as_bytes()
    );
}
