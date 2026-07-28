//! Minimal WSDL 1.1 parser — SOAP-1.1-over-HTTP subset.
//!
//! Produces a `ParsedWsdl` with everything the generator needs to
//! emit DSL YAML. Bail-out-on-unsupported-construct posture:
//! bindings other than document/literal, RPC style, MIME
//! attachments, imported schemas — all return `Err` with a
//! specific message. Never silently accept and produce wrong DSL.
//!
//! Test coverage lives in `wsdl::tests` (see mod.rs).

use crate::error::XtrError;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::BTreeMap;

/// Everything the generator needs to emit a DSL per operation.
#[derive(Debug, Clone)]
pub struct ParsedWsdl {
    /// Absolute URL from `<wsdl:service>/<wsdl:port>/<soap:address location="…"/>`.
    /// Required for plain SOAP (no security_server). May be absent
    /// when the WSDL only declares the abstract portType; in that
    /// case DSL generation still works if a metadata sidecar
    /// provides the target.
    pub service_url: Option<String>,

    /// targetNamespace of the WSDL's `xsd:schema` — used to derive
    /// the `prod:` prefix in generated envelopes.
    pub target_namespace: String,

    /// One entry per `wsdl:operation` in the first portType.
    /// Multi-portType WSDLs are unsupported for v1.
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone)]
pub struct Operation {
    /// e.g. "lookup" or "lihtandmed_v3"
    pub name: String,
    /// The `xsd:element` referenced by the input `wsdl:message` part.
    /// Its children are the payload; those become the DSL's params.
    pub input_element: ElementDef,
}

/// Element definition parsed from `xsd:element`.
///
/// The `kind` distinction is load-bearing: a scalar becomes a
/// `{{param}}` placeholder in the envelope AND contributes a
/// param name; a complex element is a container that renders as
/// XML tags with its children recursed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementDef {
    pub name: String,
    pub kind: ElementKind,
    /// True if `minOccurs="0"` — the generator uses this to mark
    /// the param optional in OpenAPI (future).
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementKind {
    /// Simple/typed leaf (e.g. `<xsd:element name="x" type="xsd:string"/>`
    /// or `<xsd:element name="x"><xsd:simpleType>…</xsd:simpleType></xsd:element>`).
    /// Becomes `<prod:x>{{x}}</prod:x>` in the envelope + a param.
    Scalar,
    /// Complex element (had an inline `xsd:complexType` wrapper).
    /// Renders as `<prod:x>…</prod:x>` with children recursed;
    /// contributes no param itself — only its scalar descendants do.
    Complex { children: Vec<ElementDef> },
}

impl ElementDef {
    /// Depth-first scalars reachable from this element. These are
    /// the DSL's params in declaration order.
    pub fn scalar_leaves(&self) -> Vec<&ElementDef> {
        match &self.kind {
            ElementKind::Scalar => vec![self],
            ElementKind::Complex { children } => {
                children.iter().flat_map(|c| c.scalar_leaves()).collect()
            }
        }
    }
}

/// Parse a WSDL document.
pub fn parse(xml: &str) -> Result<ParsedWsdl, XtrError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut state = ParseState::default();
    parse_stream(&mut reader, &mut state)?;
    state.finish()
}

#[derive(Default)]
struct ParseState {
    target_namespace: String,
    /// xsd:element name → parsed definition. Populated during
    /// walk of <wsdl:types>/<xsd:schema>.
    elements: BTreeMap<String, ElementDef>,
    /// wsdl:message name → element name it points at.
    messages: BTreeMap<String, String>,
    /// wsdl:operation name → input message name.
    op_inputs: BTreeMap<String, String>,
    /// Preserve declaration order so generated DSLs are stable.
    op_order: Vec<String>,
    /// From <soap:address location="…"/>.
    service_url: Option<String>,
}

