//! Handlebars expansion with unified auto+user context.
//!
//! Fixes JVM bug #3 — the Spring version did:
//!
//! ```java
//! result.apply(localValues);   // return discarded
//! return result.apply(values);
//! ```
//!
//! …which meant auto-context substitutions never made it into the
//! final output. This module merges auto + user context and
//! renders **once**.
//!
//! Auto-context available in every DSL envelope:
//! * `{{generate.uuid}}` — random UUID per request (X-Road message id)
//! * `{{generate.instance}}` — X-Road instance (from config)
//! * `{{generate.client}}` — pre-built `<xroad:client>` element
//!   with member class / member code / subsystem code from config

use crate::config::AppConfig;
use crate::error::XtrError;
use ::handlebars::{
    Context, Handlebars, Helper, HelperResult, Output, RenderContext, RenderErrorReason,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

/// Filter `user_params` against the DSL's `allowed` list, then
/// render `template` with merged (auto + user) context. The DSL's
/// `params:` allow-list is what prevents template injection —
/// unlisted keys are silently dropped before rendering.
pub fn expand(
    template: &str,
    allowed: &[String],
    user_params: HashMap<String, Value>,
    cfg: &AppConfig,
) -> Result<String, XtrError> {
    let mut ctx = build_auto_context(cfg);

    // Merge user params (only those on the allow-list — matches
    // JVM XTR's filterParams semantics; drops the rest silently).
    let allowed_set: std::collections::HashSet<&String> = allowed.iter().collect();
    for (k, v) in user_params {
        if allowed_set.contains(&k) {
            ctx.insert(k, v);
        }
    }

    let mut hbs = Handlebars::new();
    // Audit-v1 M3: xml_attr helper produces attribute-safe escaping
    // (quotes and apostrophes as well as <, >, &). Default `{{x}}`
    // uses handlebars' HTML escaping which is safe in XML element
    // *text* but breaks attribute values when the operator writes
    // `<x attr="{{userInput}}">`. Give DSL authors an unambiguous
    // helper for that case so it doesn't have to be re-derived.
    hbs.register_helper("xml_attr", Box::new(xml_attr_helper));
    hbs.render_template(template, &Value::Object(ctx.into_iter().collect()))
        .map_err(|e| XtrError::HandlebarsError(e.to_string()))
}

fn xml_attr_helper(
    h: &Helper,
    _: &Handlebars,
    _: &Context,
    _: &mut RenderContext,
    out: &mut dyn Output,
) -> HelperResult {
    let value = h
        .param(0)
        .ok_or_else(|| RenderErrorReason::ParamNotFoundForIndex("xml_attr", 0))?;
    let s = value.value().as_str().unwrap_or("");
    out.write(&xml_attr_escape(s))?;
    Ok(())
}

/// Escape a string for inclusion between XML attribute quotes.
/// Order matters — `&` must be replaced first or later expansions
/// double-escape existing entities. Attribute-safe means every one
/// of `< > & " '` becomes an entity; that's a strict superset of
/// what element-text escaping covers.
fn xml_attr_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn build_auto_context(cfg: &AppConfig) -> HashMap<String, Value> {
    let mut ctx: HashMap<String, Value> = HashMap::new();
    // Note: keys use dot-notation as literal string keys — that's
    // how JVM XTR's Handlebars templates reference them
    // (`{{generate.uuid}}`). handlebars-rs supports dotted-string
    // property lookup when the JSON object literally has that key.
    ctx.insert(
        "generate.uuid".into(),
        Value::String(Uuid::new_v4().to_string()),
    );
    ctx.insert(
        "generate.instance".into(),
        Value::String(cfg.xroad_instance.clone()),
    );
    ctx.insert(
        "generate.client".into(),
        Value::String(build_client_element(cfg)),
    );
    // Task 005: expose the X-Road protocol version so DSL samples
    // don't have to hardcode "4.0". Available as both dotted-key
    // and nested-object forms.
    ctx.insert(
        "generate.protocol_version".into(),
        Value::String(cfg.xroad_protocol_version.clone()),
    );
    // Also expose the fields flat so DSLs can access them
    // individually without going through generate.client:
    ctx.insert(
        "generate".into(),
        json!({
            "uuid":              Uuid::new_v4().to_string(),
            "instance":          cfg.xroad_instance.clone(),
            "client":            build_client_element(cfg),
            "protocol_version":  cfg.xroad_protocol_version.clone(),
        }),
    );
    ctx
}

/// Render the `<xroad:client>` XML element with client identity
/// pulled from config. Fixes JVM bug #5 (the Spring version
/// left `%s` placeholders literal because of ordering).
fn build_client_element(cfg: &AppConfig) -> String {
    format!(
        r#"<xroad:client id:objectType="SUBSYSTEM"><id:xRoadInstance>{}</id:xRoadInstance><id:memberClass>{}</id:memberClass><id:memberCode>{}</id:memberCode><id:subsystemCode>{}</id:subsystemCode></xroad:client>"#,
        cfg.xroad_instance,
        cfg.client_data.member_class,
        cfg.client_data.member_code,
        cfg.client_data.subsystem_code,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ClientData;

    fn cfg() -> AppConfig {
        // Test fixture values, not real X-Road registration.
        // subsystem_code is a placeholder — see docs/DESIGN.md §2.7
        // and task 006 for real-org onboarding.
        AppConfig {
            xroad_instance: "ee-test".into(),
            client_data: ClientData {
                member_class: "GOV".into(),
                member_code: "70000000".into(),
                subsystem_code: "test-subsystem".into(),
            },
            ..Default::default()
        }
    }

    #[test]
    fn user_param_substituted_when_on_allowlist() {
        let mut params = HashMap::new();
        params.insert("reg_code".into(), Value::String("42".into()));
        let out = expand(
            "<x>{{reg_code}}</x>",
            &["reg_code".to_string()],
            params,
            &cfg(),
        )
        .unwrap();
        assert_eq!(out, "<x>42</x>");
    }

    #[test]
    fn user_param_dropped_when_not_on_allowlist() {
        let mut params = HashMap::new();
        params.insert("evil".into(), Value::String("payload".into()));
        // "evil" is NOT allow-listed → the template placeholder
        // resolves to empty (Handlebars default for missing keys).
        let out = expand("<x>{{evil}}</x>", &[], params, &cfg()).unwrap();
        assert_eq!(out, "<x></x>");
    }

    #[test]
    fn generate_instance_available() {
        let out = expand("<i>{{generate.instance}}</i>", &[], HashMap::new(), &cfg()).unwrap();
        assert_eq!(out, "<i>ee-test</i>");
    }

    #[test]
    fn generate_client_expands_to_xroad_element() {
        let out = expand("<h>{{{generate.client}}}</h>", &[], HashMap::new(), &cfg()).unwrap();
        assert!(out.contains("<xroad:client"));
        assert!(out.contains("<id:memberClass>GOV</id:memberClass>"));
        assert!(out.contains("<id:memberCode>70000000</id:memberCode>"));
        assert!(out.contains("<id:subsystemCode>test-subsystem</id:subsystemCode>"));
    }

    #[test]
    fn generate_uuid_is_present_and_valid() {
        let out = expand("<u>{{generate.uuid}}</u>", &[], HashMap::new(), &cfg()).unwrap();
        let inner = out
            .strip_prefix("<u>")
            .and_then(|s| s.strip_suffix("</u>"))
            .unwrap();
        Uuid::parse_str(inner).expect("generate.uuid should render a valid UUID");
    }

    #[test]
    fn user_param_value_containing_handlebars_is_not_re_rendered() {
        // Security regression: if a user param's *value* contains
        // `{{...}}`, that value must NOT be reinterpreted as a
        // Handlebars template on a second pass. Handlebars is
        // single-pass, but this test locks in that behavior so a
        // future "expand twice" mistake would be caught.
        let mut params = HashMap::new();
        params.insert(
            "note".into(),
            Value::String("hello {{generate.instance}}".into()),
        );
        let out = expand("<n>{{note}}</n>", &["note".to_string()], params, &cfg()).unwrap();
        // The literal `{{generate.instance}}` inside the param value
        // must appear (HTML-escaped by handlebars-rs default), NOT
        // resolve to "ee-test".
        assert!(
            !out.contains("ee-test"),
            "user param value must not be re-rendered as template — got {out}"
        );
        assert!(
            out.contains("generate.instance"),
            "literal placeholder should survive as escaped text — got {out}"
        );
    }

    #[test]
    fn generate_protocol_version_from_config() {
        let mut c = cfg();
        c.xroad_protocol_version = "4.0".into();
        let out = expand(
            "<v>{{generate.protocol_version}}</v>",
            &[],
            HashMap::new(),
            &c,
        )
        .unwrap();
        assert_eq!(out, "<v>4.0</v>");
    }

    #[test]
    fn generate_protocol_version_honors_override() {
        let mut c = cfg();
        c.xroad_protocol_version = "4.1".into();
        let out = expand(
            "<v>{{generate.protocol_version}}</v>",
            &[],
            HashMap::new(),
            &c,
        )
        .unwrap();
        assert_eq!(out, "<v>4.1</v>");
    }

    #[test]
    fn audit_m3_xml_attr_helper_escapes_quotes() {
        let mut params = HashMap::new();
        params.insert(
            "id".into(),
            Value::String(r#"a"b'c<d>&e"#.into()),
        );
        let out = expand(
            r#"<x attr="{{xml_attr id}}"/>"#,
            &["id".to_string()],
            params,
            &cfg(),
        )
        .unwrap();
        assert_eq!(
            out,
            r#"<x attr="a&quot;b&apos;c&lt;d&gt;&amp;e"/>"#
        );
    }

    #[test]
    fn audit_m3_xml_attr_escape_ordering_is_stable() {
        assert_eq!(super::xml_attr_escape("a&<b"), "a&amp;&lt;b");
        assert_eq!(super::xml_attr_escape("plain"), "plain");
        assert_eq!(super::xml_attr_escape(""), "");
    }

    #[test]
    fn single_pass_apply_no_two_step_regression() {
        // Guards against the JVM bug #3 regression: if we ever
        // re-introduce a two-pass render, the auto-context values
        // would end up in the final output but with a different
        // order or timing. This test asserts a single coherent
        // render.
        let mut params = HashMap::new();
        params.insert("id".into(), Value::String("A".into()));
        let out = expand(
            "<x>{{id}}|{{generate.instance}}</x>",
            &["id".to_string()],
            params,
            &cfg(),
        )
        .unwrap();
        assert_eq!(out, "<x>A|ee-test</x>");
    }
}
