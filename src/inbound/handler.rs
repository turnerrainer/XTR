//! HTTP surface of the inbound SOAP lane.
//!
//! Request path: SOAP 1.1 envelope → operation lookup by the Body's
//! first element QName (cross-checked against `SOAPAction`) → JSON →
//! backend POST → JSON → output element → SOAP envelope.
//!
//! Every error leaves as a SOAP 1.1 Fault (`text/xml`), never as the
//! JSON error shape of the REST-facing lanes — SOAP clients cannot
//! parse the latter.

use super::codec;
use super::contract::QName;
use super::dom::{self, Element};
use super::{PayloadMode, Registry, Service};
use crate::config::AppConfig;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, RawQuery, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use quick_xml::escape::escape;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

const SOAP11_ENV: &str = "http://schemas.xmlsoap.org/soap/envelope/";
const SOAP12_ENV: &str = "http://www.w3.org/2003/05/soap-envelope";
const FAULTSTRING_MAX: usize = 400;

#[derive(Clone)]
pub struct LaneState {
    pub registry: Arc<Registry>,
    pub cfg: Arc<AppConfig>,
    client: reqwest::Client,
    pub(super) offline: bool,
    inter_service_token: Option<Arc<String>>,
}

impl LaneState {
    pub fn new(
        registry: Registry,
        cfg: Arc<AppConfig>,
        offline: bool,
        inter_service_token: Option<Arc<String>>,
    ) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.limits.request_timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .build()?;
        Ok(Self {
            registry: Arc::new(registry),
            cfg,
            client,
            offline,
            inter_service_token,
        })
    }
}

/// Inbound routes (`/soap-in/…`) — called by SOAP peers (X-Road
/// Security Server, other SOAP peers) that cannot send a bearer token; protect
/// them at the ingress (TLS + client-certificate check / network policy).
pub fn inbound_router<S: Clone + Send + Sync + 'static>(state: LaneState) -> Router<S> {
    let limit = state.cfg.limits.max_request_bytes;
    Router::new()
        .route("/soap-in/:group/:name", get(get_contract).post(post_soap))
        .layer(DefaultBodyLimit::max(limit.saturating_add(4096)))
        .with_state(state)
}

/// Outbound routes (`/soap-out/…`) — make XTR act with its own identity
/// (client certificate, X-Road client), the same exposure as
/// `/:group/:service`, hence the same XTR_INTER_SERVICE_TOKEN gate.
/// Never expose these to the SOAP peer network.
pub fn outbound_router<S: Clone + Send + Sync + 'static>(state: LaneState) -> Router<S> {
    let limit = state.cfg.limits.max_request_bytes;
    let token = state.inter_service_token.clone();
    Router::new()
        .route(
            "/soap-out/:group/:name/:operation",
            post(super::outbound::post).layer(middleware::from_fn_with_state(
                token,
                crate::router::inter_service_token::apply,
            )),
        )
        .layer(DefaultBodyLimit::max(limit.saturating_add(4096)))
        .with_state(state)
}

// ---------------------------------------------------------------- GET

async fn get_contract(
    State(st): State<LaneState>,
    Path((group, name)): Path<(String, String)>,
    RawQuery(_q): RawQuery,
    headers: HeaderMap,
) -> Response {
    if name.ends_with(".xsd") {
        return match st.registry.schema_file(&group, &name) {
            Some(p) => match tokio::fs::read(p).await {
                Ok(bytes) => xml_response(StatusCode::OK, bytes),
                Err(_) => StatusCode::NOT_FOUND.into_response(),
            },
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }
    let Some(svc) = st
        .registry
        .get(&group, &name)
        .filter(|s| s.sidecar.inbound.is_some())
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let endpoint = format!("{}/soap-in/{group}/{name}", public_base(&st.cfg, &headers));
    xml_response(StatusCode::OK, rewrite_address(svc, &endpoint).into_bytes())
}

/// Point every `<service>/<port>/<…:address location>` at `endpoint`.
/// Fast path: textual replace keeps the operator's file byte-for-byte
/// (comments, formatting). If the address is written in a way the
/// replace doesn't match (other escaping, whitespace in the attribute),
/// fall back to rewriting the parsed document — never serve a stale
/// address silently.
fn rewrite_address(svc: &Service, endpoint: &str) -> String {
    let Some(old) = &svc.contract.address else {
        return svc.wsdl_text.clone();
    };
    let mut text = svc.wsdl_text.clone();
    for q in ['"', '\''] {
        text = text.replace(
            &format!("location={q}{}{q}", escape(old.as_str())),
            &format!("location={q}{}{q}", escape(endpoint)),
        );
    }
    if text != svc.wsdl_text {
        return text;
    }
    match dom::parse(&svc.wsdl_text) {
        Ok(mut root) => {
            set_address(&mut root, endpoint, false);
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}",
                dom::serialize(&root)
            )
        }
        Err(_) => svc.wsdl_text.clone(),
    }
}

