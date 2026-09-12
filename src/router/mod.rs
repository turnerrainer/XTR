//! Axum router — the HTTP surface of XTR-on-Rust.
//!
//! Routes:
//! * `POST /:group/:service` — SOAP invocation (unchanged shape:
//!   JSON in, `{"body": …, "headers": …}` out).
//! * `<any-method> /:group/:service` — REST passthrough for issue #5
//!   DSLs (`kind: rest`). Body + query forwarded verbatim; upstream
//!   response returned as-is with all non-hop-by-hop headers +
//!   content-type + status preserved.
//! * `GET /health` — liveness probe.
//! * `GET /api` — auto-generated OpenAPI 3.1 spec.

use crate::config::AppConfig;
use crate::dsl::handlebars::expand;
use crate::dsl::loader::ServiceMap;
use crate::dsl::{SoapTemplate, TemplateKind};
use crate::error::XtrError;
use crate::executor::Executor;
use crate::translate::xml_to_json;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<AppConfig>,
    pub services: Arc<ServiceMap>,
    pub executor: Executor,
    pub openapi_spec: Arc<Value>,
}

pub fn build(state: AppState) -> Router {
    let limit = state.cfg.limits.max_request_bytes;
    Router::new()
        .route("/health", get(health))
        .route("/api", get(openapi))
        // A single `any` handler covers both SOAP (POST only) and
        // REST (DSL-declared method). Method-appropriateness is
        // decided per-template inside `invoke`. DefaultBodyLimit
        // provides the coarse ceiling; the precise 413 lives in
        // the handler.
        .route(
            "/:group/:service",
            any(invoke).layer(DefaultBodyLimit::max(limit.saturating_add(4096))),
        )
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

async fn openapi(State(state): State<AppState>) -> impl IntoResponse {
    Json((*state.openapi_spec).clone())
}

async fn invoke(
    State(state): State<AppState>,
    Path((group, service)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    Query(query_pairs): Query<Vec<(String, String)>>,
    body: Bytes,
) -> Response {
    match invoke_inner(&state, group, service, method, headers, query_pairs, body).await {
        Ok(resp) => resp,
        // Audit-v1 H3: route errors through the config-aware
        // renderer so `expose_soap_fault_detail` can opt back in
        // to sharing SOAP fault detail with REST callers.
        Err(e) => e.into_response_with_options(state.cfg.expose_soap_fault_detail),
    }
}

async fn invoke_inner(
    state: &AppState,
    group: String,
    service: String,
    method: Method,
    headers: HeaderMap,
    query_pairs: Vec<(String, String)>,
    body: Bytes,
) -> Result<Response, XtrError> {
    // Enforce the byte-exact request cap here — the DefaultBodyLimit
    // layer is a coarse backstop; this is the authoritative check
    // that produces the structured 413.
    let limit = state.cfg.limits.max_request_bytes;
    if body.len() > limit {
        return Err(XtrError::RequestTooLarge { limit });
    }

    let template = state
        .services
        .get(&(group.clone(), service.clone()))
        .cloned()
        .ok_or(XtrError::TemplateNotFound {
            group: group.clone(),
            service: service.clone(),
        })?;

    // Enforce DSL method contract for BOTH SOAP and REST — the DSL
    // file names the allowed method. SOAP DSLs default `method:
    // POST` and mismatches were previously routed via axum's own
    // 405; the new `any` route means we enforce here.
    if !method.as_str().eq_ignore_ascii_case(&template.method) {
        return Err(XtrError::MethodNotAllowed {
            method: method.to_string(),
            group,
            service,
        });
    }

    match &template.kind {
        TemplateKind::Soap(soap) => {
            let translated = invoke_soap(state, soap, &template.method, body).await?;
            Ok(Json(translated).into_response())
        }
        TemplateKind::Rest(rest) => {
            let upstream = state
                .executor
                .dispatch_rest(rest, &method, query_pairs, &headers, body.to_vec())
                .await?;
            build_rest_response(upstream)
        }
    }
}

async fn invoke_soap(
    state: &AppState,
    soap: &SoapTemplate,
    method: &str,
    body: Bytes,
) -> Result<Value, XtrError> {
    // Empty body → empty params (matches JVM XTR: an empty POST is
    // a valid request against a zero-param service). Non-empty
    // body: must parse as a JSON object.
    //
    // Audit v1 FN3: previously, malformed JSON and non-object shapes
    // silently degraded to empty params, so an attacker could send
    // garbage on the XTR wire and still force a real upstream call
    // — with XTR's mTLS identity — against a real X-Road service.
    // The upstream 500 then attributed to XTR. Post-fix, malformed
    // JSON returns 400 before any outbound call is issued; an
    // explicit empty object `{}` still means "zero params" and
    // proceeds normally.
    let user_params = if body.is_empty() {
        std::collections::HashMap::new()
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Object(map)) => map.into_iter().collect(),
            Ok(other) => {
                return Err(XtrError::InvalidJsonBody {
                    reason: format!(
                        "expected a JSON object, got {}",
                        json_kind_name(&other)
                    ),
                });
            }
            Err(e) => {
                return Err(XtrError::InvalidJsonBody {
                    reason: format!("parse error: {e}"),
                });
            }
        }
    };

    let envelope = expand(&soap.envelope, &soap.params, user_params, &state.cfg)?;
    let xml_response = state.executor.dispatch_soap(soap, method, envelope).await?;
    xml_to_json::translate_soap(&xml_response)
}

fn json_kind_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Turn the upstream response into an axum `Response`, forwarding
/// status + all non-hop-by-hop headers (the deny-list was already
/// applied by the rest_lane executor). Body bytes pass through
/// unchanged.
fn build_rest_response(
    upstream: crate::executor::rest_lane::RestUpstreamResponse,
) -> Result<Response, XtrError> {
    let status = StatusCode::from_u16(upstream.status)
        .map_err(|e| XtrError::Internal(format!("invalid upstream status: {e}")))?;
    let mut builder = Response::builder().status(status);
    for (name, value) in upstream.headers.iter() {
        // Header names arrive already-lowercased from HeaderMap
        // iteration, but re-parse via HeaderName to keep axum's
        // canonical form.
        if let Ok(hn) = HeaderName::from_bytes(name.as_str().as_bytes()) {
            builder = builder.header(hn, value.clone());
        }
    }
    builder
        .body(axum::body::Body::from(upstream.body))
        .map_err(|e| XtrError::Internal(format!("building response: {e}")))
}
