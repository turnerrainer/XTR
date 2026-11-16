//! HTTP surface of the raw-XML lanes.
//!
//! Inbound: raw XML POST → root element picks the message type →
//! XSD-guided JSON → backend POST → backend status mapped back to the
//! peer. Outbound: JSON → raw XML → POST to the peer → upstream status
//! and body returned as JSON.
//!
//! The lane is stateless: it keeps nothing between requests. Whether a
//! message is a replay, when it expires and what to retry is the
//! backend's / caller's business — XTR hands them the received bytes
//! and their SHA-256 digest so they can decide.

use super::{LogLimit, ValidateMode, XmlRegistry, XmlService};
use crate::config::AppConfig;
use crate::error::XtrError;
use crate::inbound::{codec, dom};
use crate::xsdval::{self, Model, Violation};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;

/// Set on every forwarded request. An inbound request that has it is a loop.
const FORWARDED_HEADER: &str = "x-xtr-forwarded";

#[derive(Clone)]
pub struct XmlLaneState {
    pub registry: Arc<XmlRegistry>,
    pub cfg: Arc<AppConfig>,
    backend: reqwest::Client,
    offline: bool,
    inter_service_token: Option<Arc<String>>,
}

impl XmlLaneState {
    pub fn new(
        registry: XmlRegistry,
        cfg: Arc<AppConfig>,
        offline: bool,
        inter_service_token: Option<Arc<String>>,
    ) -> anyhow::Result<Self> {
        let backend = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.limits.request_timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .build()?;
        Ok(Self {
            registry: Arc::new(registry),
            cfg,
            backend,
            offline,
            inter_service_token,
        })
    }
}

/// Inbound routes — called by the XML peer (via the ingress), which
/// cannot send a bearer token: protect them at the ingress / network.
pub fn inbound_router<S: Clone + Send + Sync + 'static>(state: XmlLaneState) -> Router<S> {
    let limit = state.cfg.limits.max_request_bytes;
    let mut router = Router::new().route("/xml-in/:group/:name", post(post_canonical));
    for svc in state.registry.iter() {
        let Some(path) = svc.sidecar.inbound.as_ref().and_then(|i| i.path.clone()) else {
            continue;
        };
        let (group, name) = (svc.group.clone(), svc.name.clone());
        router = router.route(
            &path,
            post(
                move |State(st): State<XmlLaneState>, headers: HeaderMap, body: Bytes| {
                    let (group, name) = (group.clone(), name.clone());
                    async move { handle_inbound(&st, &group, &name, &headers, &body).await }
                },
            ),
        );
    }
    router
        .layer(DefaultBodyLimit::max(limit.saturating_add(4096)))
        .with_state(state)
}

/// Outbound routes — make XTR act with its own identity, the same
/// exposure as `/:group/:service`, hence the same token gate.
pub fn outbound_router<S: Clone + Send + Sync + 'static>(state: XmlLaneState) -> Router<S> {
    let limit = state.cfg.limits.max_request_bytes;
    let token = state.inter_service_token.clone();
    Router::new()
        .route(
            "/xml-out/:group/:name/:message",
            post(post_outbound).layer(middleware::from_fn_with_state(
                token,
                crate::router::inter_service_token::apply,
            )),
        )
        .layer(DefaultBodyLimit::max(limit.saturating_add(4096)))
        .with_state(state)
}

// ------------------------------------------------------------ inbound

fn err(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({"error": code, "message": message}))).into_response()
}

fn violation_json(v: &Violation) -> Value {
    json!({
        "path": v.path,
        "code": v.code,
        "line": v.line,
        "schemaFile": v.schema_file,
        "schemaLine": v.schema_line,
    })
}

fn schema_invalid(violations: &[Violation]) -> Response {
    let list: Vec<Value> = violations.iter().map(violation_json).collect();
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({
            "error": "schema_invalid",
            "message": "the message does not conform to its XSD",
            "violations": list,
        })),
    )
        .into_response()
}

/// The validation gate of one message. `Err` is an internal error, not a
/// verdict: validation is on but the lane has no model, or the validator
/// failed. A lane must never skip validation in silence.
fn gate(
    mode: ValidateMode,
    model: Option<&Model>,
    root: &dom::Element,
) -> Result<Vec<Violation>, ()> {
    match (mode, model) {
        (ValidateMode::Off, _) => Ok(Vec::new()),
        (_, None) => Err(()),
        (_, Some(m)) => run_validator(m, root),
    }
}

