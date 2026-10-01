//! WSDL 1.1 → inbound contract.
//!
//! Independent of `wsdl::parser` on purpose: that parser only needs
//! the *input* element flattened into Handlebars placeholders, and
//! deliberately drops attributes / `xs:choice` / `maxOccurs`. The
//! inbound lane needs something different:
//!
//! * operation dispatch keys — input element QName + `soapAction`;
//! * the output element QName (absent = one-way operation);
//! * schema *hints* for the codec — child element order (XSD
//!   `sequence` order, including `extension` base types), which
//!   children repeat (`maxOccurs > 1`), and whether local elements
//!   are namespace-qualified (`elementFormDefault`).
//!
//! It is NOT a validating schema model. Attributes need no schema
//! support at all: the codec carries them as `@name` keys.

use super::dom::{self, Element};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema";
const WSDL_NS: &str = "http://schemas.xmlsoap.org/wsdl/";
const SOAP11_BINDING_NS: &str = "http://schemas.xmlsoap.org/wsdl/soap/";
const XROAD_NS: &str = "http://x-road.eu/xsd/xroad.xsd";

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QName {
    pub ns: Option<String>,
    pub local: String,
}

impl std::fmt::Display for QName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.ns {
            Some(ns) => write!(f, "{{{ns}}}{}", self.local),
            None => write!(f, "{}", self.local),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Operation {
    pub name: String,
    pub soap_action: Option<String>,
    pub input: QName,
    /// `None` → one-way operation (HTTP 202, empty body).
    pub output: Option<QName>,
    /// `<xrd:version>` from the binding (X-Road SOAP WSDLs) — becomes
    /// `<id:serviceVersion>` in outbound X-Road headers.
    pub xroad_version: Option<String>,
}

#[derive(Debug, Clone)]
pub enum TypeRef {
    /// Named complex type (local name; namespaces are not tracked
    /// for types — one WSDL is one vocabulary in practice).
    Named(String),
    Inline(Arc<TypeDef>),
    /// Reference to a global element (`<xs:element ref="x"/>`).
    ElementRef(String),
    Simple,
}

#[derive(Debug, Clone, Default)]
pub struct TypeDef {
    /// `complexContent/extension@base` — its particles come first.
    pub base: Option<String>,
    pub particles: Vec<Particle>,
}

#[derive(Debug, Clone)]
pub struct Particle {
    pub name: String,
    pub repeated: bool,
    pub ty: TypeRef,
    /// Effective namespace of this element on the wire, computed where
    /// the declaration lives: `ref=` → the referenced global element's
    /// namespace; local → the declaring schema's targetNamespace iff
    /// `form="qualified"` (or no `form` and that schema's
    /// `elementFormDefault="qualified"`), else no namespace.
    pub ns: Option<String>,
}

#[derive(Debug, Default)]
pub struct Schema {
    /// Global elements by (namespace, local).
    pub elements: HashMap<QName, TypeRef>,
    /// Global elements by local name only (for `ref=` resolution).
    elements_by_local: HashMap<String, (QName, TypeRef)>,
    /// Named complex types by local name.
    pub types: HashMap<String, Arc<TypeDef>>,
    /// Named simple types by local name (only their existence matters).
    simple_types: BTreeSet<String>,
    /// Global element → was its schema document `elementFormDefault=
    /// "qualified"` (decides the encoder's default-namespace strategy).
    qualified_roots: HashMap<QName, bool>,
}

impl Schema {
    pub fn element(&self, q: &QName) -> Option<&TypeRef> {
        self.elements.get(q)
    }

    pub fn global_by_local(&self, local: &str) -> Option<&(QName, TypeRef)> {
        self.elements_by_local.get(local)
    }

    /// Whether the global element `q` lives in an
    /// `elementFormDefault="qualified"` schema document.
    pub fn root_qualified(&self, q: &QName) -> bool {
        self.qualified_roots.get(q).copied().unwrap_or(false)
    }

