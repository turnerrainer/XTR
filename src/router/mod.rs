//! Axum router — the HTTP surface of XTR-on-Rust.
//!
//! Routes (per DESIGN.md §8.2):
//! * `POST /:group/:service` — invoke a mapped X-Road service.
//!   Request body: JSON object of params. Response body:
//!   `{"body": …, "headers": …}` (SOAP body + header translated).
//! * `GET /health` — `{"status":"ok"}`
//! * `GET /api` — auto-generated OpenAPI 3.1 (Phase G)

use crate::config::AppConfig;
use crate::dsl::handlebars::expand;
use crate::dsl::loader::ServiceMap;
use crate::error::XtrError;
use crate::executor::Executor;
use crate::translate::xml_to_json;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
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
        // Task 011: cap inbound REST body size at the extractor.
        // Overflow surfaces via `handle_body_rejection` as a
        // structured `RequestTooLarge` error (413 + JSON).
        // We give Axum's DefaultBodyLimit a slightly-larger ceiling
        // so its "connection kill" path only fires on truly
        // pathological uploads; the precise cap check lives in the
        // handler below.
        .route(
            "/:group/:service",
            post(invoke).layer(DefaultBodyLimit::max(limit.saturating_add(4096))),
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
    body: Bytes,
) -> Result<Json<Value>, XtrError> {
    // Task 011: enforce the byte-exact request cap here — the
    // DefaultBodyLimit layer is a coarse backstop; this is the
    // authoritative check that produces the structured 413.
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

    // Empty body → empty params (matches JVM XTR: an empty POST is
    // a valid request against a zero-param service). Non-empty
    // body: must parse as a JSON object; anything else is treated
    // as no params (task 012 tightens this contract).
    let user_params = if body.is_empty() {
        std::collections::HashMap::new()
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Object(map)) => map.into_iter().collect(),
            _ => std::collections::HashMap::new(),
        }
    };

    let envelope = expand(
        &template.envelope,
        &template.params,
        user_params,
        &state.cfg,
    )?;

    let xml_response = state.executor.dispatch(&template, envelope).await?;
    let translated = xml_to_json::translate_soap(&xml_response)?;
    Ok(Json(translated))
}