/// Runs the validator. A panic inside it is an internal error, not a
/// verdict on the message. `catch_unwind` needs `panic = "unwind"`, the
/// default of Cargo profiles. With `panic = "abort"` a panic would stop the
/// process.
fn run_validator(model: &Model, root: &dom::Element) -> Result<Vec<Violation>, ()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        xsdval::validate(model, root)
    }))
    .map_err(|_| ())
}

/// Applies the validation mode to the violations of one message.
/// `Some(response)` means: stop and answer with it.
fn judge(
    mode: ValidateMode,
    group: &str,
    name: &str,
    message: &str,
    direction: &str,
    violations: &[Violation],
    limit: &LogLimit,
) -> Option<Response> {
    if violations.is_empty() {
        return None;
    }
    // Paths and codes only. Never a value from the message.
    let summary: Vec<String> = violations
        .iter()
        .map(|v| format!("{}:{}", v.path, v.code))
        .collect();
    // The log is rate capped per lane. The answer is not.
    let log = limit.check();
    if mode == ValidateMode::Enforce {
        if let Some(suppressed) = log {
            tracing::warn!(group, name, msg_type = message, direction, violations = ?summary, suppressed, "XSD validation refused the message");
        }
        Some(schema_invalid(violations))
    } else {
        if let Some(suppressed) = log {
            tracing::warn!(group, name, msg_type = message, direction, violations = ?summary, suppressed, "XSD validation (report): the message does not conform");
        }
        None
    }
}

async fn post_canonical(
    State(st): State<XmlLaneState>,
    Path((group, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle_inbound(&st, &group, &name, &headers, &body).await
}

/// Read at most `limit` bytes; never fails because of the content: a
/// non-UTF-8 body is decoded lossily, an oversized one is cut and
/// flagged. The caller already holds the status — losing it to a body
/// problem would make it retry a message the peer did accept.
/// Returns `(text, truncated, read_failed)`.
async fn read_capped(mut resp: reqwest::Response, limit: usize) -> (Vec<u8>, bool, bool) {
    let mut buf: Vec<u8> = Vec::new();
    let (mut truncated, mut failed) = (false, false);
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let room = limit.saturating_sub(buf.len());
                if chunk.len() > room {
                    buf.extend_from_slice(&chunk[..room]);
                    truncated = true;
                    break;
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    (buf, truncated, failed)
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// `validate.inbound: annotate`. The backend always gets the verdict and
/// owns the answer. Input that XTR cannot read as a message of the lane
/// goes to `<inbound.backend>/_invalid`. A message that breaks the XSD goes
/// to its own backend URL with `schemaValid: false`. The invalid input is
/// never decoded and never sent as text: only `rawBase64` carries it.
/// 413 and `XTR_OFFLINE` stay HTTP answers.
async fn handle_annotated(
    st: &XmlLaneState,
    svc: &XmlService,
    group: &str,
    name: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    use base64::Engine;
    let ib = svc.sidecar.inbound.as_ref().expect("checked by the caller");
    let service = format!("{group}/{name}");
    if st.offline {
        tracing::info!(
            group,
            name,
            "inbound XML backend call blocked by XTR_OFFLINE"
        );
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "offline",
            "XTR_OFFLINE: backend calls disabled",
        );
    }
    let digest = hex(&Sha256::digest(body));
    // Input that is not a message of the lane goes to `_invalid`.
    let text = std::str::from_utf8(body).map_err(|_| "unsupported_encoding");
    let parsed = match text {
        Ok(t) => dom::parse_message(t).map_err(|e| match e.1 {
            dom::DomErrorKind::Encoding => "unsupported_encoding",
            dom::DomErrorKind::Doctype => "doctype",
            dom::DomErrorKind::Malformed => "malformed_xml",
        }),
        Err(c) => Err(c),
    };
    let root = match parsed {
        Ok(r) => r,
        Err(code) => {
            return send_invalid(
                st,
                svc,
                ib,
                headers,
                (group, name),
                &service,
                &digest,
                body,
                code,
            )
            .await;
        }
    };
    let Some(msg) = svc
        .messages
        .get(&root.local)
        .filter(|m| m.qname.ns == root.ns && m.cfg.direction.inbound())
    else {
        return send_invalid(
            st,
            svc,
            ib,
            headers,
            (group, name),
            &service,
            &digest,
            body,
            "unsupported_message",
        )
        .await;
    };
    let violations = match gate(ValidateMode::Annotate, svc.model.as_deref(), &root) {
        Ok(v) => v,
        Err(()) => {
            tracing::error!(group, name, msg_type = ?root.local, "XSD validator failed");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "validator_failed",
                "the validator failed",
            );
        }
    };
    let Some(url) = svc.backend_for(&root.local) else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "no_backend",
            "no backend configured for this message",
        );
    };
    let payload = if violations.is_empty() {
        let ty = svc
            .schema
            .element(&msg.qname)
            .and_then(|t| svc.schema.resolve(t));
        let mut p = json!({
            "service": service,
            "message": root.local,
            "request": codec::element_to_json(&root, ty, &svc.schema),
            "sha256": digest,
            "schemaValid": true,
            "violations": [],
        });
        if ib.include_raw {
            p["rawXml"] = Value::String(String::from_utf8_lossy(body).into_owned());
        }
        p
    } else {
        // Paths and codes only. Never a value from the message.
        let summary: Vec<String> = violations
            .iter()
            .map(|v| format!("{}:{}", v.path, v.code))
            .collect();
        if let Some(suppressed) = svc.log_limit_inbound.check() {
            tracing::warn!(group, name, msg_type = %root.local, direction = "inbound", violations = ?summary, suppressed, "XSD validation (annotate): the message does not conform");
        }
        json!({
            "service": service,
            "message": root.local,
            "sha256": digest,
            "rawBase64": base64::engine::general_purpose::STANDARD.encode(body),
            "schemaValid": false,
            "violations": violations.iter().map(violation_json).collect::<Vec<_>>(),
        })
    };
    call_backend(
        st,
        ib,
        &url,
        &payload,
        (group, name, &root.local),
        headers,
        lane_headers(svc, &root.local),
    )
    .await
}