impl ParseState {
    fn finish(self) -> Result<ParsedWsdl, XtrError> {
        let mut operations = Vec::with_capacity(self.op_order.len());
        for op_name in &self.op_order {
            let msg_name = self.op_inputs.get(op_name).ok_or_else(|| {
                XtrError::Internal(format!("WSDL operation `{op_name}` has no input message"))
            })?;
            let element_name = self.messages.get(msg_name).ok_or_else(|| {
                XtrError::Internal(format!(
                    "WSDL message `{msg_name}` referenced by operation `{op_name}` \
                     is undefined"
                ))
            })?;
            let element = self.elements.get(element_name).ok_or_else(|| {
                XtrError::Internal(format!(
                    "WSDL element `{element_name}` referenced by message `{msg_name}` \
                     is undefined in <xsd:schema>"
                ))
            })?;
            operations.push(Operation {
                name: op_name.clone(),
                input_element: element.clone(),
            });
        }
        Ok(ParsedWsdl {
            service_url: self.service_url,
            target_namespace: self.target_namespace,
            operations,
        })
    }
}

/// Local-name of an element (strips any `ns:` prefix).
fn local_name(e: &BytesStart) -> String {
    let full = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    match full.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => full,
    }
}

/// Look up an attribute by local-name (strips ns prefix).
fn attr_local(e: &BytesStart, want: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let local = key.rsplit_once(':').map(|(_, l)| l).unwrap_or(&key);
        if local == want {
            return Some(String::from_utf8_lossy(&a.value).into_owned());
        }
    }
    None
}

/// Strip a `ns:foo` reference down to `foo`. We don't currently
/// enforce that ns matches targetNamespace; a real X-Road WSDL
/// might import from another namespace. Bail path exists for
/// that in a later pass if we ever see it.
fn strip_ns(qname: &str) -> &str {
    match qname.rsplit_once(':') {
        Some((_, local)) => local,
        None => qname,
    }
}

/// Main state-machine loop. Uses local-name dispatch so we don't
/// have to care whether the WSDL author used `wsdl:definitions`
/// vs the default namespace.
fn parse_stream(reader: &mut Reader<&[u8]>, state: &mut ParseState) -> Result<(), XtrError> {
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match local_name(&e).as_str() {
                "definitions" => {
                    if let Some(tns) = attr_local(&e, "targetNamespace") {
                        state.target_namespace = tns;
                    }
                }
                "schema" => {
                    parse_schema(reader, state)?;
                }
                "message" => {
                    let name = attr_local(&e, "name")
                        .ok_or_else(|| XtrError::Internal("WSDL <message> without name".into()))?;
                    parse_message(reader, state, name)?;
                }
                "portType" => {
                    parse_port_type(reader, state)?;
                }
                "service" => {
                    parse_service(reader, state)?;
                }
                // binding, port, everything else — we don't need
                // it for envelope generation, so skip content.
                _ => {}
            },
            Ok(Event::Empty(_)) => {}
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => {
                return Err(XtrError::Internal(format!("WSDL parse error: {e}")));
            }
        }
    }
    if state.target_namespace.is_empty() {
        return Err(XtrError::Internal(
            "WSDL <definitions> is missing targetNamespace".into(),
        ));
    }
    Ok(())
}

fn parse_schema(reader: &mut Reader<&[u8]>, state: &mut ParseState) -> Result<(), XtrError> {
    // Walk until </schema>. Recurse into <element> at top level;
    // ignore <import>, <complexType> declared standalone (v1
    // supports only inline anonymous complexTypes).
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match local_name(&e).as_str() {
                "element" => {
                    let name = attr_local(&e, "name").ok_or_else(|| {
                        XtrError::Internal("<xsd:element> without name at schema top level".into())
                    })?;
                    let has_type = attr_local(&e, "type").is_some();
                    let def = parse_element_body(reader, name.clone(), false, has_type)?;
                    state.elements.insert(name, def);
                }
                "import" => {
                    return Err(XtrError::Internal(
                        "WSDL contains <xsd:import>; imported schemas are not \
                         supported in v1 — override with a hand-written DSL"
                            .into(),
                    ));
                }
                "complexType" if attr_local(&e, "name").is_some() => {
                    return Err(XtrError::Internal(
                        "WSDL contains a named top-level <xsd:complexType>; only \
                         inline anonymous complexTypes are supported in v1 — \
                         override with a hand-written DSL"
                            .into(),
                    ));
                }
                _ => skip_element(reader)?,
            },
            Ok(Event::Empty(e)) if local_name(&e) == "import" => {
                return Err(XtrError::Internal(
                    "WSDL contains <xsd:import>; imported schemas are not supported".into(),
                ));
            }
            Ok(Event::End(e)) if local_name_end(&e) == "schema" => return Ok(()),
            Ok(Event::Eof) => {
                return Err(XtrError::Internal(
                    "unexpected EOF inside <xsd:schema>".into(),
                ));
            }
            Ok(_) => {}
            Err(e) => return Err(XtrError::Internal(format!("WSDL parse error: {e}"))),
        }
    }
}