    /// Resolve a `TypeRef` to its complex definition (None = simple
    /// or unknown).
    pub fn resolve<'a>(&'a self, ty: &'a TypeRef) -> Option<&'a TypeDef> {
        match ty {
            TypeRef::Inline(td) => Some(td),
            TypeRef::Named(n) => self.types.get(n).map(|r| r.as_ref()),
            TypeRef::ElementRef(n) => self.elements_by_local.get(n).and_then(|(_, t)| match t {
                TypeRef::ElementRef(_) => None, // no ref chains
                other => self.resolve(other),
            }),
            TypeRef::Simple => None,
        }
    }

    /// All particles of `td` in document order, base types first.
    pub fn particles<'a>(&'a self, td: &'a TypeDef) -> Vec<&'a Particle> {
        let mut chain: Vec<&TypeDef> = vec![td];
        let mut cur = td;
        while let Some(base) = cur.base.as_deref() {
            match self.types.get(base) {
                Some(b) if chain.len() < 16 => {
                    chain.push(b);
                    cur = b;
                }
                _ => break,
            }
        }
        chain
            .iter()
            .rev()
            .flat_map(|t| t.particles.iter())
            .collect()
    }

    pub fn particle<'a>(&'a self, td: &'a TypeDef, name: &str) -> Option<&'a Particle> {
        self.particles(td).into_iter().find(|p| p.name == name)
    }
}

#[derive(Debug)]
pub struct Contract {
    pub operations: Vec<Operation>,
    pub schema: Schema,
    /// `<soap:address location>` as written (rewritten on `?wsdl`).
    pub address: Option<String>,
    /// Filenames of local XSDs pulled in via include/import — the
    /// only files `GET /soap-in/<group>/<file>.xsd` will serve.
    pub schema_files: BTreeMap<String, PathBuf>,
    /// True when the WSDL declared a SOAP 1.1 binding. SOAP 1.2-only
    /// WSDLs are refused (this lane speaks SOAP 1.1).
    pub soap11: bool,
}

impl Contract {
    pub fn op_for_input(&self, q: &QName) -> Option<&Operation> {
        self.operations.iter().find(|o| &o.input == q)
    }
}

/// Loader for local schema files: `location` → (filename, content).
pub type SchemaLoader<'a> = dyn Fn(&str) -> Option<(String, PathBuf, String)> + 'a;

pub fn load(wsdl_xml: &str, loader: &SchemaLoader) -> Result<Contract, String> {
    let root = dom::parse(wsdl_xml).map_err(|e| e.0)?;
    if root.local != "definitions" || root.ns.as_deref() != Some(WSDL_NS) {
        return Err("root element is not a WSDL 1.1 <wsdl:definitions>".into());
    }
    let mut schema = Schema::default();
    let mut schema_files = BTreeMap::new();
    let mut visited = BTreeSet::new();
    let mut problems: Vec<String> = Vec::new();
    if let Some(types) = child_ns(&root, WSDL_NS, "types") {
        for s in types.elements().filter(|e| is_xsd(e, "schema")) {
            let mut ing = Ingest {
                schema: &mut schema,
                loader,
                files: &mut schema_files,
                visited: &mut visited,
                problems: &mut problems,
            };
            ing.schema_doc(s, None, 0);
        }
    }

    // message name → element QName (first part with element=).
    let mut messages: HashMap<String, QName> = HashMap::new();
    for m in root.elements().filter(|e| is_wsdl(e, "message")) {
        let Some(name) = m.attr("name") else { continue };
        if let Some(part) = m.elements().filter(|e| is_wsdl(e, "part")).find_map(|p| {
            p.attr("element").map(|q| {
                let (ns, local) = p.resolve_qname(q);
                QName { ns, local }
            })
        }) {
            messages.insert(name.to_string(), part);
        }
    }

    // First SOAP 1.1 binding → its portType (operations) and the
    // service/port that uses it (address). Everything else in the WSDL
    // (SOAP 1.2 bindings, other portTypes and ports) is ignored, so the
    // address always matches the binding XTR speaks.
    let mut actions: HashMap<String, String> = HashMap::new();
    let mut versions: HashMap<String, String> = HashMap::new();
    let mut soap11 = false;
    let mut binding_name: Option<String> = None;
    let mut port_type: Option<String> = None;
    for b in root.elements().filter(|e| is_wsdl(e, "binding")) {
        if b.elements()
            .any(|e| e.local == "binding" && e.ns.as_deref() == Some(SOAP11_BINDING_NS))
        {
            soap11 = true;
            binding_name = b.attr("name").map(String::from);
            port_type = b.attr("type").map(|t| b.resolve_qname(t).1);
            for op in b.elements().filter(|e| is_wsdl(e, "operation")) {
                if let (Some(name), Some(v)) = (
                    op.attr("name"),
                    op.elements()
                        .find(|e| e.local == "version" && e.ns.as_deref() == Some(XROAD_NS)),
                ) {
                    versions.insert(name.to_string(), v.text().trim().to_string());
                }
                let (Some(name), Some(sop)) = (
                    op.attr("name"),
                    op.elements().find(|e| {
                        e.local == "operation" && e.ns.as_deref() == Some(SOAP11_BINDING_NS)
                    }),
                ) else {
                    continue;
                };
                if let Some(a) = sop.attr("soapAction") {
                    actions.insert(name.to_string(), a.to_string());
                }
            }
            break;
        }
    }

    let mut operations = Vec::new();
    let mut seen = BTreeSet::new();
    for pt in root
        .elements()
        .filter(|e| is_wsdl(e, "portType"))
        .filter(|e| port_type.is_none() || e.attr("name") == port_type.as_deref())
    {
        for op in pt.elements().filter(|e| is_wsdl(e, "operation")) {
            let Some(name) = op.attr("name") else {
                continue;
            };
            if !seen.insert(name.to_string()) {
                continue;
            }
            let msg = |kind: &str| {
                child_ns(op, WSDL_NS, kind)
                    .and_then(|io| io.attr("message").map(|q| io.resolve_qname(q).1))
            };
            let Some(input) = msg("input").and_then(|m| messages.get(&m).cloned()) else {
                tracing::warn!(op = ?name, "inbound WSDL: operation has no resolvable input element — skipped");
                continue;
            };
            let output = match msg("output") {
                Some(m) => match messages.get(&m) {
                    Some(q) => Some(q.clone()),
                    None => {
                        // Seen in real vendor WSDLs: output message
                        // referenced but never declared. Fall back to the
                        // `<op>Response` / `_Request→_Response` convention if
                        // such a global element exists; otherwise one-way.
                        let guess = guess_output(&input, &schema);
                        tracing::warn!(op = ?name, message = ?m, guessed = ?guess.as_ref().map(|q| q.to_string()),
                            "inbound WSDL: output message undeclared");
                        guess
                    }
                },
                None => None,
            };
            operations.push(Operation {
                name: name.to_string(),
                soap_action: actions.get(name).cloned(),
                input,
                output,
                xroad_version: versions.get(name).cloned(),
            });
        }
    }

    let address = root
        .elements()
        .filter(|e| is_wsdl(e, "service"))
        .flat_map(|s| s.elements().filter(|e| is_wsdl(e, "port")))
        .filter(|p| {
            binding_name.is_none()
                || p.attr("binding").map(|b| p.resolve_qname(b).1) == binding_name
        })
        .flat_map(|p| {
            p.elements()
                .filter(|e| e.local == "address" && e.ns.as_deref() == Some(SOAP11_BINDING_NS))
        })
        .find_map(|a| a.attr("location").map(String::from));

    // Every type the operations' messages use must be resolvable —
    // a missing xs:include must not silently degrade the codec (lost
    // order / repetition info) while the service looks healthy.
    let mut unresolved = BTreeSet::new();
    for op in &operations {
        for q in std::iter::once(&op.input).chain(op.output.iter()) {
            match schema.elements.get(q) {
                None => {
                    unresolved.insert(format!("element {q}"));
                }
                Some(ty) => check_type(&schema, ty, &mut BTreeSet::new(), &mut unresolved),
            }
        }
    }
    problems.extend(unresolved.into_iter().map(|u| format!("unresolved {u}")));
    if !problems.is_empty() {
        return Err(format!("schema problems: {}", problems.join("; ")));
    }

    Ok(Contract {
        operations,
        schema,
        address,
        schema_files,
        soap11,
    })
}

fn guess_output(input: &QName, schema: &Schema) -> Option<QName> {
    let candidates = [
        input.local.replace("_Request", "_Response"),
        format!("{}Response", input.local),
    ];
    candidates.into_iter().find_map(|local| {
        let q = QName {
            ns: input.ns.clone(),
            local,
        };
        (q.local != input.local && schema.elements.contains_key(&q)).then_some(q)
    })
}

fn is_wsdl(e: &Element, local: &str) -> bool {
    e.local == local && e.ns.as_deref() == Some(WSDL_NS)
}

fn is_xsd(e: &Element, local: &str) -> bool {
    e.local == local && e.ns.as_deref() == Some(XSD_NS)
}

fn child_ns<'a>(e: &'a Element, ns: &str, local: &str) -> Option<&'a Element> {
    e.elements()
        .find(|c| c.local == local && c.ns.as_deref() == Some(ns))
}