/// Sends input that is not a message of the lane to `<backend>/_invalid`.
#[allow(clippy::too_many_arguments)]
async fn send_invalid(
    st: &XmlLaneState,
    svc: &XmlService,
    ib: &super::XmlInbound,
    headers: &HeaderMap,
    (group, name): (&str, &str),
    service: &str,
    digest: &str,
    body: &[u8],
    code: &'static str,
) -> Response {
    use base64::Engine;
    // The sidecar check guarantees `inbound.backend` under `annotate`.
    let base = ib.backend.as_deref().unwrap_or_default();
    let url = format!("{}/{}", base.trim_end_matches('/'), super::INVALID_ROUTE);
    if let Some(suppressed) = svc.log_limit_inbound.check() {
        tracing::warn!(
            group,
            name,
            direction = "inbound",
            code,
            suppressed,
            "XSD validation (annotate): input is not a message of the lane"
        );
    }
    let payload = json!({
        "service": service,
        "sha256": digest,
        "rawBase64": base64::engine::general_purpose::STANDARD.encode(body),
        "schemaValid": false,
        "violations": [{"code": code}],
    });
    call_backend(
        st,
        ib,
        &url,
        &payload,
        (group, name, super::INVALID_ROUTE),
        headers,
        &svc.backend_headers,
    )
    .await
}