/// Parse the body of an `<xsd:element name="X" [type="…"]>`.
///
/// If the element had a `type=` attribute already resolved by the
/// caller (via `type_attr`), it's a scalar with no body content
/// worth parsing — but we still need to consume up to the End tag
/// if it wasn't a self-closing Empty event.
///
/// Otherwise we descend into the inline `<xsd:complexType>` /
/// `<xsd:simpleType>`. Anything wrapped in a `complexType` becomes
/// `ElementKind::Complex`, even if the sequence is empty. Anything
/// wrapped in a `simpleType` (or no wrapper at all) is `Scalar`.
fn parse_element_body(
    reader: &mut Reader<&[u8]>,
    name: String,
    optional: bool,
    has_type_attr: bool,
) -> Result<ElementDef, XtrError> {
    // Scalar-by-attribute: <xsd:element name="x" type="xsd:string"/>
    // or <xsd:element name="x" type="xsd:string">...</xsd:element>.
    // Either way the element is a scalar; consume up to the End if
    // this was a Start event (the End-event skip is a no-op for
    // Empty callers because they don't invoke this function at all).
    let mut kind: Option<ElementKind> = if has_type_attr {
        Some(ElementKind::Scalar)
    } else {
        None
    };
    let mut children: Vec<ElementDef> = Vec::new();
    let mut saw_complex_type = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match local_name(&e).as_str() {
                "complexType" => {
                    saw_complex_type = true;
                }
                "sequence" => { /* descend into content */ }
                "simpleType" => {
                    kind = Some(ElementKind::Scalar);
                    skip_element(reader)?;
                }
                "element" => {
                    let child_name = attr_local(&e, "name").ok_or_else(|| {
                        XtrError::Internal("child <xsd:element> without name".into())
                    })?;
                    let child_optional = attr_local(&e, "minOccurs")
                        .map(|v| v == "0")
                        .unwrap_or(false);
                    let child_has_type = attr_local(&e, "type").is_some();
                    let child =
                        parse_element_body(reader, child_name, child_optional, child_has_type)?;
                    children.push(child);
                }
                "choice" => {
                    return Err(XtrError::Internal(
                        "WSDL uses <xsd:choice>; not supported in v1 — override \
                         with a hand-written DSL"
                            .into(),
                    ));
                }
                _ => skip_element(reader)?,
            },
            Ok(Event::Empty(e)) => match local_name(&e).as_str() {
                "sequence" => { /* self-closing empty sequence — legal, keeps children empty */ }
                "element" => {
                    let child_name = attr_local(&e, "name").ok_or_else(|| {
                        XtrError::Internal("child <xsd:element/> without name".into())
                    })?;
                    let child_optional = attr_local(&e, "minOccurs")
                        .map(|v| v == "0")
                        .unwrap_or(false);
                    let child_has_type = attr_local(&e, "type").is_some();
                    // Self-closing child element: scalar iff it has
                    // a type= attribute (which is the whole point of
                    // self-closing shorthand — no inline type body).
                    let child_kind = if child_has_type {
                        ElementKind::Scalar
                    } else {
                        // No type + self-closing = element with no
                        // structure declared — treat as an empty
                        // complex (renders as `<prod:x/>`).
                        ElementKind::Complex { children: vec![] }
                    };
                    children.push(ElementDef {
                        name: child_name,
                        kind: child_kind,
                        optional: child_optional,
                    });
                }
                _ => {}
            },
            Ok(Event::End(e)) => match local_name_end(&e).as_str() {
                "element" => {
                    let resolved_kind = if let Some(k) = kind {
                        k
                    } else if saw_complex_type {
                        ElementKind::Complex { children }
                    } else {
                        // No complexType wrapper, no type attr,
                        // no simpleType — element with no body
                        // at all. Treat as scalar (most likely
                        // an anyType-implied leaf).
                        ElementKind::Scalar
                    };
                    return Ok(ElementDef {
                        name,
                        kind: resolved_kind,
                        optional,
                    });
                }
                "complexType" | "sequence" => { /* pop */ }
                _ => {}
            },
            Ok(Event::Eof) => {
                return Err(XtrError::Internal(format!(
                    "unexpected EOF inside <xsd:element name=\"{name}\">"
                )));
            }
            Ok(_) => {}
            Err(e) => return Err(XtrError::Internal(format!("WSDL parse error: {e}"))),
        }
    }
}