fn set_address(el: &mut Element, endpoint: &str, in_service: bool) {
    let in_service = in_service || el.local == "service";
    if in_service && el.local == "address" {
        for a in el
            .attrs
            .iter_mut()
            .filter(|a| a.local == "location" && a.ns.is_none())
        {
            a.value = endpoint.to_string();
        }
    }
    for c in el.children.iter_mut() {
        if let dom::Node::Elem(child) = c {
            set_address(child, endpoint, in_service);
        }
    }
}

fn public_base(cfg: &AppConfig, headers: &HeaderMap) -> String {
    if let Some(b) = &cfg.inbound.public_base_url {
        return b.trim_end_matches('/').to_string();
    }
    // Host header is caller-controlled: only reflect plain host[:port].
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| {
            !h.is_empty()
                && h.len() <= 255
                && h.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
        })
        .unwrap_or("localhost");
    // Behind a TLS-terminating ingress the request arrives as http.
    let scheme = match headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
    {
        Some(p) if p.eq_ignore_ascii_case("https") => "https",
        _ => "http",
    };
    format!("{scheme}://{host}")
}

// --------------------------------------------------------------- POST

async fn post_soap(
    State(st): State<LaneState>,
    Path((group, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some((svc, ib)) = st
        .registry
        .get(&group, &name)
        .cloned()
        .and_then(|s| s.sidecar.inbound.clone().map(|ib| (s, ib)))
    else {
        return fault(
            StatusCode::NOT_FOUND,
            "Client",
            "no inbound SOAP service at this path",
            None,
            None,
        );
    };
    let limit = st.cfg.limits.max_request_bytes;
    if body.len() > limit {
        return fault(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Client",
            &format!("request exceeds {limit} bytes"),
            None,
            None,
        );
    }
    let decoded = match decode_request(&svc, &headers, &body) {
        Ok(d) => d,
        Err((code, msg)) => {
            return fault(StatusCode::INTERNAL_SERVER_ERROR, code, &msg, None, None)
        }
    };
    let echo = decoded.echo_header.as_deref();
    let Some(url) = ib.backend_for(&decoded.operation) else {
        return fault(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Server",
            &format!("no backend configured for operation {}", decoded.operation),
            None,
            echo,
        );
    };
    if st.offline {
        tracing::info!(op = ?decoded.operation, "inbound backend call blocked by XTR_OFFLINE");
        return fault(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Server",
            "XTR_OFFLINE: backend calls disabled",
            None,
            echo,
        );
    }
    let request_json = match ib.request_pointer.as_deref().filter(|p| !p.is_empty()) {
        None => decoded.request_json.clone(),
        Some(ptr) => match decoded.request_json.pointer(ptr) {
            Some(v) => v.clone(),
            None => {
                return fault(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Client",
                    &format!("request has nothing at {ptr}"),
                    None,
                    echo,
                )
            }
        },
    };
    let payload = match ib.payload {
        PayloadMode::Wrapped => json!({
            "service": format!("{group}/{name}"),
            "operation": decoded.operation,
            "soapAction": decoded.soap_action,
            "header": decoded.header_json,
            "request": request_json,
        }),
        PayloadMode::Request => request_json.clone(),
    };
    let mut req = st
        .client
        .post(&url)
        .header("x-xtr-inbound-service", format!("{group}/{name}"))
        .header("x-xtr-operation", decoded.operation.as_str())
        .json(&payload);
    if let Some(tp) = headers.get("traceparent") {
        req = req.header("traceparent", tp.clone());
    }
    if ib.forward_xroad_headers {
        for (name, value) in &decoded.xroad_headers {
            // from_bytes accepts UTF-8 (obs-text); from_str would reject
            // e.g. a non-ASCII X-Road-Issue and fail the whole request.
            match axum::http::HeaderValue::from_bytes(value.as_bytes()) {
                Ok(v) => req = req.header(*name, v),
                Err(_) => {
                    tracing::warn!(header = %name, "inbound: X-Road header value not forwardable — dropped")
                }
            }
        }
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(op = ?decoded.operation, url = ?url, error = ?e.to_string(), "inbound backend call failed");
            let msg = if e.is_timeout() {
                "backend timeout"
            } else if e.is_builder() {
                "invalid backend request"
            } else {
                "backend unreachable"
            };
            return fault(StatusCode::INTERNAL_SERVER_ERROR, "Server", msg, None, echo);
        }
    };
    let status = resp.status();
    let text = match crate::executor::plain::read_bounded(resp, st.cfg.limits.max_response_bytes)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(op = ?decoded.operation, error = ?e.to_string(), "inbound backend body unreadable");
            return fault(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Server",
                "backend response unreadable",
                None,
                echo,
            );
        }
    };
    encode_response(
        &svc,
        &ib,
        &decoded,
        &request_json,
        status,
        &text,
        st.cfg.expose_soap_fault_detail,
    )
}