/// Walk a type and record references that don't resolve.
fn check_type(
    schema: &Schema,
    ty: &TypeRef,
    seen: &mut BTreeSet<String>,
    out: &mut BTreeSet<String>,
) {
    let td: Option<&TypeDef> = match ty {
        TypeRef::Simple => None,
        TypeRef::Inline(td) => Some(td),
        TypeRef::Named(n) => {
            if schema.simple_types.contains(n) {
                None
            } else if !seen.insert(n.clone()) {
                return;
            } else {
                match schema.types.get(n) {
                    Some(td) => Some(td),
                    None => {
                        out.insert(format!("type {n}"));
                        None
                    }
                }
            }
        }
        TypeRef::ElementRef(n) => {
            match schema.elements_by_local.get(n) {
                Some((_, t)) if !matches!(t, TypeRef::ElementRef(_)) => {
                    if seen.insert(format!("@{n}")) {
                        check_type(schema, t, seen, out);
                    }
                }
                Some(_) => {}
                None => {
                    out.insert(format!("element ref {n}"));
                }
            }
            None
        }
    };
    let Some(td) = td else { return };
    if let Some(base) = &td.base {
        // Recurse through the whole extension chain (the base's own
        // particles and its own base); `seen` breaks cycles.
        if !schema.types.contains_key(base) && !schema.simple_types.contains(base) {
            out.insert(format!("base type {base}"));
        } else {
            check_type(schema, &TypeRef::Named(base.clone()), seen, out);
        }
    }
    for p in &td.particles {
        check_type(schema, &p.ty, seen, out);
    }
}