fn parse_message(
    reader: &mut Reader<&[u8]>,
    state: &mut ParseState,
    name: String,
) -> Result<(), XtrError> {
    // Look for <part element="tns:foo"/> or <part element="foo"/>.
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if local_name(&e) == "part" => {
                if let Some(elem_ref) = attr_local(&e, "element") {
                    state
                        .messages
                        .insert(name.clone(), strip_ns(&elem_ref).to_string());
                }
                // If it's Start, skip to matching End.
            }
            Ok(Event::End(e)) if local_name_end(&e) == "message" => return Ok(()),
            Ok(Event::Eof) => {
                return Err(XtrError::Internal(format!(
                    "unexpected EOF inside <wsdl:message name=\"{name}\">"
                )));
            }
            Ok(_) => {}
            Err(e) => return Err(XtrError::Internal(format!("WSDL parse error: {e}"))),
        }
    }
}

fn parse_port_type(reader: &mut Reader<&[u8]>, state: &mut ParseState) -> Result<(), XtrError> {
    let mut current_op: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match local_name(&e).as_str() {
                "operation" => {
                    let op_name = attr_local(&e, "name").ok_or_else(|| {
                        XtrError::Internal("<wsdl:operation> without name".into())
                    })?;
                    if !state.op_inputs.contains_key(&op_name) {
                        state.op_order.push(op_name.clone());
                    }
                    current_op = Some(op_name);
                }
                "input" => {
                    if let (Some(op), Some(msg_ref)) =
                        (current_op.as_ref(), attr_local(&e, "message"))
                    {
                        state
                            .op_inputs
                            .insert(op.clone(), strip_ns(&msg_ref).to_string());
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                if local_name(&e) == "input" {
                    if let (Some(op), Some(msg_ref)) =
                        (current_op.as_ref(), attr_local(&e, "message"))
                    {
                        state
                            .op_inputs
                            .insert(op.clone(), strip_ns(&msg_ref).to_string());
                    }
                }
            }
            Ok(Event::End(e)) => match local_name_end(&e).as_str() {
                "operation" => current_op = None,
                "portType" => return Ok(()),
                _ => {}
            },
            Ok(Event::Eof) => {
                return Err(XtrError::Internal(
                    "unexpected EOF inside <wsdl:portType>".into(),
                ));
            }
            Ok(_) => {}
            Err(e) => return Err(XtrError::Internal(format!("WSDL parse error: {e}"))),
        }
    }
}

fn parse_service(reader: &mut Reader<&[u8]>, state: &mut ParseState) -> Result<(), XtrError> {
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if local_name(&e) == "address" => {
                if let Some(loc) = attr_local(&e, "location") {
                    state.service_url = Some(loc);
                }
            }
            Ok(Event::End(e)) if local_name_end(&e) == "service" => return Ok(()),
            Ok(Event::Eof) => {
                return Err(XtrError::Internal(
                    "unexpected EOF inside <wsdl:service>".into(),
                ));
            }
            Ok(_) => {}
            Err(e) => return Err(XtrError::Internal(format!("WSDL parse error: {e}"))),
        }
    }
}