struct Decoded {
    operation: String,
    output: Option<QName>,
    soap_action: Option<String>,
    header_json: Value,
    request_json: Value,
    echo_header: Option<String>,
    /// X-Road REST-protocol request headers derived from the X-Road
    /// SOAP header (empty when the request carries none).
    xroad_headers: Vec<(&'static str, String)>,
}

const XROAD_NS: &str = "http://x-road.eu/xsd/xroad.xsd";
const XROAD_ID_NS: &str = "http://x-road.eu/xsd/identifiers";

/// `<xrd:client>` / `<xrd:service>` → `inst/class/code[/subsystem][/serviceCode]`,
/// each segment percent-encoded as in the X-Road REST protocol §4.2.
fn xroad_identifier(el: &Element, parts: &[&str]) -> Option<String> {
    let segs: Vec<String> = parts
        .iter()
        .filter_map(|p| {
            el.elements()
                .find(|c| c.local == *p && c.ns.as_deref() == Some(XROAD_ID_NS))
                .map(|c| crate::executor::rest_lane::pct(c.text().trim()))
        })
        .collect();
    (segs.len() >= 3).then(|| segs.join("/"))
}

fn xroad_headers(hdr: Option<&Element>) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let Some(h) = hdr else { return out };
    let find = |local: &str| {
        h.elements()
            .find(|e| e.local == local && e.ns.as_deref() == Some(XROAD_NS))
    };
    if let Some(v) = find("client").and_then(|c| {
        xroad_identifier(
            c,
            &[
                "xRoadInstance",
                "memberClass",
                "memberCode",
                "subsystemCode",
            ],
        )
    }) {
        out.push(("x-road-client", v));
    }
    if let Some(v) = find("service").and_then(|c| {
        xroad_identifier(
            c,
            &[
                "xRoadInstance",
                "memberClass",
                "memberCode",
                "subsystemCode",
                "serviceCode",
            ],
        )
    }) {
        out.push(("x-road-service", v));
    }
    for (local, header) in [
        ("id", "x-road-id"),
        ("userId", "x-road-userid"),
        ("issue", "x-road-issue"),
    ] {
        if let Some(v) = find(local).map(|e| e.text().trim().to_string()) {
            // Header values must not carry CR/LF — reqwest would refuse
            // the whole request; drop the one bad value instead.
            if !v.is_empty() && !v.chars().any(|c| c.is_control()) {
                out.push((header, v));
            }
        }
    }
    out
}

