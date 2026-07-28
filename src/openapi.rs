//! OpenAPI 3.1 spec auto-generated from the loaded DSL tree.
//! Built once at boot; served from cache at `GET /api`.
//! Full implementation in Phase G — Phase F ships a placeholder
//! so the router can wire it into AppState.

use crate::dsl::loader::ServiceMap;
use serde_json::{json, Value};

pub fn build_spec(_services: &ServiceMap, version: &str) -> Value {
    // Placeholder — Phase G fills this in with real per-DSL
    // operations. For now, a bare OpenAPI 3.1 skeleton so
    // `GET /api` returns something structurally valid.
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "XTR-on-Rust",
            "version": version,
        },
        "paths": {},
    })
}
