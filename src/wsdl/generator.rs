//! WSDL → DSL YAML generation.
//!
//! Produces deterministic YAML: same WSDL always yields byte-equal
//! bytes so operators can `git diff` two runs and see only WSDL
//! changes.
//!
//! Two envelope shapes:
//!   - Plain SOAP (default) — when no sidecar metadata is present.
//!     Works for public SOAP services like Ariregister.
//!   - X-Road wrapped — when an `<wsdl>.meta.yaml` sidecar provides
//!     `{ member_class, member_code, subsystem_code, service_code }`,
//!     the generator injects the standard `<xroad:*>` header block
//!     with the auto-context placeholders.

use crate::error::XtrError;
use crate::wsdl::parser::{ElementDef, ElementKind, Operation, ParsedWsdl};
use crate::wsdl::MARKER;
use serde::Deserialize;

/// Optional per-WSDL metadata used to opt into X-Road envelope
/// wrapping. Placed next to the WSDL as `<name>.meta.yaml`.
#[derive(Debug, Clone, Deserialize)]
pub struct WsdlMeta {
    pub member_class: String,
    pub member_code: String,
    pub subsystem_code: String,
    /// Optional — falls back to the operation name if not set.
    #[serde(default)]
    pub service_code: Option<String>,
    /// Optional public-HTTPS override for vendors that ALSO expose
    /// their SOAP endpoint outside X-Road (Ariregister-style dual
    /// mode). When set, the generated DSL emits this as `service:`
    /// even if the WSDL's `<soap:address>` says TURVASERVER — so
    /// callers hit the public URL directly, no Security Server
    /// needed. Vendor auth (typically username/password in the
    /// SOAP body) is still required.
    #[serde(default)]
    pub service_url: Option<String>,
}

/// Emit one DSL YAML per operation in the parsed WSDL.
/// Returns `(operation_name, dsl_yaml)` pairs.
pub fn generate_all(
    wsdl: &ParsedWsdl,
    meta: Option<&WsdlMeta>,
) -> Result<Vec<(String, String)>, XtrError> {
    let mut out = Vec::with_capacity(wsdl.operations.len());
    for op in &wsdl.operations {
        let yaml = generate_one(wsdl, op, meta)?;
        out.push((op.name.clone(), yaml));
    }
    Ok(out)
}

/// Emit a single DSL YAML for one operation.
fn generate_one(
    wsdl: &ParsedWsdl,
    op: &Operation,
    meta: Option<&WsdlMeta>,
) -> Result<String, XtrError> {
    let leaves: Vec<&ElementDef> = op.input_element.scalar_leaves();
    let params: Vec<&str> = leaves.iter().map(|e| e.name.as_str()).collect();

    // X-Road header block only when the sidecar declares an X-Road
    // target AND there's no direct-HTTPS service_url override. A
    // service_url override means "hit the vendor URL directly,
    // skip X-Road" — the envelope stays plain SOAP.
    let envelope = match meta {
        Some(m) if m.service_url.is_none() => build_xroad_envelope(wsdl, op, m),
        _ => build_plain_envelope(wsdl, op),
    };

    let mut yaml = String::new();
    yaml.push_str(MARKER);
    yaml.push('\n');
    yaml.push_str("params:\n");
    if params.is_empty() {
        // Explicit empty list — keeps the shape consistent.
        yaml.push_str("  []\n");
    } else {
        for p in &params {
            yaml.push_str("  - ");
            yaml.push_str(p);
            yaml.push('\n');
        }
    }
    // Resolve the target URL in priority order:
    //   1. Explicit sidecar `service_url:` override (for dual-mode
    //      vendors that publish a public HTTPS endpoint alongside
    //      their X-Road subsystem — Ariregister-style).
    //   2. WSDL's `<soap:address location=…/>` — but only if it's
    //      NOT the X-Road TURVASERVER placeholder.
    //   3. Nothing — DSL omits `service:`, executor routes via
    //      `security_server:` config.
    let effective_service_url: Option<&str> =
        meta.and_then(|m| m.service_url.as_deref()).or_else(|| {
            wsdl.service_url
                .as_deref()
                .filter(|u| !is_xroad_placeholder_url(u))
        });
    if let Some(url) = effective_service_url {
        yaml.push_str("service: ");
        yaml.push_str(url);
        yaml.push('\n');
    }
    // Method is always POST for SOAP-over-HTTP. Task 013 v5
    // memory: XTR is POST-only by design.
    yaml.push_str("method: POST\n");
    yaml.push_str("envelope: |\n");
    for line in envelope.lines() {
        yaml.push_str("  ");
        yaml.push_str(line);
        yaml.push('\n');
    }
    Ok(yaml)
}

