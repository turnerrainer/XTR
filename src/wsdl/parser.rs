//! WSDL 1.1 parser — SOAP-1.1-over-HTTP subset X-Road actually uses.
//!
//! Supports named top-level complex types (Ariregister-style), inline
//! anonymous complex types, `xsd:include` via caller-provided loader,
//! `xsd:import` (skipped as framework schemas), `xsd:annotation`
//! (skipped as documentation). Bail-out-on-unsupported-construct
//! posture for `xsd:choice`, WSDL 2.0, RPC/encoded.
//!
//! Per-op lenient: an operation whose element cannot be resolved is
//! dropped from the output with a WARN, not fatal. The rest of the
//! WSDL still generates. Same rule at type-reference resolution:
//! unresolvable `type="ns:X"` yields the element as scalar with a
//! WARN, preserving whatever we could parse.

use crate::error::XtrError;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::BTreeMap;

/// Everything the generator needs to emit a DSL per operation.
#[derive(Debug, Clone)]
pub struct ParsedWsdl {
    /// Absolute URL from `<wsdl:service>/<wsdl:port>/<soap:address location="…"/>`.
    pub service_url: Option<String>,
    /// targetNamespace of the WSDL's `xsd:schema` — used to derive
    /// the `prod:` prefix in generated envelopes.
    pub target_namespace: String,
    /// One entry per resolvable `wsdl:operation` in the first
    /// portType. Multi-portType WSDLs are unsupported for v1.
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone)]
pub struct Operation {
    pub name: String,
    pub input_element: ElementDef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementDef {
    pub name: String,
    pub kind: ElementKind,
    /// True if `minOccurs="0"` — advisory only for now.
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementKind {
    /// Simple/typed leaf → `{{param}}` placeholder + one param.
    Scalar,
    /// Inline complex element or fully-resolved TypeRef → renders
    /// as `<prod:name>…</prod:name>` with children recursed.
    Complex { children: Vec<ElementDef> },
    /// Unresolved reference to a named complex type; resolved to
    /// `Complex` at `finish()` time via the state's named-types
    /// map. Left as-is if unresolvable (element skipped by
    /// resolver with a WARN — see `resolve_type_refs`).
    TypeRef { type_name: String },
}

impl ElementDef {
    /// Depth-first scalars reachable from this element. Params in
    /// the generated DSL are these leaves, in declaration order.
    pub fn scalar_leaves(&self) -> Vec<&ElementDef> {
        match &self.kind {
            ElementKind::Scalar => vec![self],
            ElementKind::Complex { children } => {
                children.iter().flat_map(|c| c.scalar_leaves()).collect()
            }
            // Unresolved refs contribute no scalar leaves — they
            // were dropped from their parent's children list.
            ElementKind::TypeRef { .. } => vec![],
        }
    }
}

/// Parse a WSDL with no include support.
pub fn parse(xml: &str) -> Result<ParsedWsdl, XtrError> {
    parse_with_loader(xml, |_| None)
}

/// Parse a WSDL, resolving `<xsd:include>` via a caller loader.
pub fn parse_with_loader<F>(xml: &str, loader: F) -> Result<ParsedWsdl, XtrError>
where
    F: Fn(&str) -> Option<String>,
{
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut state = ParseState::default();
    parse_stream(&mut reader, &mut state, &loader)?;
    state.finish()
}

#[derive(Default)]
struct ParseState {
    target_namespace: String,
    /// xsd:element name → parsed definition (may contain
    /// TypeRef children pending resolution).
    elements: BTreeMap<String, ElementDef>,
    /// Named complex types: local-name → child list.
    /// Collected from every schema (including included ones).
    /// Resolved to Complex at finish().
    named_types: BTreeMap<String, Vec<ElementDef>>,
    /// wsdl:message name → element name it points at.
    messages: BTreeMap<String, String>,
    /// wsdl:operation name → input message name.
    op_inputs: BTreeMap<String, String>,
    /// Preserve declaration order for stable output.
    op_order: Vec<String>,
    /// From <soap:address location="…"/>.
    service_url: Option<String>,
}

impl ParseState {
    fn finish(self) -> Result<ParsedWsdl, XtrError> {
        let mut operations = Vec::with_capacity(self.op_order.len());
        for op_name in &self.op_order {
            let Some(msg_name) = self.op_inputs.get(op_name) else {
                tracing::warn!("WSDL operation `{op_name}` has no input message — skipping");
                continue;
            };
            let Some(element_name) = self.messages.get(msg_name) else {
                tracing::warn!(
                    "WSDL message `{msg_name}` referenced by operation `{op_name}` \
                     is undefined — skipping op",
                );
                continue;
            };
            let Some(raw_element) = self.elements.get(element_name) else {
                tracing::warn!(
                    "WSDL element `{element_name}` referenced by message `{msg_name}` \
                     is undefined (likely an unresolved <xsd:include>) — skipping op `{op_name}`"
                );
                continue;
            };
            let resolved = resolve_type_refs(raw_element, &self.named_types, &mut vec![]);
            operations.push(Operation {
                name: op_name.clone(),
                input_element: resolved,
            });
        }
        Ok(ParsedWsdl {
            service_url: self.service_url,
            target_namespace: self.target_namespace,
            operations,
        })
    }
}

/// Walk an element tree, replacing every `TypeRef` with the
/// corresponding named type's `Complex { children }`. Unresolvable
/// refs are logged and the element is converted to `Scalar` so it
/// still exists in the tree but as a leaf (contributes one param).
/// This matches the "graceful degradation" theme — better to expose
/// a params list with SOME entries than to drop the whole op.
///
/// `stack` is a cycle-detection guard: a type referencing itself
/// (directly or transitively) would infinite-loop without it.
fn resolve_type_refs(
    el: &ElementDef,
    named_types: &BTreeMap<String, Vec<ElementDef>>,
    stack: &mut Vec<String>,
) -> ElementDef {
    let kind = match &el.kind {
        ElementKind::Scalar => ElementKind::Scalar,
        ElementKind::Complex { children } => ElementKind::Complex {
            children: children
                .iter()
                .map(|c| resolve_type_refs(c, named_types, stack))
                .collect(),
        },
        ElementKind::TypeRef { type_name } => {
            if stack.contains(type_name) {
                tracing::warn!(
                    "type `{type_name}` recurses; treating element `{}` as scalar to break cycle",
                    el.name
                );
                ElementKind::Scalar
            } else if let Some(children) = named_types.get(type_name) {
                stack.push(type_name.clone());
                let resolved_children: Vec<ElementDef> = children
                    .iter()
                    .map(|c| resolve_type_refs(c, named_types, stack))
                    .collect();
                stack.pop();
                ElementKind::Complex {
                    children: resolved_children,
                }
            } else {
                // Cross-namespace, misspelled, or type from a
                // framework schema we skipped. Fall back to
                // scalar so the element still surfaces.
                tracing::warn!(
                    "element `{}` has type=\"…:{}\" — type not defined in the \
                     WSDL schema; treating as scalar leaf",
                    el.name,
                    type_name
                );
                ElementKind::Scalar
            }
        }
    };
    ElementDef {
        name: el.name.clone(),
        kind,
        optional: el.optional,
    }
}

// -------- helpers --------

fn local_name(e: &BytesStart) -> String {
    let full = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    match full.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => full,
    }
}

fn local_name_end(e: &quick_xml::events::BytesEnd) -> String {
    let full = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    match full.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => full,
    }
}

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