async fn handle_inbound(
    st: &XmlLaneState,
    group: &str,
    name: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let Some(svc) = st
        .registry
        .get(group, name)
        .filter(|s| s.sidecar.inbound.is_some())
    else {
        return err(
            StatusCode::NOT_FOUND,
            "not_found",
            "no inbound XML service at this path",
        );
    };
    let ib = svc.sidecar.inbound.as_ref().expect("filtered above");
    let limit = st.cfg.limits.max_request_bytes;
    if body.len() > limit {
        return err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            &format!("request exceeds {limit} bytes"),
        );
    }
    if svc.sidecar.validate.inbound == ValidateMode::Annotate {
        return handle_annotated(st, svc, group, name, headers, body).await;
    }
    let Ok(text) = std::str::from_utf8(body) else {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid_encoding",
            "request body is not valid UTF-8",
        );
    };
    let root = match dom::parse_message(text) {
        Ok(r) => r,
        Err(e) if e.1 == dom::DomErrorKind::Encoding => {
            return err(
                StatusCode::BAD_REQUEST,
                "unsupported_encoding",
                "XML declaration must say UTF-8 or leave the encoding out",
            )
        }
        Err(e) => return err(StatusCode::BAD_REQUEST, "malformed_xml", &e.0),
    };
    let Some(msg) = svc
        .messages
        .get(&root.local)
        .filter(|m| m.qname.ns == root.ns && m.cfg.direction.inbound())
    else {
        return err(
            StatusCode::BAD_REQUEST,
            "unsupported_message",
            &format!("root element {} is not served by this endpoint", root.local),
        );
    };
    match gate(svc.sidecar.validate.inbound, svc.model.as_deref(), &root) {
        Err(()) => {
            tracing::error!(group, name, msg_type = ?root.local, "XSD validator failed");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "validator_failed",
                "the validator failed",
            );
        }
        Ok(v) => {
            if let Some(resp) = judge(
                svc.sidecar.validate.inbound,
                group,
                name,
                &root.local,
                "inbound",
                &v,
                &svc.log_limit_inbound,
            ) {
                return resp;
            }
        }
    }
    if let Some(target) = msg.cfg.forward.as_deref() {
        // A forwarded message that is forwarded again is a loop. Refuse it
        // before the outbound call. A forwarded message that goes to a
        // backend is not a loop.
        if headers.contains_key(FORWARDED_HEADER) {
            tracing::warn!(group, name, msg_type = ?root.local, "forward loop detected");
            return err(
                StatusCode::LOOP_DETECTED,
                "loop_detected",
                "this message was already forwarded by XTR",
            );
        }
        return forward_raw(st, target, &root.local, headers, body, &ib.relay_statuses).await;
    }
    let Some(url) = svc.backend_for(&root.local) else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "no_backend",
            "no backend configured for this message",
        );
    };
    if st.offline {
        tracing::info!(msg_type = ?root.local, "inbound XML backend call blocked by XTR_OFFLINE");
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "offline",
            "XTR_OFFLINE: backend calls disabled",
        );
    }
    let ty = svc
        .schema
        .element(&msg.qname)
        .and_then(|t| svc.schema.resolve(t));
    let request = codec::element_to_json(&root, ty, &svc.schema);
    let digest = hex(&Sha256::digest(body));
    let mut payload = json!({
        "service": format!("{group}/{name}"),
        "message": root.local,
        "request": request,
        "sha256": digest,
    });
    if ib.include_raw {
        payload["rawXml"] = Value::String(text.to_string());
    }
    call_backend(
        st,
        ib,
        &url,
        &payload,
        (group, name, &root.local),
        headers,
        lane_headers(svc, &root.local),
    )
    .await
}

/// The extra headers for the backend of a message: only when the URL comes
/// from `inbound.backend`.
fn lane_headers<'a>(svc: &'a XmlService, message: &str) -> &'a [(header::HeaderName, HeaderValue)] {
    if svc.backend_is_lane(message) {
        &svc.backend_headers
    } else {
        &[]
    }
}

