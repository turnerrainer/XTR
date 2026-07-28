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

    let error_ref = json!({ "$ref": "#/components/schemas/XtrError" });

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

        let err_response = |desc: &str| {
            json!({
                "description": desc,
                "content": {
                    "application/json": { "schema": error_ref }
                }
            })
        };

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
                "404": err_response("Template not found (unmapped group/service)"),
                "413": err_response("Request body exceeds configured max_request_bytes"),
                "502": err_response("Upstream error: upstream_http_error / upstream_soap_fault / upstream_xml_parse_error / upstream_body_too_large"),
                "504": err_response("Upstream request timed out"),
                "500": err_response("Internal error: template expansion failed, keystore load failed, etc."),
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
        "components": {
            "schemas": {
                // Shape aligned with XtrError::into_response
                // (src/error.rs). `error` + `message` are always
                // present; `code`/`string`/`detail`/`limit` are
                // populated for specific variants.
                "XtrError": {
                    "type": "object",
                    "required": ["error", "message"],
                    "properties": {
                        "error":   { "type": "string",
                                     "description": "Stable machine-readable error code." },
                        "message": { "type": "string" },
                        "code":    { "type": "string",
                                     "description": "SOAP Fault code (only for upstream_soap_fault)." },
                        "string":  { "type": "string",
                                     "description": "SOAP Fault message (only for upstream_soap_fault)." },
                        "detail":  { "description": "SOAP Fault detail body (only for upstream_soap_fault)." },
                        "limit":   { "type": "integer",
                                     "description": "Byte cap that was exceeded (request_too_large / upstream_body_too_large)." },
                    }
                }
            }
        }
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
    fn error_responses_include_413_and_reference_shared_schema() {
        let mut m = ServiceMap::new();
        m.insert(("g".into(), "s".into()), tpl(&[]));
        let spec = build_spec(&m, "0.1.0");
        let responses = &spec["paths"]["/g/s"]["post"]["responses"];
        // Task 011 — 413 must be advertised now that the router
        // enforces max_request_bytes.
        assert!(
            responses.get("413").is_some(),
            "413 response should be documented"
        );
        // All error responses reference the shared XtrError
        // schema — consumers can codegen one error type.
        for code in ["404", "413", "502", "504", "500"] {
            let schema = &responses[code]["content"]["application/json"]["schema"];
            assert_eq!(
                schema["$ref"], "#/components/schemas/XtrError",
                "expected XtrError $ref on {code}"
            );
        }
        // The shared schema itself exists and declares the fields
        // that at least one variant of XtrError populates.
        let xtr_err = &spec["components"]["schemas"]["XtrError"];
        assert_eq!(xtr_err["type"], "object");
        for field in ["error", "message", "code", "string", "detail", "limit"] {
            assert!(
                xtr_err["properties"].get(field).is_some(),
                "XtrError.properties.{field} missing"
            );
        }
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