fn strip_ns(qname: &str) -> &str {
    match qname.rsplit_once(':') {
        Some((_, local)) => local,
        None => qname,
    }
}

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

// -------- WSDL walk --------

fn parse_stream<F>(
    reader: &mut Reader<&[u8]>,
    state: &mut ParseState,
    loader: &F,
) -> Result<(), XtrError>
where
    F: Fn(&str) -> Option<String>,
{
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match local_name(&e).as_str() {
                "definitions" => {
                    if let Some(tns) = attr_local(&e, "targetNamespace") {
                        state.target_namespace = tns;
                    }
                }
                "schema" => parse_schema(reader, state, loader)?,
                "message" => {
                    let name = attr_local(&e, "name")
                        .ok_or_else(|| XtrError::Internal("WSDL <message> without name".into()))?;
                    parse_message(reader, state, name)?;
                }
                "portType" => parse_port_type(reader, state)?,
                "service" => parse_service(reader, state)?,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(XtrError::Internal(format!("WSDL parse error: {e}"))),
        }
    }
    if state.target_namespace.is_empty() {
        return Err(XtrError::Internal(
            "WSDL <definitions> is missing targetNamespace".into(),
        ));
    }
    Ok(())
}

// -------- schema walk --------

fn parse_schema<F>(
    reader: &mut Reader<&[u8]>,
    state: &mut ParseState,
    loader: &F,
) -> Result<(), XtrError>
where
    F: Fn(&str) -> Option<String>,
{
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match local_name(&e).as_str() {
                "element" => {
                    let name = attr_local(&e, "name").ok_or_else(|| {
                        XtrError::Internal("<xsd:element> without name at schema top level".into())
                    })?;
                    let has_type = attr_local(&e, "type").is_some();
                    let type_ref = attr_local(&e, "type").map(|t| strip_ns(&t).to_string());
                    let def = parse_element_body(reader, name.clone(), false, has_type, type_ref)?;
                    state.elements.insert(name, def);
                }
                "complexType" => {
                    // Named top-level or (rare) anonymous top-level.
                    if let Some(type_name) = attr_local(&e, "name") {
                        let children = parse_complex_type_body(reader)?;
                        state.named_types.insert(type_name, children);
                    } else {
                        skip_element(reader)?;
                    }
                }
                "simpleType" => {
                    // Named simple type — advisory only, we don't
                    // codegen restrictions.
                    skip_element(reader)?;
                }
                "import" => skip_element(reader)?,
                "include" => {
                    // Non-self-closing include is rare, but consume
                    // and skip its body. Loader invocation happens
                    // via the Empty-form branch below.
                    if let Some(loc) = attr_local(&e, "schemaLocation") {
                        include_schema_from_location(&loc, state, loader);
                    }
                    skip_element(reader)?;
                }
                "annotation" => skip_element(reader)?,
                _ => skip_element(reader)?,
            },
            Ok(Event::Empty(e)) => match local_name(&e).as_str() {
                "import" => {}
                "include" => {
                    if let Some(loc) = attr_local(&e, "schemaLocation") {
                        include_schema_from_location(&loc, state, loader);
                    }
                }
                "element" => {
                    // Self-closing top-level element.
                    if let Some(name) = attr_local(&e, "name") {
                        let type_ref = attr_local(&e, "type").map(|t| strip_ns(&t).to_string());
                        let optional = attr_local(&e, "minOccurs")
                            .map(|v| v == "0")
                            .unwrap_or(false);
                        let kind = match type_ref {
                            Some(t) if is_xsd_primitive(&t) => ElementKind::Scalar,
                            Some(t) => ElementKind::TypeRef { type_name: t },
                            None => ElementKind::Complex { children: vec![] },
                        };
                        state.elements.insert(
                            name.clone(),
                            ElementDef {
                                name,
                                kind,
                                optional,
                            },
                        );
                    }
                }
                _ => {}
            },
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

/// Parse the body of `<xsd:complexType name="X">…</xsd:complexType>`
/// after the Start event has been consumed. Returns the flat list
/// of children from the first `<xsd:sequence>` encountered.
/// `<xsd:choice>`, `<xsd:all>`, etc are unsupported — skipped
/// silently (element ends up with fewer children than the WSDL
/// author intended; the type-resolution step logs downgrades).
fn parse_complex_type_body(reader: &mut Reader<&[u8]>) -> Result<Vec<ElementDef>, XtrError> {
    let mut children: Vec<ElementDef> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match local_name(&e).as_str() {
                "sequence" => { /* descend */ }
                "annotation" => skip_element(reader)?,
                "complexContent" | "simpleContent" | "extension" | "restriction" => {
                    // Common shape: complexType > complexContent >
                    // extension base="Y" > sequence > element*
                    // Just descend so the sequence inside picks
                    // up children. `base=` inheritance is not
                    // followed — advisory.
                }
                "element" => {
                    let name = attr_local(&e, "name").ok_or_else(|| {
                        XtrError::Internal("child <xsd:element> without name".into())
                    })?;
                    let optional = attr_local(&e, "minOccurs")
                        .map(|v| v == "0")
                        .unwrap_or(false);
                    let has_type = attr_local(&e, "type").is_some();
                    let type_ref = attr_local(&e, "type").map(|t| strip_ns(&t).to_string());
                    let child = parse_element_body(reader, name, optional, has_type, type_ref)?;
                    children.push(child);
                }
                "choice" | "all" => skip_element(reader)?,
                _ => skip_element(reader)?,
            },
            Ok(Event::Empty(e)) if local_name(&e) == "element" => {
                if let Some(name) = attr_local(&e, "name") {
                    let optional = attr_local(&e, "minOccurs")
                        .map(|v| v == "0")
                        .unwrap_or(false);
                    let type_ref = attr_local(&e, "type").map(|t| strip_ns(&t).to_string());
                    let kind = match type_ref {
                        Some(t) if is_xsd_primitive(&t) => ElementKind::Scalar,
                        Some(t) => ElementKind::TypeRef { type_name: t },
                        None => ElementKind::Complex { children: vec![] },
                    };
                    children.push(ElementDef {
                        name,
                        kind,
                        optional,
                    });
                }
            }
            Ok(Event::End(e)) => match local_name_end(&e).as_str() {
                "complexType" => return Ok(children),
                "sequence" | "complexContent" | "simpleContent" | "extension" | "restriction" => {
                    /* pop */
                }
                _ => {}
            },
            Ok(Event::Eof) => {
                return Err(XtrError::Internal(
                    "unexpected EOF inside <xsd:complexType>".into(),
                ));
            }
            Ok(_) => {}
            Err(e) => return Err(XtrError::Internal(format!("WSDL parse error: {e}"))),
        }
    }
}