/// Plain SOAP envelope (no X-Road header block). Namespace prefix
/// `prod:` binds to the WSDL's targetNamespace — matches the JVM
/// XTR convention.
fn build_plain_envelope(wsdl: &ParsedWsdl, op: &Operation) -> String {
    let mut env = String::new();
    env.push_str(&format!(
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\" xmlns:prod=\"{}\">\n",
        wsdl.target_namespace
    ));
    env.push_str("<soapenv:Body>\n");
    render_input_element(&mut env, &op.input_element, 0);
    env.push_str("</soapenv:Body>\n");
    env.push_str("</soapenv:Envelope>");
    env
}

/// X-Road-wrapped envelope with the standard header block.
fn build_xroad_envelope(wsdl: &ParsedWsdl, op: &Operation, meta: &WsdlMeta) -> String {
    let service_code = meta.service_code.as_deref().unwrap_or(&op.name);
    let mut env = String::new();
    env.push_str(&format!(
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         xmlns:xroad=\"http://x-road.eu/xsd/xroad.xsd\" \
         xmlns:id=\"http://x-road.eu/xsd/identifiers\" \
         xmlns:prod=\"{}\">\n",
        wsdl.target_namespace
    ));
    env.push_str("<soapenv:Header>\n");
    env.push_str("{{{generate.client}}}\n");
    env.push_str("<xroad:service id:objectType=\"SERVICE\">\n");
    env.push_str("  <id:xRoadInstance>{{generate.instance}}</id:xRoadInstance>\n");
    env.push_str(&format!(
        "  <id:memberClass>{}</id:memberClass>\n",
        meta.member_class
    ));
    env.push_str(&format!(
        "  <id:memberCode>{}</id:memberCode>\n",
        meta.member_code
    ));
    env.push_str(&format!(
        "  <id:subsystemCode>{}</id:subsystemCode>\n",
        meta.subsystem_code
    ));
    env.push_str(&format!(
        "  <id:serviceCode>{}</id:serviceCode>\n",
        service_code
    ));
    env.push_str("</xroad:service>\n");
    env.push_str("<xroad:id>{{generate.uuid}}</xroad:id>\n");
    env.push_str("<xroad:protocolVersion>{{generate.protocol_version}}</xroad:protocolVersion>\n");
    env.push_str("</soapenv:Header>\n");
    env.push_str("<soapenv:Body>\n");
    render_input_element(&mut env, &op.input_element, 0);
    env.push_str("</soapenv:Body>\n");
    env.push_str("</soapenv:Envelope>");
    env
}

/// X-Road WSDLs use "TURVASERVER" (or a case variant) as the host
/// portion of `<soap:address location=>` to signal that the caller
/// must route through their own Security Server. Recognise the
/// pattern so we don't emit a broken `service:` URL.
fn is_xroad_placeholder_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains("turvaserver")
        || lower.contains("security-server")
        || lower.contains("your-security-server")
}

