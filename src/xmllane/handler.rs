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

use super::{XmlRegistry, XmlService};
use crate::config::AppConfig;
use crate::error::XtrError;
use crate::inbound::{codec, dom};
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
    let mut req = st
        .backend
        .post(&url)
        .header("x-xtr-inbound-service", format!("{group}/{name}"))
        .header("x-xtr-message", root.local.as_str())
        .json(&payload);
    if let Some(tp) = headers.get("traceparent") {
        req = req.header("traceparent", tp.clone());
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(msg_type = ?root.local, url = ?url, error = ?e.to_string(), "inbound XML backend call failed");
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
    tracing::warn!(msg_type = ?root.local, status = status.as_u16(), "inbound XML backend failed");
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
