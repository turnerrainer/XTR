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
use axum::extract::{Path, State};
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
    Router::new()
        .route("/health", get(health))
        .route("/api", get(openapi))
        .route("/:group/:service", post(invoke))
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
    body: Option<Json<Value>>,
) -> Result<Json<Value>, XtrError> {
    let template = state
        .services
        .get(&(group.clone(), service.clone()))
        .cloned()
        .ok_or(XtrError::TemplateNotFound {
            group: group.clone(),
            service: service.clone(),
        })?;

    // Extract params from the request body. Missing body = empty
    // object. For MVP: object body = params; anything else = empty.
    // v0.3 (task filed under epic-response-mapping when needed)
    // will allow richer JSON types.
    let user_params = match body {
        Some(Json(Value::Object(map))) => map.into_iter().collect(),
        _ => std::collections::HashMap::new(),
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
