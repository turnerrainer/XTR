//! `XRoadTemplate` — the parsed shape of one DSL file.
//!
//! Two kinds share a route surface:
//! * `TemplateKind::Soap` — legacy DSL (envelope + Handlebars).
//!   Absent `kind:` deserialises here to preserve 0.2.0 DSLs.
//! * `TemplateKind::Rest` — issue #5 REST passthrough. Forwards
//!   request bytes verbatim to the X-Road Security Server with
//!   the correct `/r1/…` URL and `X-Road-Client` header derived
//!   from the DSL's `target:` block per
//!   [X-Road Message Protocol for REST v1.0.4](https://github.com/nordic-institute/X-Road/blob/develop/doc/Protocols/pr-rest_x-road_message_protocol_for_rest.md).

use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone)]
pub struct XRoadTemplate {
    /// HTTP method the DSL declares as the contract for this
    /// endpoint. Enforced at the router: inbound method mismatch
    /// yields `405`. For REST DSLs that want to accept multiple
    /// methods, ship multiple DSL files.
    pub method: String,
    pub kind: TemplateKind,
}

#[derive(Debug, Clone)]
pub enum TemplateKind {
    Soap(SoapTemplate),
    Rest(RestTemplate),
}

#[derive(Debug, Clone)]
pub struct SoapTemplate {
    pub params: Vec<String>,
    /// Optional direct upstream URL. `None` → route via Security Server.
    pub service: Option<String>,
    pub envelope: String,
}

#[derive(Debug, Clone)]
pub struct RestTemplate {
    pub target: RestTarget,
    /// Optional whitelist of query-string keys forwarded upstream.
    ///
    /// * `None` (field omitted): forward every query key — matches
    ///   X-Road REST §4.5 "query parameters MUST be passed
    ///   unmodified".
    /// * `Some(vec![])`: drop all query keys. Paranoid opt-in for
    ///   operators who want the SOAP-lane's silent-drop posture.
    /// * `Some(vec!["k"])`: allow-list only these keys.
    pub allowed_query_params: Option<Vec<String>>,
    /// Forward the inbound request body verbatim upstream. Default
    /// true; set false for methods that MUST NOT carry a body
    /// (some servers reject bodies on GET).
    pub forward_body: bool,
}

/// Target X-Road REST service identity. Combined with the Security
/// Server base URL + `xroad_instance` to build the outbound URL
/// per X-Road REST §4.1:
///
/// ```text
/// /r1/{instance}/{class}/{code}/{subsystem}/{service_code}[/path?query]
/// ```
///
/// Note: X-Road versioning ("v1") is part of `[path]`, **not** a
/// separate segment in `{serviceId}`. If your service publishes at
/// `service_code=petstore` with `/v2/pets` under it, set
/// `service_code: petstore` and `path: /v2/pets`.
#[derive(Debug, Clone, Deserialize)]
pub struct RestTarget {
    pub member_class: String,
    pub member_code: String,
    pub subsystem_code: String,
    pub service_code: String,
    /// Path appended after `{service_code}`. Leading slash optional.
    /// May contain further path segments (`/v1/isikud`, `/v2/pets/{id}`)
    /// — everything up to `?` is passed to the provider by the
    /// Security Server after stripping the `/r1/{serviceId}/` prefix.
    #[serde(default)]
    pub path: String,
}

impl<'de> Deserialize<'de> for XRoadTemplate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawTemplate::deserialize(deserializer)?;
        let kind = match raw.kind.as_str() {
            "soap" => TemplateKind::Soap(SoapTemplate {
                params: raw.params,
                service: raw.service,
                envelope: raw.envelope,
            }),
            "rest" => TemplateKind::Rest(RestTemplate {
                target: raw.target.ok_or_else(|| {
                    serde::de::Error::custom("rest template requires `target:` block")
                })?,
                allowed_query_params: raw.allowed_query_params,
                forward_body: raw.forward_body,
            }),
            other => {
                return Err(serde::de::Error::custom(format!(
                    "unknown kind '{other}' (expected 'soap' or 'rest')"
                )))
            }
        };
        Ok(XRoadTemplate {
            method: raw.method,
            kind,
        })
    }
}

#[derive(Deserialize)]
struct RawTemplate {
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default = "default_method")]
    method: String,

    // SOAP-only fields
    #[serde(default)]
    params: Vec<String>,
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    envelope: String,

    // REST-only fields
    #[serde(default)]
    target: Option<RestTarget>,
    #[serde(default)]
    allowed_query_params: Option<Vec<String>>,
    #[serde(default = "default_forward_body")]
    forward_body: bool,
}

fn default_kind() -> String {
    "soap".into()
}
fn default_method() -> String {
    "POST".into()
}
fn default_forward_body() -> bool {
    true
}

impl RestTarget {
    /// Validate identifier segments against X-Road REST §4.8
    /// character restrictions. Non-conforming identifiers are
    /// accepted by parsing (so operators can experiment) but the
    /// doctor flags them as `weak-rest-identifier-charset`.
    pub fn identifier_charset_ok(&self) -> bool {
        [
            &self.member_class,
            &self.member_code,
            &self.subsystem_code,
            &self.service_code,
        ]
        .iter()
        .all(|s| identifier_chars_ok(s))
    }

