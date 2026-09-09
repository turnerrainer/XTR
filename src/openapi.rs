//! OpenAPI 3.1 spec auto-generated from the loaded DSL tree.
//!
//! Built once at boot; served from cache at `GET /api`.
//! Fixes JVM bug #14 — Spring version emitted property type as
//! `"String"` (Java class name) instead of `"string"` (JSON type).

use crate::dsl::loader::ServiceMap;
use crate::dsl::TemplateKind;
use serde_json::{json, Map, Value};

pub fn build_spec(services: &ServiceMap, version: &str) -> Value {
    let mut paths = Map::new();

    // Sort for stable output — same input tree always produces
    // the same spec bytes. Useful when consumers diff.
    let mut keys: Vec<&(String, String)> = services.keys().collect();
    keys.sort();

    let error_ref = json!({ "$ref": "#/components/schemas/XtrError" });
    let err_response = |desc: &str| {
        json!({
            "description": desc,
            "content": {
                "application/json": { "schema": error_ref }
            }
        })
    };

    for (group, service) in keys {
        let template = &services[&(group.clone(), service.clone())];
        let path = format!("/{group}/{service}");

        match &template.kind {
            TemplateKind::Soap(soap) => {
                let mut request_body_props = Map::new();
                for p in &soap.params {
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
                        "required": !soap.params.is_empty(),
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
            TemplateKind::Rest(rest) => {
                // REST-lane operations advertise the DSL's declared
                // method + upstream service identity but leave
                // request/response schema opaque — the body is
                // forwarded verbatim.
                let method = template.method.to_lowercase();
                let mut query_params: Vec<Value> = Vec::new();
                // Enumerate query params only when the DSL narrows
                // the set. None (spec default: forward all) means
                // "any query key permitted" — impossible to
                // enumerate. Empty Some(vec![]) means "no query
                // keys permitted" — also nothing to enumerate.
                if let Some(list) = &rest.allowed_query_params {
                    for q in list {
                        query_params.push(json!({
                            "name": q,
                            "in": "query",
                            "required": false,
                            "schema": {"type": "string"},
                        }));
                    }
                }
                let path_seg = if rest.target.path.is_empty() {
                    String::new()
                } else if rest.target.path.starts_with('/') {
                    rest.target.path.clone()
                } else {
                    format!("/{}", rest.target.path)
                };
                let target_summary = format!(
                    "{}/{}/{}/{}{}",
                    rest.target.member_class,
                    rest.target.member_code,
                    rest.target.subsystem_code,
                    rest.target.service_code,
                    path_seg,
                );
                let mut operation = json!({
                    "operationId": format!("{}_{group}_{service}", method),
                    "tags": [group],
                    "summary": format!("{group}/{service} (REST → {target_summary})"),
                    "parameters": query_params,
                    "responses": {
                        "200": {"description": "Upstream response (passthrough)"},
                        "404": err_response("Template not found (unmapped group/service)"),
                        "405": err_response("Method not allowed for this template"),
                        "413": err_response("Request body exceeds configured max_request_bytes"),
                        "502": err_response("Upstream error"),
                        "504": err_response("Upstream request timed out"),
                        "500": err_response("Internal error"),
                    }
                });
                if rest.forward_body {
                    operation["requestBody"] = json!({
                        "required": false,
                        "content": {
                            "application/octet-stream": {
                                "schema": {"type": "string", "format": "binary"},
                            }
                        }
                    });
                }
                paths.insert(path, json!({ method: operation }));
            }
        }
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
    use crate::dsl::{RestTarget, RestTemplate, SoapTemplate, TemplateKind, XRoadTemplate};
    use std::sync::Arc;

    fn tpl(params: &[&str]) -> Arc<XRoadTemplate> {
        Arc::new(XRoadTemplate {
            method: "POST".into(),
            kind: TemplateKind::Soap(SoapTemplate {
                params: params.iter().map(|s| s.to_string()).collect(),
                service: Some("https://x".into()),
                envelope: "<x/>".into(),
            }),
        })
    }

    fn rest_tpl(method: &str, allowed: Option<&[&str]>) -> Arc<XRoadTemplate> {
        Arc::new(XRoadTemplate {
            method: method.into(),
            kind: TemplateKind::Rest(RestTemplate {
                target: RestTarget {
                    member_class: "GOV".into(),
                    member_code: "70008440".into(),
                    subsystem_code: "rr".into(),
                    service_code: "dde".into(),
                    path: "/v1/isikud".into(),
                },
                allowed_query_params: allowed
                    .map(|a| a.iter().map(|s| s.to_string()).collect()),
                forward_body: true,
            }),
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
    fn rest_kind_dsl_advertised_under_configured_method() {
        // Issue #5: a `kind: rest` DSL declaring `method: GET` must
        // appear under `paths."/g/s".get`, not `.post`.
        let mut m = ServiceMap::new();
        m.insert(
            ("rr".into(), "isikud".into()),
            rest_tpl("GET", Some(&["personalCode"])),
        );
        let spec = build_spec(&m, "0.3.0-rc-test");
        let op = &spec["paths"]["/rr/isikud"]["get"];
        assert_eq!(op["operationId"], "get_rr_isikud");
        // Allow-listed query param is enumerated.
        let params = op["parameters"].as_array().expect("parameters array");
        assert!(
            params.iter().any(|p| p["name"] == "personalCode" && p["in"] == "query"),
            "expected personalCode as query parameter, got: {params:?}"
        );
        // No SOAP-style JSON requestBody schema (body is opaque).
        let content = &op["requestBody"]["content"];
        assert!(content.get("application/json").is_none());
        assert!(content.get("application/octet-stream").is_some());
    }

    #[test]
    fn rest_kind_dsl_with_no_query_filter_advertises_no_parameters() {
        // When allowed_query_params is None (spec §4.5 default:
        // forward all), the OpenAPI spec advertises no query
        // params — impossible to enumerate the universe.
        let mut m = ServiceMap::new();
        m.insert(("rr".into(), "isikud".into()), rest_tpl("GET", None));
        let spec = build_spec(&m, "0.3.0-rc-test");
        let params = spec["paths"]["/rr/isikud"]["get"]["parameters"]
            .as_array()
            .expect("parameters array");
        assert!(params.is_empty());
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