/// Parse the body of an `<xsd:element name="X" [type="…"]>…`.
///
/// - `type=` present → Scalar (or TypeRef if the type looks
///   local — resolved at finish() time).
/// - Inline `<complexType>` → Complex with children.
/// - Otherwise → Scalar (safe default; empty body = leaf).
fn parse_element_body(
    reader: &mut Reader<&[u8]>,
    name: String,
    optional: bool,
    has_type_attr: bool,
    type_ref: Option<String>,
) -> Result<ElementDef, XtrError> {
    // Element with a type attribute → resolve later or scalar now.
    if has_type_attr {
        let kind = if let Some(t) = type_ref {
            // xsd:string, xsd:int, xsd:date etc are XSD primitives
            // — treat as scalar directly. Anything else is a
            // named type reference to resolve at finish().
            if is_xsd_primitive(&t) {
                ElementKind::Scalar
            } else {
                ElementKind::TypeRef { type_name: t }
            }
        } else {
            ElementKind::Scalar
        };
        // Consume up to </element>.
        let mut depth: usize = 0;
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    if local_name(&e) == "annotation" {
                        skip_element(reader)?;
                    } else {
                        depth += 1;
                    }
                }
                Ok(Event::End(e)) => {
                    if depth == 0 && local_name_end(&e) == "element" {
                        return Ok(ElementDef {
                            name,
                            kind,
                            optional,
                        });
                    }
                    if depth > 0 {
                        depth -= 1;
                    }
                }
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
    // No type= — parse inline structure.
    let mut kind: Option<ElementKind> = None;
    let mut children: Vec<ElementDef> = Vec::new();
    let mut saw_complex_type = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match local_name(&e).as_str() {
                "complexType" => {
                    saw_complex_type = true;
                }
                "sequence" => {}
                "simpleType" => {
                    kind = Some(ElementKind::Scalar);
                    skip_element(reader)?;
                }
                "annotation" => skip_element(reader)?,
                "complexContent" | "simpleContent" | "extension" | "restriction" => {}
                "element" => {
                    let child_name = attr_local(&e, "name").ok_or_else(|| {
                        XtrError::Internal("child <xsd:element> without name".into())
                    })?;
                    let child_optional = attr_local(&e, "minOccurs")
                        .map(|v| v == "0")
                        .unwrap_or(false);
                    let child_has_type = attr_local(&e, "type").is_some();
                    let child_type = attr_local(&e, "type").map(|t| strip_ns(&t).to_string());
                    let child = parse_element_body(
                        reader,
                        child_name,
                        child_optional,
                        child_has_type,
                        child_type,
                    )?;
                    children.push(child);
                }
                "choice" | "all" => skip_element(reader)?,
                _ => skip_element(reader)?,
            },
            Ok(Event::Empty(e)) => match local_name(&e).as_str() {
                "sequence" => {}
                "element" => {
                    let child_name = attr_local(&e, "name").ok_or_else(|| {
                        XtrError::Internal("child <xsd:element/> without name".into())
                    })?;
                    let child_optional = attr_local(&e, "minOccurs")
                        .map(|v| v == "0")
                        .unwrap_or(false);
                    let child_type = attr_local(&e, "type").map(|t| strip_ns(&t).to_string());
                    let child_kind = if let Some(t) = child_type {
                        if is_xsd_primitive(&t) {
                            ElementKind::Scalar
                        } else {
                            ElementKind::TypeRef { type_name: t }
                        }
                    } else {
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
                        ElementKind::Scalar
                    };
                    return Ok(ElementDef {
                        name,
                        kind: resolved_kind,
                        optional,
                    });
                }
                "complexType" | "sequence" | "complexContent" | "simpleContent" | "extension"
                | "restriction" => {}
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

/// XSD built-in scalar types — anything else with an `xsd:`/`xs:`
/// prefix (or in the XSD namespace) still counts as a scalar for
/// our purposes since we don't codegen response types.
fn is_xsd_primitive(t: &str) -> bool {
    // Local name after strip_ns.
    matches!(
        t,
        "string"
            | "int"
            | "integer"
            | "long"
            | "short"
            | "byte"
            | "decimal"
            | "float"
            | "double"
            | "boolean"
            | "date"
            | "dateTime"
            | "time"
            | "duration"
            | "gYear"
            | "gMonth"
            | "gDay"
            | "gYearMonth"
            | "gMonthDay"
            | "anyURI"
            | "base64Binary"
            | "hexBinary"
            | "QName"
            | "NOTATION"
            | "normalizedString"
            | "token"
            | "language"
            | "NMTOKEN"
            | "Name"
            | "ID"
            | "IDREF"
            | "positiveInteger"
            | "nonNegativeInteger"
            | "negativeInteger"
            | "nonPositiveInteger"
            | "unsignedLong"
            | "unsignedInt"
            | "unsignedShort"
            | "unsignedByte"
            | "anyType"
    )
}

fn parse_message(
    reader: &mut Reader<&[u8]>,
    state: &mut ParseState,
    name: String,
) -> Result<(), XtrError> {
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if local_name(&e) == "part" => {
                if let Some(elem_ref) = attr_local(&e, "element") {
                    state
                        .messages
                        .insert(name.clone(), strip_ns(&elem_ref).to_string());
                }
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

fn include_schema_from_location<F>(location: &str, state: &mut ParseState, loader: &F)
where
    F: Fn(&str) -> Option<String>,
{
    let Some(content) = loader(location) else {
        tracing::warn!(
            "xsd:include schemaLocation=\"{}\" not resolvable — operations \
             depending on its elements will be skipped",
            location
        );
        return;
    };
    let mut reader = Reader::from_str(&content);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if local_name(&e) == "schema" => {
                if let Err(e) = parse_schema(&mut reader, state, loader) {
                    tracing::warn!(
                        "xsd:include schemaLocation=\"{}\" parse error: {} — \
                         skipping (its elements will be undefined)",
                        location,
                        e
                    );
                }
                return;
            }
            Ok(Event::Eof) => {
                tracing::warn!(
                    "xsd:include schemaLocation=\"{}\" had no <xsd:schema> root — skipping",
                    location
                );
                return;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    "xsd:include schemaLocation=\"{}\" read error: {} — skipping",
                    location,
                    e
                );
                return;
            }
        }
    }
}

// -------- tests --------

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
            other => panic!("expected complex element `{}`, got {other:?}", el.name),
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
        let leaves = w.operations[0].input_element.scalar_leaves();
        assert_eq!(leaves.len(), 3);
        assert_eq!(leaves[0].name, "ariregistri_kood");
        assert_eq!(leaves[1].name, "ariregister_kasutajanimi");
        assert_eq!(leaves[2].name, "ariregister_parool");
    }

    #[test]
    fn named_complex_type_referenced_by_type_attribute() {
        // Real X-Road pattern: element declared with type="ns:X",
        // X defined as a named top-level complexType elsewhere.
        let xml = r#"<?xml version="1.0"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:tns="http://ex/"
                  targetNamespace="http://ex/">
  <wsdl:types><xsd:schema targetNamespace="http://ex/">
    <xsd:complexType name="LookupInputType">
      <xsd:sequence>
        <xsd:element name="reg_code" type="xsd:string"/>
        <xsd:element name="verbose" type="xsd:boolean" minOccurs="0"/>
      </xsd:sequence>
    </xsd:complexType>
    <xsd:element name="lookup" type="tns:LookupInputType"/>
  </xsd:schema></wsdl:types>
  <wsdl:message name="m"><wsdl:part name="p" element="tns:lookup"/></wsdl:message>
  <wsdl:portType name="pt">
    <wsdl:operation name="lookup"><wsdl:input message="tns:m"/></wsdl:operation>
  </wsdl:portType>
</wsdl:definitions>"#;
        let w = parse(xml).unwrap();
        assert_eq!(w.operations.len(), 1);
        let leaves = w.operations[0].input_element.scalar_leaves();
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[0].name, "reg_code");
        assert_eq!(leaves[1].name, "verbose");
    }

    #[test]
    fn transitive_type_refs_resolved() {
        // Type A has an element of type B; B has scalars.
        let xml = r#"<?xml version="1.0"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:tns="http://ex/"
                  targetNamespace="http://ex/">
  <wsdl:types><xsd:schema targetNamespace="http://ex/">
    <xsd:complexType name="Inner">
      <xsd:sequence><xsd:element name="q" type="xsd:string"/></xsd:sequence>
    </xsd:complexType>
    <xsd:complexType name="Outer">
      <xsd:sequence><xsd:element name="inner" type="tns:Inner"/></xsd:sequence>
    </xsd:complexType>
    <xsd:element name="lookup" type="tns:Outer"/>
  </xsd:schema></wsdl:types>
  <wsdl:message name="m"><wsdl:part name="p" element="tns:lookup"/></wsdl:message>
  <wsdl:portType name="pt">
    <wsdl:operation name="lookup"><wsdl:input message="tns:m"/></wsdl:operation>
  </wsdl:portType>
</wsdl:definitions>"#;
        let w = parse(xml).unwrap();
        let leaves = w.operations[0].input_element.scalar_leaves();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].name, "q");
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
    fn xsd_choice_skipped_not_fatal() {
        // v1: xsd:choice is skipped silently (element ends up with
        // fewer children than the WSDL author intended). No error.
        let xml = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:tns="http://ex/"
                  targetNamespace="http://ex/">
  <wsdl:types>
    <xsd:schema targetNamespace="http://ex/">
      <xsd:element name="e">
        <xsd:complexType>
          <xsd:sequence>
            <xsd:element name="always" type="xsd:string"/>
            <xsd:choice>
              <xsd:element name="a" type="xsd:string"/>
              <xsd:element name="b" type="xsd:string"/>
            </xsd:choice>
          </xsd:sequence>
        </xsd:complexType>
      </xsd:element>
    </xsd:schema>
  </wsdl:types>
  <wsdl:message name="m"><wsdl:part name="p" element="tns:e"/></wsdl:message>
  <wsdl:portType name="pt">
    <wsdl:operation name="e"><wsdl:input message="tns:m"/></wsdl:operation>
  </wsdl:portType>
</wsdl:definitions>"#;
        let w = parse(xml).unwrap();
        // Should parse; 'always' present, choice branches dropped.
        let leaves = w.operations[0].input_element.scalar_leaves();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].name, "always");
    }

    #[test]
    fn xsd_import_is_silently_skipped() {
        let xml = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:tns="http://ex/"
                  targetNamespace="http://ex/">
  <wsdl:types>
    <xsd:schema targetNamespace="http://ex/">
      <xsd:import namespace="http://framework/" schemaLocation="framework.xsd"/>
      <xsd:element name="lookup">
        <xsd:complexType><xsd:sequence>
          <xsd:element name="q" type="xsd:string"/>
        </xsd:sequence></xsd:complexType>
      </xsd:element>
    </xsd:schema>
  </wsdl:types>
  <wsdl:message name="m"><wsdl:part name="p" element="tns:lookup"/></wsdl:message>
  <wsdl:portType name="pt">
    <wsdl:operation name="lookup"><wsdl:input message="tns:m"/></wsdl:operation>
  </wsdl:portType>
</wsdl:definitions>"#;
        let w = parse(xml).unwrap();
        assert_eq!(w.operations.len(), 1);
    }

    #[test]
    fn xsd_annotation_is_skipped_in_element_body() {
        let xml = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:tns="http://ex/"
                  targetNamespace="http://ex/">
  <wsdl:types>
    <xsd:schema targetNamespace="http://ex/">
      <xsd:element name="lookup" type="xsd:string">
        <xsd:annotation><xsd:documentation>doc text</xsd:documentation></xsd:annotation>
      </xsd:element>
    </xsd:schema>
  </wsdl:types>
  <wsdl:message name="m"><wsdl:part name="p" element="tns:lookup"/></wsdl:message>
  <wsdl:portType name="pt">
    <wsdl:operation name="lookup"><wsdl:input message="tns:m"/></wsdl:operation>
  </wsdl:portType>
</wsdl:definitions>"#;
        let w = parse(xml).unwrap();
        assert_eq!(w.operations.len(), 1);
        assert!(matches!(
            w.operations[0].input_element.kind,
            ElementKind::Scalar
        ));
    }

    #[test]
    fn unresolved_type_ref_falls_back_to_scalar() {
        // type=ar:NotDefined — no such type. The element should
        // remain in the tree as a scalar so its parent still has
        // structure.
        let xml = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:ar="http://a/"
                  xmlns:tns="http://ex/"
                  targetNamespace="http://ex/">
  <wsdl:types>
    <xsd:schema targetNamespace="http://ex/">
      <xsd:element name="lookup">
        <xsd:complexType><xsd:sequence>
          <xsd:element name="mystery" type="ar:NotDefined"/>
        </xsd:sequence></xsd:complexType>
      </xsd:element>
    </xsd:schema>
  </wsdl:types>
  <wsdl:message name="m"><wsdl:part name="p" element="tns:lookup"/></wsdl:message>
  <wsdl:portType name="pt">
    <wsdl:operation name="lookup"><wsdl:input message="tns:m"/></wsdl:operation>
  </wsdl:portType>
</wsdl:definitions>"#;
        let w = parse(xml).unwrap();
        let leaves = w.operations[0].input_element.scalar_leaves();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].name, "mystery");
    }
}