    pub fn required_fields_present(&self) -> bool {
        !self.member_class.is_empty()
            && !self.member_code.is_empty()
            && !self.subsystem_code.is_empty()
            && !self.service_code.is_empty()
    }
}

/// X-Road REST §4.8 identifier character restriction:
/// `A-Za-z0-9'()+,-.=?` are the only allowed characters. Empty
/// strings return `false` — an empty identifier is a separate
/// error class.
fn identifier_chars_ok(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    s.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '\'' | '(' | ')' | '+' | ',' | '-' | '.' | '=' | '?')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(y: &str) -> XRoadTemplate {
        serde_yaml_ng::from_str(y).expect("valid DSL")
    }

    #[test]
    fn soap_dsl_without_kind_deserialises_as_soap() {
        // 0.2.0 backward-compat: DSL files pre-dating issue #5 must
        // continue to parse unchanged.
        let t = parse("params: [reg_code]\nmethod: POST\nenvelope: <x>{{reg_code}}</x>\n");
        assert_eq!(t.method, "POST");
        match t.kind {
            TemplateKind::Soap(s) => {
                assert_eq!(s.params, vec!["reg_code"]);
                assert!(s.service.is_none());
                assert_eq!(s.envelope, "<x>{{reg_code}}</x>");
            }
            _ => panic!("expected SOAP kind"),
        }
    }

    #[test]
    fn rest_dsl_deserialises_and_versioning_lives_in_path() {
        // Per X-Road REST §4.1 the `/v1` segment is inside [path];
        // there is no separate serviceVersion in the URL. Operators
        // encode versioning as part of the path field.
        let t = parse(
            "kind: rest\nmethod: GET\ntarget:\n  member_class: GOV\n  member_code: '70008440'\n  subsystem_code: rr\n  service_code: dde\n  path: /v1/isikud\nallowed_query_params: [personalCode]\n",
        );
        assert_eq!(t.method, "GET");
        match t.kind {
            TemplateKind::Rest(r) => {
                assert_eq!(r.target.service_code, "dde");
                assert_eq!(r.target.path, "/v1/isikud");
                assert_eq!(
                    r.allowed_query_params.as_deref(),
                    Some(&["personalCode".to_string()][..])
                );
                assert!(r.forward_body); // default
            }
            _ => panic!("expected REST kind"),
        }
    }

    #[test]
    fn rest_dsl_requires_target_block() {
        let err = serde_yaml_ng::from_str::<XRoadTemplate>("kind: rest\nmethod: POST\n").unwrap_err();
        assert!(
            err.to_string().contains("target"),
            "expected error about missing target, got: {err}"
        );
    }

    #[test]
    fn unknown_kind_rejected_at_parse_time() {
        let err = serde_yaml_ng::from_str::<XRoadTemplate>("kind: graphql\nmethod: POST\n").unwrap_err();
        assert!(
            err.to_string().contains("graphql"),
            "expected error naming bad kind, got: {err}"
        );
    }

    #[test]
    fn rest_dsl_omitted_query_field_forwards_all() {
        // Spec §4.5 default: unmodified pass-through. Absent field
        // in DSL means None which the executor interprets as "no
        // filter".
        let t = parse(
            "kind: rest\nmethod: GET\ntarget:\n  member_class: GOV\n  member_code: '1'\n  subsystem_code: s\n  service_code: c\n",
        );
        match t.kind {
            TemplateKind::Rest(r) => assert!(r.allowed_query_params.is_none()),
            _ => panic!("expected REST kind"),
        }
    }

    #[test]
    fn rest_dsl_empty_query_list_drops_all() {
        // Explicit []: paranoid opt-in for the SOAP-lane's
        // silent-drop posture. Distinguishable from omitted-field
        // via the Option layer.
        let t = parse(
            "kind: rest\nmethod: GET\ntarget:\n  member_class: GOV\n  member_code: '1'\n  subsystem_code: s\n  service_code: c\nallowed_query_params: []\n",
        );
        match t.kind {
            TemplateKind::Rest(r) => {
                assert_eq!(r.allowed_query_params.as_deref(), Some(&[][..]))
            }
            _ => panic!("expected REST kind"),
        }
    }

    #[test]
    fn identifier_charset_matches_spec_4_8() {
        // Allowed: A-Za-z0-9 '()+,-.=?
        assert!(identifier_chars_ok("GOV"));
        assert!(identifier_chars_ok("70008440"));
        assert!(identifier_chars_ok("DEV-TEST"));
        assert!(identifier_chars_ok("a.b.c"));
        // Disallowed:
        assert!(!identifier_chars_ok("has space"));
        assert!(!identifier_chars_ok("has/slash"));
        assert!(!identifier_chars_ok("has_underscore")); // '_' is NOT in the spec set
        assert!(!identifier_chars_ok("äöü"));            // non-ASCII
        assert!(!identifier_chars_ok(""));               // empty
    }

    #[test]
    fn rest_target_required_fields_detected() {
        let t = RestTarget {
            member_class: "GOV".into(),
            member_code: "".into(),
            subsystem_code: "s".into(),
            service_code: "c".into(),
            path: "".into(),
        };
        assert!(!t.required_fields_present());
    }
}