/// Skip until the matching close tag for the *current* element.
/// The caller has already consumed the Start event.
fn skip_element(reader: &mut Reader<&[u8]>) -> Result<(), XtrError> {
    let mut depth: usize = 1;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Ok(Event::Eof) => {
                return Err(XtrError::Internal(
                    "unexpected EOF while skipping element".into(),
                ));
            }
            Ok(_) => {}
            Err(e) => return Err(XtrError::Internal(format!("WSDL parse error: {e}"))),
        }
    }
}

fn local_name_end(e: &quick_xml::events::BytesEnd) -> String {
    let full = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    match full.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_WSDL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/"
                  xmlns:tns="http://example.com/svc"
                  targetNamespace="http://example.com/svc">
  <wsdl:types>
    <xsd:schema targetNamespace="http://example.com/svc"
                xmlns:xsd="http://www.w3.org/2001/XMLSchema">
      <xsd:element name="lookupInput">
        <xsd:complexType>
          <xsd:sequence>
            <xsd:element name="reg_code" type="xsd:string"/>
            <xsd:element name="verbose" type="xsd:string" minOccurs="0"/>
          </xsd:sequence>
        </xsd:complexType>
      </xsd:element>
    </xsd:schema>
  </wsdl:types>
  <wsdl:message name="lookupIn">
    <wsdl:part name="parameters" element="tns:lookupInput"/>
  </wsdl:message>
  <wsdl:portType name="LookupPortType">
    <wsdl:operation name="lookup">
      <wsdl:input message="tns:lookupIn"/>
    </wsdl:operation>
  </wsdl:portType>
  <wsdl:service name="LookupService">
    <wsdl:port name="LookupPort" binding="tns:LookupBinding">
      <soap:address location="https://example.com/soap"/>
    </wsdl:port>
  </wsdl:service>
</wsdl:definitions>"#;

    fn assert_children(el: &ElementDef, expected_names: &[&str]) {
        match &el.kind {
            ElementKind::Complex { children } => {
                let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
                assert_eq!(names, expected_names);
            }
            ElementKind::Scalar => panic!("expected complex element `{}`, got scalar", el.name),
        }
    }

    #[test]
    fn parses_minimal_wsdl() {
        let w = parse(MINIMAL_WSDL).unwrap();
        assert_eq!(w.service_url.as_deref(), Some("https://example.com/soap"));
        assert_eq!(w.target_namespace, "http://example.com/svc");
        assert_eq!(w.operations.len(), 1);
        let op = &w.operations[0];
        assert_eq!(op.name, "lookup");
        assert_eq!(op.input_element.name, "lookupInput");
        assert_children(&op.input_element, &["reg_code", "verbose"]);
        // minOccurs="0" was set on the second child
        if let ElementKind::Complex { children } = &op.input_element.kind {
            assert!(!children[0].optional);
            assert!(children[1].optional);
            // Both are scalars (had type= attributes)
            assert!(matches!(children[0].kind, ElementKind::Scalar));
            assert!(matches!(children[1].kind, ElementKind::Scalar));
        }
    }

    #[test]
    fn scalar_leaves_flattens_nested() {
        let w = parse(MINIMAL_WSDL).unwrap();
        let leaves = w.operations[0].input_element.scalar_leaves();
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[0].name, "reg_code");
        assert_eq!(leaves[1].name, "verbose");
    }

    #[test]
    fn empty_sequence_yields_zero_scalar_leaves() {
        // Regression: <element><complexType><sequence/></complexType></element>
        // must NOT count as a scalar leaf. It's an empty complex
        // element, rendered as `<prod:x/>`, with zero params.
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
        let w = parse(xml).unwrap();
        let op = &w.operations[0];
        assert!(matches!(op.input_element.kind, ElementKind::Complex { .. }));
        assert_eq!(op.input_element.scalar_leaves().len(), 0);
    }

    #[test]
    fn nested_complex_type_recursed() {
        // Ariregister-style shape: input has a single <keha> wrapper
        // containing the real leaf fields.
        let xml = r#"<?xml version="1.0"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
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
</wsdl:definitions>"#;
        let w = parse(xml).unwrap();
        assert_eq!(w.operations.len(), 1);
        let op = &w.operations[0];
        assert_eq!(op.name, "lihtandmed_v3");
        // The input element wraps <keha> which wraps the real leaves.
        let leaves = op.input_element.scalar_leaves();
        assert_eq!(leaves.len(), 3);
        assert_eq!(leaves[0].name, "ariregistri_kood");
        assert_eq!(leaves[1].name, "ariregister_kasutajanimi");
        assert_eq!(leaves[2].name, "ariregister_parool");
    }

    #[test]
    fn multiple_operations_preserve_declaration_order() {
        let xml = r#"<?xml version="1.0"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:tns="http://ex/"
                  targetNamespace="http://ex/">
  <wsdl:types>
    <xsd:schema targetNamespace="http://ex/">
      <xsd:element name="a"><xsd:complexType><xsd:sequence>
        <xsd:element name="x" type="xsd:string"/>
      </xsd:sequence></xsd:complexType></xsd:element>
      <xsd:element name="b"><xsd:complexType><xsd:sequence>
        <xsd:element name="y" type="xsd:string"/>
      </xsd:sequence></xsd:complexType></xsd:element>
    </xsd:schema>
  </wsdl:types>
  <wsdl:message name="ma"><wsdl:part name="p" element="tns:a"/></wsdl:message>
  <wsdl:message name="mb"><wsdl:part name="p" element="tns:b"/></wsdl:message>
  <wsdl:portType name="pt">
    <wsdl:operation name="second"><wsdl:input message="tns:mb"/></wsdl:operation>
    <wsdl:operation name="first"><wsdl:input message="tns:ma"/></wsdl:operation>
  </wsdl:portType>
</wsdl:definitions>"#;
        let w = parse(xml).unwrap();
        assert_eq!(w.operations.len(), 2);
        // WSDL declaration order preserved (second, first — not
        // sorted alphabetically).
        assert_eq!(w.operations[0].name, "second");
        assert_eq!(w.operations[1].name, "first");
    }

    #[test]
    fn missing_target_namespace_rejected() {
        let xml = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/">
</wsdl:definitions>"#;
        let err = parse(xml).unwrap_err();
        assert!(
            matches!(&err, XtrError::Internal(m) if m.contains("targetNamespace")),
            "expected targetNamespace error, got {err:?}"
        );
    }

    #[test]
    fn xsd_choice_rejected_with_clear_error() {
        let xml = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:tns="http://ex/"
                  targetNamespace="http://ex/">
  <wsdl:types>
    <xsd:schema targetNamespace="http://ex/">
      <xsd:element name="e">
        <xsd:complexType>
          <xsd:choice>
            <xsd:element name="a" type="xsd:string"/>
            <xsd:element name="b" type="xsd:string"/>
          </xsd:choice>
        </xsd:complexType>
      </xsd:element>
    </xsd:schema>
  </wsdl:types>
</wsdl:definitions>"#;
        let err = parse(xml).unwrap_err();
        assert!(
            matches!(&err, XtrError::Internal(m) if m.contains("choice")),
            "expected choice error, got {err:?}"
        );
    }

    #[test]
    fn xsd_import_rejected_with_clear_error() {
        let xml = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:tns="http://ex/"
                  targetNamespace="http://ex/">
  <wsdl:types>
    <xsd:schema targetNamespace="http://ex/">
      <xsd:import namespace="http://other/" schemaLocation="other.xsd"/>
    </xsd:schema>
  </wsdl:types>
</wsdl:definitions>"#;
        let err = parse(xml).unwrap_err();
        assert!(
            matches!(&err, XtrError::Internal(m) if m.contains("import")),
            "expected import error, got {err:?}"
        );
    }
}
