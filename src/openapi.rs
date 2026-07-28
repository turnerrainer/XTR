//! OpenAPI 3.1 spec auto-generated from the loaded DSL tree.
//!
//! Built once at boot; served from cache at `GET /api`.
//! Fixes JVM bug #14 — Spring version emitted property type as
//! `"String"` (Java class name) instead of `"string"` (JSON type).

use crate::dsl::loader::ServiceMap;
use serde_json::{json, Map, Value};

pub fn build_spec(services: &ServiceMap, version: &str) -> Value {
    let mut paths = Map::new();

    // Sort for stable output — same input tree always produces
    // the same spec bytes. Useful when consumers diff.
    let mut keys: Vec<&(String, String)> = services.keys().collect();
    keys.sort();

    for (group, service) in keys {
        let template = &services[&(group.clone(), service.clone())];
        let path = format!("/{group}/{service}");

        let mut request_body_props = Map::new();
        for p in &template.params {
            request_body_props.insert(
                p.clone(),
                json!({
                    // Fixes JVM bug #14: correct JSON schema type.
                    "type": "string",
                }),
            );
        }

        let operation = json!({
            "operationId": format!("post_{group}_{service}"),
            "tags": [group],
            "summary": format!("{group}/{service}"),
            "requestBody": {
                "required": !template.params.is_empty(),
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "properties": request_body_props,
                        }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Success",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "body":    { "type": "object" },
                                    "headers": { "type": "object" },
                                }
                            }
                        }
                    }
                },
                "404": {"description": "Template not found"},
                "502": {"description": "Upstream error"},
                "504": {"description": "Upstream timeout"},
            }
        });

        paths.insert(path, json!({ "post": operation }));
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "XTR-on-Rust",
            "version": version,
            "description": "REST proxy for X-Road SOAP services.",
        },
        "paths": paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::XRoadTemplate;
    use std::sync::Arc;

    fn tpl(params: &[&str]) -> Arc<XRoadTemplate> {
        Arc::new(XRoadTemplate {
            params: params.iter().map(|s| s.to_string()).collect(),
            service: Some("https://x".into()),
            method: "POST".into(),
            envelope: "<x/>".into(),
        })
    }

    #[test]
    fn empty_service_map_yields_empty_paths() {
        let spec = build_spec(&ServiceMap::new(), "0.1.0");
        assert_eq!(spec["openapi"], "3.1.0");
        assert_eq!(spec["paths"], json!({}));
    }

    #[test]
    fn one_service_yields_one_post_path() {
        let mut m = ServiceMap::new();
        m.insert(("ar".into(), "lihtandmed_v3".into()), tpl(&["reg_code"]));
        let spec = build_spec(&m, "0.1.0");
        let op = &spec["paths"]["/ar/lihtandmed_v3"]["post"];
        assert_eq!(op["operationId"], "post_ar_lihtandmed_v3");
        assert_eq!(op["tags"], json!(["ar"]));
    }

    #[test]
    fn param_schema_type_is_lowercase_string() {
        // Guards against re-introducing JVM bug #14.
        let mut m = ServiceMap::new();
        m.insert(("ar".into(), "svc".into()), tpl(&["reg_code"]));
        let spec = build_spec(&m, "0.1.0");
        let prop_type = &spec["paths"]["/ar/svc"]["post"]["requestBody"]["content"]
            ["application/json"]["schema"]["properties"]["reg_code"]["type"];
        assert_eq!(prop_type, "string");
    }

    #[test]
    fn request_body_required_only_when_params_present() {
        let mut m = ServiceMap::new();
        m.insert(("g".into(), "with_params".into()), tpl(&["x"]));
        m.insert(("g".into(), "no_params".into()), tpl(&[]));
        let spec = build_spec(&m, "0.1.0");
        assert_eq!(
            spec["paths"]["/g/with_params"]["post"]["requestBody"]["required"],
            true
        );
        assert_eq!(
            spec["paths"]["/g/no_params"]["post"]["requestBody"]["required"],
            false
        );
    }

    #[test]
    fn response_schema_documents_body_and_headers() {
        // Ensures the {body, headers} response shape is
        // discoverable via the spec.
        let mut m = ServiceMap::new();
        m.insert(("g".into(), "s".into()), tpl(&[]));
        let spec = build_spec(&m, "0.1.0");
        let props = &spec["paths"]["/g/s"]["post"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["properties"];
        assert!(props.get("body").is_some());
        assert!(props.get("headers").is_some());
    }
}