/// Namespace context of one schema document.
#[derive(Clone)]
struct DocCtx {
    tns: Option<String>,
    qualified: bool,
}

struct Ingest<'a, 'l> {
    schema: &'a mut Schema,
    loader: &'a SchemaLoader<'l>,
    files: &'a mut BTreeMap<String, PathBuf>,
    visited: &'a mut BTreeSet<String>,
    problems: &'a mut Vec<String>,
}

impl Ingest<'_, '_> {
    /// `inherited_tns`: for `xs:include` of a schema without its own
    /// targetNamespace ("chameleon" include) the includer's applies.
    fn schema_doc(&mut self, s: &Element, inherited_tns: Option<String>, depth: usize) {
        let ctx = DocCtx {
            tns: s
                .attr("targetNamespace")
                .map(String::from)
                .or(inherited_tns),
            qualified: s.attr("elementFormDefault") == Some("qualified"),
        };
        for c in s.elements() {
            if c.ns.as_deref() != Some(XSD_NS) {
                continue;
            }
            match c.local.as_str() {
                kind @ ("include" | "import") => {
                    let Some(loc) = c.attr("schemaLocation") else {
                        continue;
                    };
                    if depth > 16 {
                        self.problems
                            .push(format!("schema include depth > 16 at {loc}"));
                        continue;
                    }
                    let remote = loc.starts_with("http://") || loc.starts_with("https://");
                    match (self.loader)(loc) {
                        Some((file, path, xml)) => {
                            if !self.visited.insert(file.clone()) {
                                continue;
                            }
                            self.files.insert(file.clone(), path);
                            match dom::parse(&xml) {
                                Ok(inc) if is_xsd(&inc, "schema") => {
                                    let inherit =
                                        (kind == "include").then(|| ctx.tns.clone()).flatten();
                                    self.schema_doc(&inc, inherit, depth + 1)
                                }
                                Ok(_) => {
                                    self.problems.push(format!("{file} is not an <xs:schema>"))
                                }
                                Err(e) => self.problems.push(format!("{file}: {}", e.0)),
                            }
                        }
                        // Remote framework schemas (x-road.eu/xsd/xroad.xsd …)
                        // are never fetched; if the operations really use a
                        // type from one, the unresolved-type check reports it.
                        None if remote => {
                            tracing::debug!(location = ?loc, "remote schema not available locally — skipped")
                        }
                        None => self.problems.push(format!(
                            "{kind} schemaLocation=\"{loc}\" not found next to the WSDL"
                        )),
                    }
                }
                "element" => {
                    if let Some(name) = c.attr("name") {
                        let q = QName {
                            ns: ctx.tns.clone(),
                            local: name.to_string(),
                        };
                        let ty = element_type(c, &ctx);
                        self.schema
                            .elements_by_local
                            .entry(name.to_string())
                            .or_insert((q.clone(), ty.clone()));
                        self.schema.qualified_roots.insert(q.clone(), ctx.qualified);
                        self.schema.elements.insert(q, ty);
                    }
                }
                "complexType" => {
                    if let Some(name) = c.attr("name") {
                        self.schema
                            .types
                            .insert(name.to_string(), Arc::new(complex_type(c, &ctx)));
                    }
                }
                "simpleType" => {
                    if let Some(name) = c.attr("name") {
                        self.schema.simple_types.insert(name.to_string());
                    }
                }
                _ => {}
            }
        }
    }
}