/// POSTs the payload to the backend and maps its answer to the answer for
/// the peer.
async fn call_backend(
    st: &XmlLaneState,
    ib: &super::XmlInbound,
    url: &str,
    payload: &Value,
    (group, name, msg_type): (&str, &str, &str),
    headers: &HeaderMap,
    extra: &[(header::HeaderName, HeaderValue)],
) -> Response {
    let mut req = st
        .backend
        .post(url)
        .header("x-xtr-inbound-service", format!("{group}/{name}"))
        .header("x-xtr-message", msg_type)
        .json(payload);
    if let Some(tp) = headers.get("traceparent") {
        req = req.header("traceparent", tp.clone());
    }
    for (n, v) in extra {
        req = req.header(n.clone(), v.clone());
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(msg_type = msg_type, url = ?url, error = ?e.to_string(), "inbound XML backend call failed");
            return if e.is_timeout() {
                err(
                    StatusCode::GATEWAY_TIMEOUT,
                    "backend_timeout",
                    "backend timeout",
                )
            } else {
                err(
                    StatusCode::BAD_GATEWAY,
                    "backend_unreachable",
                    "backend unreachable",
                )
            };
        }
    };
    let status = resp.status();
    if status.is_success() {
        // Drain (bounded) so the connection can be reused; the body is not used.
        let _ = crate::executor::plain::read_bounded(resp, st.cfg.limits.max_response_bytes).await;
        let code = StatusCode::from_u16(ib.success_status).unwrap_or(StatusCode::ACCEPTED);
        return code.into_response();
    }
    let (bytes, _, _) = read_capped(resp, st.cfg.limits.max_response_bytes).await;
    let text = lossy(&bytes);
    if ib.relay_statuses.contains(&status.as_u16()) {
        // The backend decides the peer-visible status (400/409/422 …).
        let mut r = (status, text).into_response();
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        return r;
    }
    // 5xx and any 4xx outside `relay_statuses` (wrong backend path,
    // gateway guard …): not a verdict on the message — the peer must retry.
    tracing::warn!(
        msg_type = msg_type,
        status = status.as_u16(),
        "inbound XML backend failed"
    );
    err(StatusCode::BAD_GATEWAY, "backend_error", "backend failed")
}

// ----------------------------------------------------------- outbound

async fn post_outbound(
    State(st): State<XmlLaneState>,
    Path((group, name, message)): Path<(String, String, String)>,
    body: Bytes,
) -> Response {
    match call_outbound(&st, &group, &name, &message, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response_with_options(st.cfg.expose_soap_fault_detail),
    }
}

async fn call_outbound(
    st: &XmlLaneState,
    group: &str,
    name: &str,
    message: &str,
    body: &[u8],
) -> Result<Response, XtrError> {
    let not_found = || XtrError::TemplateNotFound {
        group: group.to_string(),
        service: format!("{name}/{message}"),
    };
    let svc: &XmlService = st.registry.get(group, name).ok_or_else(not_found)?;
    let ob = svc.sidecar.outbound.as_ref().ok_or_else(not_found)?;
    let http = svc.http.as_ref().ok_or_else(not_found)?;
    let msg = svc
        .messages
        .get(message)
        .filter(|m| m.cfg.direction.outbound())
        .ok_or_else(not_found)?;
    let url = svc.url_for(message).ok_or_else(not_found)?;
    let limit = st.cfg.limits.max_request_bytes;
    if body.len() > limit {
        return Err(XtrError::RequestTooLarge { limit });
    }
    let value: Value = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(body) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => {
                return Err(XtrError::InvalidJsonBody {
                    reason: "expected a JSON object".into(),
                })
            }
            Err(e) => {
                return Err(XtrError::InvalidJsonBody {
                    reason: format!("parse error: {e}"),
                })
            }
        }
    };
    if st.offline {
        tracing::info!(msg_type = ?message, "outbound XML blocked by XTR_OFFLINE");
        return Err(XtrError::OfflineMode);
    }
    let element = codec::json_to_root_element(&msg.qname, &value, &svc.schema);
    let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{element}");
    // Validate the bytes that go to the peer, read back by the same parser
    // that reads inbound messages.
    let checked = if svc.sidecar.validate.outbound == ValidateMode::Off {
        Ok(Vec::new())
    } else {
        dom::parse_message(&xml)
            .map_err(|_| ())
            .and_then(|root| gate(svc.sidecar.validate.outbound, svc.model.as_deref(), &root))
    };
    match checked {
        Err(()) => {
            tracing::error!(
                group,
                name,
                msg_type = message,
                "XSD validator failed on an outbound request"
            );
            return Ok(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "validator_failed",
                "the validator failed",
            ));
        }
        Ok(v) => {
            if let Some(resp) = judge(
                svc.sidecar.validate.outbound,
                group,
                name,
                message,
                "outbound",
                &v,
                &svc.log_limit_outbound,
            ) {
                return Ok(resp);
            }
        }
    }
    let timeout = svc.timeout_for(
        message,
        Duration::from_secs(st.cfg.limits.request_timeout_secs),
    );
    let resp = match http
        .post(&url)
        .timeout(timeout)
        .header(header::CONTENT_TYPE, ob.content_type.as_str())
        .body(xml)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return Err(XtrError::UpstreamTimeout),
        Err(e) => {
            // Not "the peer said no": we do not know what happened.
            // The URL stays in the log, not in the response.
            tracing::warn!(msg_type = ?message, url = ?url, error = ?e.to_string(), "outbound XML peer unreachable");
            return Ok(err(
                StatusCode::BAD_GATEWAY,
                "upstream_unreachable",
                "peer unreachable",
            ));
        }
    };
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let (bytes, truncated, read_failed) = read_capped(resp, st.cfg.limits.max_response_bytes).await;
    let text = lossy(&bytes);
    // Decode the reply when it is one of our messages; anything else
    // (an empty 202, an HTML error page) is returned as plain `body`.
    let decoded = (!truncated && !read_failed)
        .then(|| dom::parse(&text).ok())
        .flatten()
        .and_then(|root| {
            let q = crate::inbound::contract::QName {
                ns: root.ns.clone(),
                local: root.local.clone(),
            };
            let element = svc.schema.element(&q)?;
            let ty = svc.schema.resolve(element);
            Some(json!({
                "message": root.local,
                "content": codec::element_to_json(&root, ty, &svc.schema),
            }))
        });
    Ok(Json(json!({
        "status": status,
        "contentType": content_type,
        "body": text,
        "bodyTruncated": truncated,
        "bodyReadFailed": read_failed,
        "response": decoded,
    }))
    .into_response())
}