/// Recursively render the input element as `<prod:X>...</prod:X>`.
/// Scalars become `<prod:X>{{X}}</prod:X>` placeholders; complex
/// elements recurse (empty complex → `<prod:X/>`).
fn render_input_element(env: &mut String, el: &ElementDef, indent: usize) {
    let ind = "  ".repeat(indent);
    match &el.kind {
        ElementKind::Scalar => {
            env.push_str(&format!(
                "{ind}<prod:{name}>{{{{{name}}}}}</prod:{name}>\n",
                name = el.name
            ));
        }
        ElementKind::Complex { children } if children.is_empty() => {
            env.push_str(&format!("{ind}<prod:{}/>\n", el.name));
        }
        ElementKind::Complex { children } => {
            env.push_str(&format!("{ind}<prod:{}>\n", el.name));
            for child in children {
                render_input_element(env, child, indent + 1);
            }
            env.push_str(&format!("{ind}</prod:{}>\n", el.name));
        }
        // Unresolved TypeRef reaching the generator means the
        // type wasn't defined anywhere in the schema. Render as
        // a scalar placeholder so the element still exists on
        // the wire; scalar_leaves() already treats it as a
        // no-op for params, so this is the safest fallback.
        ElementKind::TypeRef { .. } => {
            env.push_str(&format!(
                "{ind}<prod:{name}>{{{{{name}}}}}</prod:{name}>\n",
                name = el.name
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wsdl::parser::parse;

    const ARIREG_LIKE_WSDL: &str = r#"<?xml version="1.0"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/"
                  xmlns:tns="http://arireg.x-road.eu/producer/"
                  targetNamespace="http://arireg.x-road.eu/producer/">
  <wsdl:types>
    <xsd:schema targetNamespace="http://arireg.x-road.eu/producer/">
      <xsd:element name="lihtandmed_v3">
        <xsd:complexType>
          <xsd:sequence>
            <xsd:element name="keha">
              <xsd:complexType>
                <xsd:sequence>
                  <xsd:element name="ariregistri_kood" type="xsd:string"/>
                  <xsd:element name="ariregister_kasutajanimi" type="xsd:string"/>
                  <xsd:element name="ariregister_parool" type="xsd:string"/>
                </xsd:sequence>
              </xsd:complexType>
            </xsd:element>
          </xsd:sequence>
        </xsd:complexType>
      </xsd:element>
    </xsd:schema>
  </wsdl:types>
  <wsdl:message name="in"><wsdl:part name="p" element="tns:lihtandmed_v3"/></wsdl:message>
  <wsdl:portType name="pt">
    <wsdl:operation name="lihtandmed_v3">
      <wsdl:input message="tns:in"/>
    </wsdl:operation>
  </wsdl:portType>
  <wsdl:service name="LookupService">
    <wsdl:port name="LookupPort" binding="tns:LookupBinding">
      <soap:address location="https://ariregxmlv6.rik.ee/"/>
    </wsdl:port>
  </wsdl:service>
</wsdl:definitions>"#;

    #[test]
    fn generates_plain_soap_dsl_from_ariregister_shape() {
        let wsdl = parse(ARIREG_LIKE_WSDL).unwrap();
        let files = generate_all(&wsdl, None).unwrap();
        assert_eq!(files.len(), 1);
        let (name, yaml) = &files[0];
        assert_eq!(name, "lihtandmed_v3");

        // Marker present
        assert!(yaml.starts_with(MARKER));
        // Params are the three leaves under <keha>, in WSDL order
        assert!(yaml.contains("- ariregistri_kood\n"));
        assert!(yaml.contains("- ariregister_kasutajanimi\n"));
        assert!(yaml.contains("- ariregister_parool\n"));
        // Service URL from <soap:address>
        assert!(yaml.contains("service: https://ariregxmlv6.rik.ee/\n"));
        // Method is always POST
        assert!(yaml.contains("method: POST\n"));
        // Envelope contains the wrapper <prod:keha> AND the leaf
        // placeholders inside it
        assert!(yaml.contains("<prod:lihtandmed_v3>"));
        assert!(yaml.contains("<prod:keha>"));
        assert!(
            yaml.contains("<prod:ariregistri_kood>{{ariregistri_kood}}</prod:ariregistri_kood>")
        );
        // No X-Road header block when metadata is absent
        assert!(!yaml.contains("<xroad:client"));
        assert!(!yaml.contains("<xroad:service"));
    }

    #[test]
    fn generator_output_is_deterministic() {
        let wsdl = parse(ARIREG_LIKE_WSDL).unwrap();
        let a = generate_all(&wsdl, None).unwrap();
        let b = generate_all(&wsdl, None).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn xroad_wrapping_when_meta_present() {
        let wsdl = parse(ARIREG_LIKE_WSDL).unwrap();
        let meta = WsdlMeta {
            member_class: "GOV".into(),
            member_code: "70000000".into(),
            subsystem_code: "arireg".into(),
            service_code: None,
            service_url: None,
        };
        let files = generate_all(&wsdl, Some(&meta)).unwrap();
        let yaml = &files[0].1;
        // X-Road header block present
        assert!(yaml.contains("{{{generate.client}}}"));
        assert!(yaml.contains("<xroad:service"));
        assert!(yaml.contains("<id:memberClass>GOV</id:memberClass>"));
        assert!(yaml.contains("<id:memberCode>70000000</id:memberCode>"));
        assert!(yaml.contains("<id:subsystemCode>arireg</id:subsystemCode>"));
        // service_code falls back to op.name when not set
        assert!(yaml.contains("<id:serviceCode>lihtandmed_v3</id:serviceCode>"));
        assert!(yaml.contains("{{generate.uuid}}"));
        assert!(yaml.contains("{{generate.protocol_version}}"));
    }

    #[test]
    fn service_url_override_uses_plain_soap_envelope_and_direct_url() {
        // Ariregister-style dual-mode: sidecar declares a
        // public-HTTPS override, WSDL says TURVASERVER, generated
        // DSL should route DIRECTLY to the vendor URL with a
        // plain SOAP envelope (no X-Road header).
        let wsdl = parse(ARIREG_LIKE_WSDL).unwrap();
        let meta = WsdlMeta {
            member_class: "GOV".into(),
            member_code: "70000310".into(),
            subsystem_code: "arireg".into(),
            service_code: None,
            service_url: Some("https://ariregxmlv6.rik.ee/".into()),
        };
        let files = generate_all(&wsdl, Some(&meta)).unwrap();
        let yaml = &files[0].1;
        assert!(yaml.contains("service: https://ariregxmlv6.rik.ee/\n"));
        // No X-Road header block when service_url override is set
        assert!(!yaml.contains("{{{generate.client}}}"));
        assert!(!yaml.contains("<xroad:service"));
    }

    #[test]
    fn empty_params_renders_as_explicit_empty_list() {
        // Corner case: operation whose input element has no
        // leaves (rare but valid — e.g. a no-arg listMethods).
        let xml = r#"<?xml version="1.0"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:tns="http://ex/" targetNamespace="http://ex/">
  <wsdl:types><xsd:schema targetNamespace="http://ex/">
    <xsd:element name="empty"><xsd:complexType><xsd:sequence/></xsd:complexType></xsd:element>
  </xsd:schema></wsdl:types>
  <wsdl:message name="m"><wsdl:part name="p" element="tns:empty"/></wsdl:message>
  <wsdl:portType name="pt">
    <wsdl:operation name="noArg"><wsdl:input message="tns:m"/></wsdl:operation>
  </wsdl:portType>
</wsdl:definitions>"#;
        let wsdl = parse(xml).unwrap();
        let files = generate_all(&wsdl, None).unwrap();
        let yaml = &files[0].1;
        // "params:" followed by "  []" then a newline
        assert!(
            yaml.contains("params:\n  []\n"),
            "expected explicit empty list, got:\n{yaml}"
        );
    }

    #[test]
    fn generated_yaml_parses_as_dsl() {
        // Sanity: what the generator emits must be loadable by
        // the existing DSL loader (round-trip through serde_yaml_ng).
        let wsdl = parse(ARIREG_LIKE_WSDL).unwrap();
        let files = generate_all(&wsdl, None).unwrap();
        for (op_name, yaml) in &files {
            let parsed: crate::dsl::XRoadTemplate = serde_yaml_ng::from_str(yaml)
                .unwrap_or_else(|e| panic!("op {op_name} DSL failed to parse: {e}\n{yaml}"));
            assert_eq!(parsed.method, "POST");
            assert!(parsed.service.is_some());
            assert!(parsed.envelope.contains("<prod:"));
        }
    }

    #[test]
    fn generated_envelope_passes_handlebars_validation() {
        // The startup Handlebars validation must accept every
        // envelope we emit — regression guard against generator
        // producing broken templates.
        let wsdl = parse(ARIREG_LIKE_WSDL).unwrap();
        let files = generate_all(&wsdl, None).unwrap();
        for (op_name, yaml) in &files {
            let parsed: crate::dsl::XRoadTemplate = serde_yaml_ng::from_str(yaml).unwrap();
            handlebars::Template::compile(&parsed.envelope).unwrap_or_else(|e| {
                panic!(
                    "op {op_name} envelope failed Handlebars validation: {e}\n{}",
                    parsed.envelope
                );
            });
        }
    }
}