fn element_type(el: &Element, ctx: &DocCtx) -> TypeRef {
    if let Some(r) = el.attr("ref") {
        return TypeRef::ElementRef(el.resolve_qname(r).1);
    }
    if let Some(t) = el.attr("type") {
        let (ns, local) = el.resolve_qname(t);
        return if ns.as_deref() == Some(XSD_NS) {
            TypeRef::Simple
        } else {
            TypeRef::Named(local)
        };
    }
    if let Some(ct) = el.elements().find(|e| is_xsd(e, "complexType")) {
        return TypeRef::Inline(Arc::new(complex_type(ct, ctx)));
    }
    TypeRef::Simple
}

fn complex_type(ct: &Element, ctx: &DocCtx) -> TypeDef {
    let mut td = TypeDef::default();
    collect(ct, false, &mut td, ctx);
    td
}

fn occurs_many(e: &Element) -> bool {
    match e.attr("maxOccurs") {
        Some("unbounded") => true,
        Some(n) => n.parse::<u64>().map(|v| v > 1).unwrap_or(false),
        None => false,
    }
}

fn collect(node: &Element, repeated: bool, td: &mut TypeDef, ctx: &DocCtx) {
    for c in node.elements() {
        if c.ns.as_deref() != Some(XSD_NS) {
            continue;
        }
        match c.local.as_str() {
            "sequence" | "choice" | "all" => collect(c, repeated || occurs_many(c), td, ctx),
            "complexContent" | "simpleContent" => collect(c, repeated, td, ctx),
            "extension" => {
                if let Some(b) = c.attr("base") {
                    let (ns, local) = c.resolve_qname(b);
                    if ns.as_deref() != Some(XSD_NS) {
                        td.base = Some(local);
                    }
                }
                collect(c, repeated, td, ctx);
            }
            // complexContent/restriction redefines the content model;
            // its particles are listed in full.
            "restriction" => collect(c, repeated, td, ctx),
            "element" => {
                let (name, ns) = if let Some(r) = c.attr("ref") {
                    let (ns, local) = c.resolve_qname(r);
                    (Some(local), ns)
                } else {
                    let qualified = match c.attr("form") {
                        Some("qualified") => true,
                        Some("unqualified") => false,
                        _ => ctx.qualified,
                    };
                    (
                        c.attr("name").map(String::from),
                        if qualified { ctx.tns.clone() } else { None },
                    )
                };
                if let Some(name) = name {
                    td.particles.push(Particle {
                        name,
                        repeated: repeated || occurs_many(c),
                        ty: element_type(c, ctx),
                        ns,
                    });
                }
            }
            _ => {}
        }
    }
}