/// Relay the received bytes untouched and hand the target's status
/// (and body) back to the peer. Nothing is decoded or stored here.
async fn forward_raw(
    st: &XmlLaneState,
    target: &str,
    message: &str,
    headers: &HeaderMap,
    body: &[u8],
    relay_statuses: &[u16],
) -> Response {
    if st.offline {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "offline",
            "XTR_OFFLINE: forwarding disabled",
        );
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("application/xml"));
    let mut req = st
        .backend
        .post(target)
        .header(header::CONTENT_TYPE, content_type)
        .header("x-xtr-message", message)
        .header(FORWARDED_HEADER, "1")
        .body(body.to_vec());
    if let Some(tp) = headers.get("traceparent") {
        req = req.header("traceparent", tp.clone());
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(msg_type = ?message, error = ?e.to_string(), "forward target call failed");
            return if e.is_timeout() {
                err(
                    StatusCode::GATEWAY_TIMEOUT,
                    "forward_timeout",
                    "forward target timeout",
                )
            } else {
                err(
                    StatusCode::BAD_GATEWAY,
                    "forward_unreachable",
                    "forward target unreachable",
                )
            };
        }
    };
    let status = resp.status();
    let ct = resp.headers().get(header::CONTENT_TYPE).cloned();
    let (bytes, truncated, failed) = read_capped(resp, st.cfg.limits.max_response_bytes).await;
    // Success is relayed whatever the body looks like (the target accepted
    // the message; a 502 would make the peer resend it). A 4xx is relayed
    // only when it is a verdict on the message (`relay_statuses`); 3xx, 5xx
    // and other 4xx are infrastructure faults: details stay in the log.
    if !(status.is_success() || relay_statuses.contains(&status.as_u16())) {
        tracing::warn!(msg_type = ?message, status = status.as_u16(), "forward target failed");
        return err(
            StatusCode::BAD_GATEWAY,
            "forward_error",
            "forward target failed",
        );
    }
    let text = if truncated || failed {
        tracing::warn!(msg_type = ?message, status = status.as_u16(), truncated, failed, "forward target body dropped");
        Vec::new()
    } else {
        bytes
    };
    let mut r = (status, text).into_response();
    if let Some(ct) = ct {
        r.headers_mut().insert(header::CONTENT_TYPE, ct);
    }
    r
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    #[test]
    fn validation_that_is_on_but_has_no_model_is_an_internal_error() {
        let root = dom::parse(r#"<a xmlns="urn:x"/>"#).unwrap();
        assert!(gate(ValidateMode::Enforce, None, &root).is_err());
        assert!(gate(ValidateMode::Report, None, &root).is_err());
        assert_eq!(gate(ValidateMode::Off, None, &root), Ok(Vec::new()));
    }
}