fn decode_request(
    svc: &Service,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Decoded, (&'static str, String)> {
    let xml = std::str::from_utf8(body)
        .map_err(|_| ("Client", "request body is not UTF-8".to_string()))?;
    let env = dom::parse(xml).map_err(|e| ("Client", format!("malformed XML: {}", e.0)))?;
    if env.local != "Envelope" {
        return Err(("Client", "root element is not a SOAP Envelope".into()));
    }
    match env.ns.as_deref() {
        Some(SOAP11_ENV) => {}
        Some(SOAP12_ENV) => {
            return Err((
                "VersionMismatch",
                "SOAP 1.2 is not supported by this endpoint; use SOAP 1.1".into(),
            ))
        }
        _ => return Err(("VersionMismatch", "unknown SOAP envelope namespace".into())),
    }
    let in_env = |e: &&Element| e.ns.as_deref() == Some(SOAP11_ENV);
    let hdr = env.elements().filter(in_env).find(|e| e.local == "Header");
    let body_el = env
        .elements()
        .filter(in_env)
        .find(|e| e.local == "Body")
        .ok_or(("Client", "SOAP Body is missing".to_string()))?;
    let payload = body_el
        .elements()
        .next()
        .ok_or(("Client", "SOAP Body is empty".to_string()))?;
    let q = QName {
        ns: payload.ns.clone(),
        local: payload.local.clone(),
    };
    let op = svc.contract.op_for_input(&q).ok_or_else(|| {
        (
            "Client",
            format!("no operation of this service accepts {q}"),
        )
    })?;
    let action = headers
        .get("soapaction")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().trim_matches('"').to_string());
    if let (Some(got), Some(want)) = (action.as_deref(), op.soap_action.as_deref()) {
        if !got.is_empty() && !want.is_empty() && got != want {
            return Err((
                "Client",
                format!(
                    "SOAPAction {got:?} does not match operation {} ({want:?})",
                    op.name
                ),
            ));
        }
    }
    let schema = &svc.contract.schema;
    let td = schema.element(&op.input).and_then(|t| schema.resolve(t));
    let header_json = hdr
        .map(|h| codec::element_to_json(h, None, schema))
        .unwrap_or(Value::Null);
    let echo = svc
        .sidecar
        .inbound
        .as_ref()
        .map(|i| i.echo_soap_header)
        .unwrap_or(true);
    let echo_header = if echo {
        hdr.map(|h| h.elements().map(dom::serialize).collect::<String>())
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    Ok(Decoded {
        operation: op.name.clone(),
        output: op.output.clone(),
        soap_action: action,
        header_json,
        request_json: codec::element_to_json(payload, td, schema),
        echo_header,
        xroad_headers: xroad_headers(hdr),
    })
}

fn encode_response(
    svc: &Service,
    ib: &super::InboundCfg,
    d: &Decoded,
    request_json: &Value,
    status: reqwest::StatusCode,
    text: &str,
    expose_detail: bool,
) -> Response {
    let echo = d.echo_header.as_deref();
    let schema = &svc.contract.schema;
    let parsed: Option<Value> = if text.trim().is_empty() {
        None
    } else {
        serde_json::from_str(text).ok()
    };
    // Backend-declared fault — honoured on any status.
    if let Some(f) = parsed
        .as_ref()
        .and_then(|v| v.get("fault"))
        .filter(|f| f.is_object())
    {
        return backend_fault(f, schema, echo);
    }
    if !status.is_success() {
        // Look for `message`/`error` at the top level and, for wrapping
        // backends (Ruuter), under `response_pointer` too.
        let pointed = parsed
            .as_ref()
            .zip(ib.response_pointer.as_deref())
            .and_then(|(v, p)| v.pointer(p))
            .map(|v| match v {
                Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
                other => other.clone(),
            });
        let msg = [parsed.as_ref(), pointed.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|v| {
                v.get("message")
                    .or_else(|| v.get("error"))
                    .and_then(|m| m.as_str())
                    .map(String::from)
            })
            // 4xx is a business rejection addressed to the caller — its
            // message goes out. 5xx text is backend-internal: kept out of
            // the faultstring unless `expose_soap_fault_detail` (audit-v1
            // H3 posture), always logged.
            .filter(|_| status.is_client_error() || expose_detail)
            .map(|m| format!("backend returned HTTP {}: {m}", status.as_u16()))
            .unwrap_or_else(|| format!("backend returned HTTP {}", status.as_u16()));
        if status.is_server_error() {
            tracing::warn!(status = status.as_u16(), body = ?text.chars().take(1024).collect::<String>(),
                "inbound backend returned a server error");
        }
        // 4xx = the caller's request was rejected → Client; else Server.
        let code = if status.is_client_error() {
            "Client"
        } else {
            "Server"
        };
        return fault(StatusCode::INTERNAL_SERVER_ERROR, code, &msg, None, echo);
    }
    let Some(output) = &d.output else {
        // One-way operation: acknowledge receipt only.
        return StatusCode::ACCEPTED.into_response();
    };
    let mut value = match parsed {
        Some(v) => v,
        None if text.trim().is_empty() => Value::Object(Default::default()),
        None => {
            return fault(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Server",
                "backend returned non-JSON body",
                None,
                echo,
            )
        }
    };
    if let Some(ptr) = ib.response_pointer.as_deref().filter(|p| !p.is_empty()) {
        value = match value.pointer(ptr) {
            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::String(s.clone())),
            Some(v) => v.clone(),
            None => {
                return fault(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Server",
                    &format!("backend response has nothing at {ptr}"),
                    None,
                    echo,
                )
            }
        };
        if let Some(f) = value.get("fault").filter(|f| f.is_object()) {
            return backend_fault(f, schema, echo);
        }
    }
    if let Some(wrap) = &ib.response_wrap {
        let mut obj = serde_json::Map::new();
        for (key, src) in wrap {
            let v = match src {
                super::WrapSource::Request => request_json.clone(),
                super::WrapSource::Backend => value.clone(),
            };
            obj.insert(key.clone(), v);
        }
        value = Value::Object(obj);
    }
    let body = codec::json_to_root_element(output, &value, schema);
    xml_response(StatusCode::OK, envelope(echo, &body).into_bytes())
}