/// Filesystem loader for local include/import targets, reusing the
/// audit-v1 H1 hardened resolver from the outbound pipeline
/// (filename-only, charset allow-list, no symlinks, canonical
/// containment in the WSDL directory).
pub fn fs_loader(wsdl_dir: &Path) -> impl Fn(&str) -> Option<(String, PathBuf, String)> + '_ {
    move |location: &str| {
        // Remote framework schemas (x-road.eu/xsd/xroad.xsd …) are never
        // fetched — only a local file with the same name is used.
        let file = location.rsplit(['/', '\\']).next()?.to_string();
        let xml = crate::wsdl::pipeline::resolve_local_schema(wsdl_dir, location)?;
        Some((file.clone(), wsdl_dir.join(&file), xml))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WSDL: &str = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
        xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" xmlns:xs="http://www.w3.org/2001/XMLSchema"
        xmlns:tns="urn:t" targetNamespace="urn:t">
      <wsdl:types><xs:schema targetNamespace="urn:t" elementFormDefault="qualified">
        <xs:complexType name="BaseT"><xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence></xs:complexType>
        <xs:complexType name="ExtT"><xs:complexContent><xs:extension base="tns:BaseT"><xs:sequence>
          <xs:element name="b" type="xs:string" maxOccurs="unbounded"/>
          <xs:choice><xs:element name="c1" type="xs:string"/><xs:element name="c2" type="xs:string"/></xs:choice>
        </xs:sequence></xs:extension></xs:complexContent></xs:complexType>
        <xs:element name="Req" type="tns:ExtT"/>
        <xs:element name="Res"><xs:complexType><xs:sequence><xs:element name="ok" type="xs:string"/></xs:sequence></xs:complexType></xs:element>
      </xs:schema></wsdl:types>
      <wsdl:message name="In"><wsdl:part name="p" element="tns:Req"/></wsdl:message>
      <wsdl:message name="Out"><wsdl:part name="p" element="tns:Res"/></wsdl:message>
      <wsdl:message name="EvIn"><wsdl:part name="p" element="tns:Res"/></wsdl:message>
      <wsdl:portType name="P">
        <wsdl:operation name="Do"><wsdl:input message="tns:In"/><wsdl:output message="tns:Out"/></wsdl:operation>
        <wsdl:operation name="Notify"><wsdl:input message="tns:EvIn"/></wsdl:operation>
      </wsdl:portType>
      <wsdl:binding name="B" type="tns:P"><soap:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/>
        <wsdl:operation name="Do"><soap:operation soapAction="urn:t/Do"/></wsdl:operation>
      </wsdl:binding>
      <wsdl:service name="S"><wsdl:port name="p" binding="tns:B"><soap:address location="http://TURVASERVER/x"/></wsdl:port></wsdl:service>
    </wsdl:definitions>"#;

    const TWO_BINDINGS: &str = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
        xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" xmlns:soap12="http://schemas.xmlsoap.org/wsdl/soap12/"
        xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:tns="urn:t" targetNamespace="urn:t">
      <wsdl:types><xs:schema targetNamespace="urn:t">
        <xs:element name="A" type="xs:string"/><xs:element name="B" type="xs:string"/>
      </xs:schema></wsdl:types>
      <wsdl:message name="MA"><wsdl:part name="p" element="tns:A"/></wsdl:message>
      <wsdl:message name="MB"><wsdl:part name="p" element="tns:B"/></wsdl:message>
      <wsdl:portType name="Other"><wsdl:operation name="OnlyInOther"><wsdl:input message="tns:MB"/></wsdl:operation></wsdl:portType>
      <wsdl:portType name="Main"><wsdl:operation name="Op"><wsdl:input message="tns:MA"/></wsdl:operation></wsdl:portType>
      <wsdl:binding name="B12" type="tns:Other"><soap12:binding transport="http://schemas.xmlsoap.org/soap/http"/></wsdl:binding>
      <wsdl:binding name="B11" type="tns:Main"><soap:binding transport="http://schemas.xmlsoap.org/soap/http"/>
        <wsdl:operation name="Op"><soap:operation soapAction="urn:t/Op"/></wsdl:operation></wsdl:binding>
      <wsdl:service name="S">
        <wsdl:port name="p12" binding="tns:B12"><soap12:address location="https://soap12.example/"/></wsdl:port>
        <wsdl:port name="p11" binding="tns:B11"><soap:address location="https://soap11.example/"/></wsdl:port>
      </wsdl:service>
    </wsdl:definitions>"#;

    #[test]
    fn address_and_operations_follow_the_soap11_binding() {
        let c = load(TWO_BINDINGS, &|_| None).unwrap();
        assert_eq!(c.address.as_deref(), Some("https://soap11.example/"));
        let ops: Vec<_> = c.operations.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            ops,
            vec!["Op"],
            "operations come from the SOAP 1.1 binding's portType only"
        );
        assert_eq!(c.operations[0].soap_action.as_deref(), Some("urn:t/Op"));
    }

    fn wsdl_with_schema(schema_body: &str) -> String {
        format!(
            r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:tns="urn:t" targetNamespace="urn:t">
            <wsdl:types><xs:schema targetNamespace="urn:t">{schema_body}</xs:schema></wsdl:types>
            <wsdl:message name="M"><wsdl:part name="p" element="tns:Req"/></wsdl:message>
            <wsdl:portType name="P"><wsdl:operation name="Op"><wsdl:input message="tns:M"/></wsdl:operation></wsdl:portType>
            <wsdl:binding name="B" type="tns:P"><soap:binding transport="http://schemas.xmlsoap.org/soap/http"/></wsdl:binding>
            </wsdl:definitions>"#
        )
    }

    #[test]
    fn missing_local_include_is_an_error() {
        let w = wsdl_with_schema(
            r#"<xs:include schemaLocation="types.xsd"/><xs:element name="Req" type="tns:ReqT"/>"#,
        );
        let e = load(&w, &|_| None).unwrap_err();
        assert!(e.contains("schemaLocation=\"types.xsd\" not found"), "{e}");
        assert!(e.contains("unresolved type ReqT"), "{e}");
    }

    #[test]
    fn unresolved_type_used_by_an_operation_is_an_error() {
        // Remote import (not fetched) is fine on its own …
        let ok = wsdl_with_schema(
            r#"<xs:import namespace="urn:x" schemaLocation="https://example.org/x.xsd"/>
               <xs:simpleType name="Code"><xs:restriction base="xs:string"/></xs:simpleType>
               <xs:element name="Req"><xs:complexType><xs:sequence><xs:element name="c" type="tns:Code"/></xs:sequence></xs:complexType></xs:element>"#,
        );
        assert!(load(&ok, &|_| None).is_ok(), "named simple types resolve");
        // … but a type the operation needs that nobody defines is not.
        let bad = wsdl_with_schema(
            r#"<xs:element name="Req"><xs:complexType><xs:sequence><xs:element name="x" type="tns:Missing"/><xs:element ref="tns:Gone"/></xs:sequence></xs:complexType></xs:element>"#,
        );
        let e = load(&bad, &|_| None).unwrap_err();
        assert!(
            e.contains("unresolved type Missing") && e.contains("unresolved element ref Gone"),
            "{e}"
        );
    }

    #[test]
    fn missing_type_deep_in_the_extension_chain_is_an_error() {
        let w = wsdl_with_schema(
            r#"<xs:complexType name="Middle"><xs:complexContent><xs:extension base="tns:MissingBase"><xs:sequence><xs:element name="m" type="xs:string"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>
               <xs:complexType name="HeaderT"><xs:complexContent><xs:extension base="tns:Middle"><xs:sequence><xs:element name="h" type="xs:string"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>
               <xs:element name="Req" type="tns:HeaderT"/>"#,
        );
        let e = load(&w, &|_| None).unwrap_err();
        assert!(e.contains("unresolved base type MissingBase"), "{e}");
        // Cycles terminate.
        let cyc = wsdl_with_schema(
            r#"<xs:complexType name="A"><xs:complexContent><xs:extension base="tns:B"/></xs:complexContent></xs:complexType>
               <xs:complexType name="B"><xs:complexContent><xs:extension base="tns:A"/></xs:complexContent></xs:complexType>
               <xs:element name="Req" type="tns:A"/>"#,
        );
        assert!(load(&cyc, &|_| None).is_ok());
    }

    #[test]
    fn unparsable_included_schema_is_an_error() {
        let w = wsdl_with_schema(
            r#"<xs:include schemaLocation="bad.xsd"/><xs:element name="Req" type="xs:string"/>"#,
        );
        let loader = |_: &str| {
            Some((
                "bad.xsd".to_string(),
                PathBuf::from("bad.xsd"),
                "<xs:schema".to_string(),
            ))
        };
        let e = load(&w, &loader).unwrap_err();
        assert!(e.contains("bad.xsd"), "{e}");
    }

    #[test]
    fn extracts_operations_and_schema_hints() {
        let c = load(WSDL, &|_| None).unwrap();
        assert!(c.soap11);
        assert_eq!(c.address.as_deref(), Some("http://TURVASERVER/x"));
        assert_eq!(c.operations.len(), 2);
        let op = &c.operations[0];
        assert_eq!(op.soap_action.as_deref(), Some("urn:t/Do"));
        assert_eq!(op.input.to_string(), "{urn:t}Req");
        assert_eq!(op.output.as_ref().unwrap().local, "Res");
        assert!(c.operations[1].output.is_none(), "Notify is one-way");

        let td = c
            .schema
            .resolve(c.schema.element(&op.input).unwrap())
            .unwrap();
        let names: Vec<_> = c
            .schema
            .particles(td)
            .iter()
            .map(|p| (p.name.as_str(), p.repeated))
            .collect();
        assert_eq!(
            names,
            vec![("a", false), ("b", true), ("c1", false), ("c2", false)]
        );
        assert!(c.schema.root_qualified(&op.input));
    }
}