/// `{"fault": {"code": "Client|Server", "string": "…", "detail": {"<Elem>": {…}}}}`
fn backend_fault(f: &Value, schema: &super::contract::Schema, echo: Option<&str>) -> Response {
    let code = f.get("code").and_then(|c| c.as_str()).unwrap_or("Server");
    let string = f
        .get("string")
        .and_then(|s| s.as_str())
        .unwrap_or("backend fault");
    let detail = f.get("detail").and_then(|d| d.as_object()).map(|d| {
        d.iter()
            .map(|(k, v)| codec::json_to_plain(k, v, schema))
            .collect::<String>()
    });
    fault(
        StatusCode::INTERNAL_SERVER_ERROR,
        code,
        string,
        detail.as_deref(),
        echo,
    )
}

pub(super) fn envelope(header: Option<&str>, body: &str) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<SOAP-ENV:Envelope xmlns:SOAP-ENV=\"http://schemas.xmlsoap.org/soap/envelope/\">",
    );
    if let Some(h) = header {
        s.push_str("<SOAP-ENV:Header>");
        s.push_str(h);
        s.push_str("</SOAP-ENV:Header>");
    }
    s.push_str("<SOAP-ENV:Body>");
    s.push_str(body);
    s.push_str("</SOAP-ENV:Body></SOAP-ENV:Envelope>");
    s
}

fn fault(
    status: StatusCode,
    code: &str,
    string: &str,
    detail: Option<&str>,
    header: Option<&str>,
) -> Response {
    let code = match code {
        "Client" | "Server" | "VersionMismatch" | "MustUnderstand" => code,
        _ => "Server",
    };
    let clipped: String = string
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .take(FAULTSTRING_MAX)
        .collect();
    let mut body = format!(
        "<SOAP-ENV:Fault><faultcode>SOAP-ENV:{code}</faultcode><faultstring>{}</faultstring>",
        escape(clipped.as_str())
    );
    if let Some(d) = detail.filter(|d| !d.is_empty()) {
        body.push_str("<detail>");
        body.push_str(d);
        body.push_str("</detail>");
    }
    body.push_str("</SOAP-ENV:Fault>");
    xml_response(status, envelope(header, &body).into_bytes())
}

fn xml_response(status: StatusCode, body: Vec<u8>) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/xml; charset=utf-8")],
        body,
    )
        .into_response()
}
