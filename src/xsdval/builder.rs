//! Builds the validation model from the XSD documents of a lane.
//!
//! Only the constructs of the supported subset are accepted. The build
//! starts at the message roots. A construct outside the subset that a
//! root reaches (also through type derivation, attribute groups or
//! includes) is a [`Problem`]. Constructs that no root reaches are not
//! read.

use super::model::{
    AttributeDecl, Builtin, ComplexType, ElementDecl, Model, Occurs, Particle, Problem, SimpleType,
    SrcLoc, Step, TypeDef, WhiteSpace,
};
use crate::inbound::contract::{QName, SchemaLoader, XSD_NS};
use crate::inbound::dom::{self, Element};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

const MAX_INCLUDE_DEPTH: usize = 16;
const ONE: Occurs = Occurs {
    min: 1,
    max: Some(1),
};

struct Doc {
    file: String,
    root: Element,
    tns: Option<String>,
    qualified: bool,
}

/// Position of a global component: document, index among the element
/// children of the `xs:schema` element.
type Loc = (usize, usize);

#[derive(Default)]
struct Index {
    docs: Vec<Doc>,
    elements: HashMap<QName, Loc>,
    complex: HashMap<QName, Loc>,
    simple: HashMap<QName, Loc>,
    attr_groups: HashMap<QName, Loc>,
}

fn is_xs(e: &Element) -> bool {
    e.ns.as_deref() == Some(XSD_NS)
}

struct Reader<'l> {
    loader: &'l SchemaLoader<'l>,
    idx: Index,
    visited: BTreeSet<String>,
    problems: Vec<Problem>,
}

impl Reader<'_> {
    fn problem(&mut self, file: &str, line: u32, construct: impl Into<String>) {
        let p = Problem {
            file: file.to_string(),
            component: String::new(),
            construct: construct.into(),
            line,
        };
        if !self.problems.contains(&p) {
            self.problems.push(p);
        }
    }

    /// `from` is the including file and the line of its `xs:include`.
    fn add_doc(
        &mut self,
        file: String,
        xml: &str,
        inherited_tns: Option<String>,
        depth: usize,
        from: Option<(&str, u32)>,
    ) {
        if !self.visited.insert(file.clone()) {
            return;
        }
        let root = match dom::parse(xml) {
            Ok(r) if r.local == "schema" && is_xs(&r) => r,
            Ok(_) => return self.problem(&file, 0, "document is not an xs:schema"),
            Err(_) => return self.problem(&file, 0, "document is not well-formed XML"),
        };
        let own_tns = root.attr("targetNamespace").map(String::from);
        if let Some((inc_file, inc_line)) = from {
            if own_tns != inherited_tns {
                let what = if own_tns.is_none() {
                    "chameleon include (the included schema has no targetNamespace)"
                } else {
                    "include with a different targetNamespace"
                };
                self.problem(inc_file, inc_line, what);
            }
        }
        let tns = own_tns.or(inherited_tns);
        let qualified = root.attr("elementFormDefault") == Some("qualified");
        if root.attr("attributeFormDefault") == Some("qualified") {
            self.problem(&file, root.line, "attributeFormDefault=qualified");
        }
        let mut includes: Vec<(String, u32)> = Vec::new();
        let d = self.idx.docs.len();
        for (i, c) in root.elements().enumerate() {
            if !is_xs(c) {
                continue;
            }
            match c.local.as_str() {
                "include" => match c.attr("schemaLocation") {
                    Some(l) => includes.push((l.to_string(), c.line)),
                    None => self.problem(&file, c.line, "xs:include without schemaLocation"),
                },
                "import" | "redefine" | "override" => {
                    self.problem(&file, c.line, format!("xs:{}", c.local));
                }
                kind @ ("element" | "complexType" | "simpleType" | "attributeGroup") => {
                    let Some(name) = c.attr("name") else { continue };
                    let q = QName {
                        ns: tns.clone(),
                        local: name.to_string(),
                    };
                    let map = match kind {
                        "element" => &mut self.idx.elements,
                        "complexType" => &mut self.idx.complex,
                        "simpleType" => &mut self.idx.simple,
                        _ => &mut self.idx.attr_groups,
                    };
                    match map.entry(q) {
                        std::collections::hash_map::Entry::Occupied(_) => {
                            self.problems.push(Problem {
                                file: file.clone(),
                                component: name.to_string(),
                                construct: format!("duplicate definition of {name}"),
                                line: c.line,
                            });
                        }
                        std::collections::hash_map::Entry::Vacant(v) => {
                            v.insert((d, i));
                        }
                    }
                }
                _ => {}
            }
        }
        self.idx.docs.push(Doc {
            file: file.clone(),
            root,
            tns: tns.clone(),
            qualified,
        });
        for (loc, line) in includes {
            if depth >= MAX_INCLUDE_DEPTH {
                self.problem(&file, line, "xs:include nested too deep");
                continue;
            }
            match (self.loader)(&loc) {
                Some((f, _path, text)) => {
                    self.add_doc(f, &text, tns.clone(), depth + 1, Some((&file, line)))
                }
                None => self.problem(&file, line, "xs:include: schema file not found"),
            }
        }
    }
}

struct Build<'a> {
    idx: &'a Index,
    problems: Vec<Problem>,
    cur: (String, String),
    line: u32,
    elem_cache: HashMap<QName, Arc<ElementDecl>>,
    complex_cache: HashMap<QName, Arc<ComplexType>>,
    simple_cache: HashMap<QName, Arc<SimpleType>>,
    group_cache: HashMap<QName, Vec<AttributeDecl>>,
    stack: Vec<String>,
}

pub(super) fn run(
    entries: &[(String, String)],
    loader: &SchemaLoader,
    roots: &[&str],
) -> Result<Model, Vec<Problem>> {
    let mut rd = Reader {
        loader,
        idx: Index::default(),
        visited: BTreeSet::new(),
        problems: Vec::new(),
    };
    for (file, xml) in entries {
        rd.add_doc(file.clone(), xml, None, 0, None);
    }
    let Reader { idx, problems, .. } = rd;
    let mut b = Build {
        idx: &idx,
        problems,
        cur: (String::new(), String::new()),
        line: 0,
        elem_cache: HashMap::new(),
        complex_cache: HashMap::new(),
        simple_cache: HashMap::new(),
        group_cache: HashMap::new(),
        stack: Vec::new(),
    };
    let mut model = Model {
        roots: BTreeMap::new(),
    };
    for root in roots {
        let found: Vec<&QName> = idx.elements.keys().filter(|q| q.local == *root).collect();
        match found.as_slice() {
            [] => b.problem(format!("root element {root} not found")),
            [q] => {
                if let Some(decl) = b.global_element(q) {
                    model.roots.insert((*root).to_string(), decl);
                }
            }
            _ => b.problem(format!("root element {root} is in more than one namespace")),
        }
    }
    if b.problems.is_empty() {
        Ok(model)
    } else {
        Err(b.problems)
    }
}

impl<'a> Build<'a> {
    fn problem(&mut self, construct: impl Into<String>) {
        let line = self.line;
        self.problem_at(line, construct);
    }

    fn problem_at(&mut self, line: u32, construct: impl Into<String>) {
        let p = Problem {
            file: self.cur.0.clone(),
            component: self.cur.1.clone(),
            construct: construct.into(),
            line,
        };
        if !self.problems.contains(&p) {
            self.problems.push(p);
        }
    }

    fn at(&self, (d, i): Loc) -> Option<&'a Element> {
        let idx: &'a Index = self.idx;
        idx.docs.get(d)?.root.elements().nth(i)
    }

    fn doc(&self, d: usize) -> &'a Doc {
        let idx: &'a Index = self.idx;
        &idx.docs[d]
    }

    /// Enters a global component: sets the context for problems and
    /// guards against cycles. Returns the previous context.
    fn enter(&mut self, d: usize, name: &str, key: String) -> Option<(String, String)> {
        if self.stack.contains(&key) {
            self.problem(format!("recursive definition of {name}"));
            return None;
        }
        self.stack.push(key);
        let next = (self.doc(d).file.clone(), name.to_string());
        Some(std::mem::replace(&mut self.cur, next))
    }

    fn leave(&mut self, prev: (String, String)) {
        self.stack.pop();
        self.cur = prev;
    }

    /// Checks the attributes of an XSD element. `allowed` are accepted;
    /// `false_ok` are accepted only with the value `false`.
    fn check_attrs(&mut self, el: &Element, allowed: &[&str], false_ok: &[&str]) {
        self.line = el.line;
        for a in &el.attrs {
            if a.ns.is_some() || a.local == "id" || allowed.contains(&a.local.as_str()) {
                continue;
            }
            if false_ok.contains(&a.local.as_str()) && a.value == "false" {
                continue;
            }
            self.problem(a.local.clone());
        }
    }

    fn occurs(&mut self, el: &Element) -> Occurs {
        self.line = el.line;
        let min = match el.attr("minOccurs") {
            None => 1,
            Some(v) => v.parse::<u32>().unwrap_or_else(|_| {
                self.problem("minOccurs");
                1
            }),
        };
        let max = match el.attr("maxOccurs") {
            None => Some(1),
            Some("unbounded") => None,
            Some(v) => Some(v.parse::<u32>().unwrap_or_else(|_| {
                self.problem("maxOccurs");
                1
            })),
        };
        if max == Some(0) {
            self.problem("maxOccurs=0");
        }
        if max.is_some_and(|m| m < min) {
            self.problem("maxOccurs below minOccurs");
        }
        Occurs { min, max }
    }

    fn qname(&self, el: &Element, value: &str) -> QName {
        let (ns, local) = el.resolve_qname(value);
        QName { ns, local }
    }

    // ---- elements ----

    fn global_element(&mut self, q: &QName) -> Option<Arc<ElementDecl>> {
        if let Some(d) = self.elem_cache.get(q) {
            return Some(d.clone());
        }
        let Some(loc) = self.idx.elements.get(q).copied() else {
            self.problem(format!("unresolved element reference {}", q.local));
            return None;
        };
        let el = self.at(loc)?;
        let prev = self.enter(loc.0, &q.local, format!("e:{q}"))?;
        let decl = self.element_decl(loc.0, el, true);
        self.leave(prev);
        if let Some(d) = &decl {
            self.elem_cache.insert(q.clone(), d.clone());
        }
        decl
    }

    fn element_decl(
        &mut self,
        d: usize,
        el: &'a Element,
        global: bool,
    ) -> Option<Arc<ElementDecl>> {
        self.check_attrs(
            el,
            &[
                "name",
                "type",
                "ref",
                "minOccurs",
                "maxOccurs",
                "block",
                "final",
            ],
            &["nillable", "abstract"],
        );
        if let Some(r) = el.attr("ref") {
            let q = self.qname(el, r);
            return self.global_element(&q);
        }
        let Some(name) = el.attr("name") else {
            self.problem("element without name or ref");
            return None;
        };
        let doc = self.doc(d);
        let ns = if global || doc.qualified {
            doc.tns.clone()
        } else {
            None
        };
        let mut ty: Option<TypeDef> = None;
        if let Some(t) = el.attr("type") {
            ty = self.type_ref(el, t);
        }
        for c in el.elements() {
            if !is_xs(c) {
                continue;
            }
            match c.local.as_str() {
                "annotation" => {}
                "complexType" if el.attr("type").is_none() => {
                    ty = Some(TypeDef::Complex(self.complex_def(d, c)));
                }
                "simpleType" if el.attr("type").is_none() => {
                    ty = self.simple_def(d, c).map(TypeDef::Simple);
                }
                other => self.problem_at(c.line, format!("xs:{other}")),
            }
        }
        if ty.is_none() && el.attr("type").is_none() {
            self.line = el.line;
            self.problem("element without a type");
        }
        Some(Arc::new(ElementDecl {
            name: QName {
                ns,
                local: name.to_string(),
            },
            ty: ty?,
            loc: SrcLoc {
                file: self.cur.0.clone(),
                line: el.line,
            },
        }))
    }

    fn type_ref(&mut self, el: &Element, value: &str) -> Option<TypeDef> {
        let q = self.qname(el, value);
        if q.ns.as_deref() == Some(XSD_NS) {
            return self.builtin(&q.local).map(TypeDef::Simple);
        }
        if self.idx.complex.contains_key(&q) {
            return self.complex_named(&q).map(TypeDef::Complex);
        }
        if self.idx.simple.contains_key(&q) {
            return self.simple_named(&q).map(TypeDef::Simple);
        }
        self.problem(format!("unresolved type reference {}", q.local));
        None
    }

    // ---- complex types ----

    fn complex_named(&mut self, q: &QName) -> Option<Arc<ComplexType>> {
        if let Some(c) = self.complex_cache.get(q) {
            return Some(c.clone());
        }
        let loc = *self.idx.complex.get(q)?;
        let el = self.at(loc)?;
        let prev = self.enter(loc.0, &q.local, format!("c:{q}"))?;
        let ct = self.complex_def(loc.0, el);
        self.leave(prev);
        self.complex_cache.insert(q.clone(), ct.clone());
        Some(ct)
    }

    fn complex_def(&mut self, d: usize, el: &'a Element) -> Arc<ComplexType> {
        self.line = el.line;
        self.check_attrs(el, &["name", "block", "final"], &["mixed", "abstract"]);
        let mut ct = ComplexType::default();
        for c in el.elements().filter(|c| is_xs(c)) {
            match c.local.as_str() {
                "annotation" => {}
                "sequence" | "choice" => {
                    if ct.content.is_some() {
                        self.problem("more than one content model");
                    }
                    ct.content = self.model_group(d, c);
                }
                "attribute" => {
                    if let Some(a) = self.attribute(d, c) {
                        ct.attributes.push(a);
                    }
                }
                "attributeGroup" => {
                    let g = self.group_ref(c);
                    ct.attributes.extend(g);
                }
                "complexContent" => self.complex_content(d, c, &mut ct),
                other => self.problem_at(c.line, format!("xs:{other}")),
            }
        }
        if ct
            .content
            .as_ref()
            .is_some_and(|c| !super::upa::is_deterministic(c))
        {
            self.problem("ambiguous content model");
        }
        Arc::new(ct)
    }

    fn complex_content(&mut self, d: usize, el: &'a Element, ct: &mut ComplexType) {
        self.line = el.line;
        self.check_attrs(el, &[], &["mixed"]);
        for c in el.elements().filter(|c| is_xs(c)) {
            match c.local.as_str() {
                "annotation" => {}
                "extension" => self.extension(d, c, ct),
                "restriction" => self.problem("xs:restriction in complexContent"),
                other => self.problem_at(c.line, format!("xs:{other}")),
            }
        }
    }

    fn extension(&mut self, d: usize, el: &'a Element, ct: &mut ComplexType) {
        self.line = el.line;
        self.check_attrs(el, &["base"], &[]);
        let mut base: Option<Arc<ComplexType>> = None;
        match el.attr("base") {
            None => self.problem("extension without base"),
            Some(b) => {
                let q = self.qname(el, b);
                if q.ns.as_deref() == Some(XSD_NS) {
                    self.problem(format!("xs:{}", q.local));
                } else if self.idx.complex.contains_key(&q) {
                    base = self.complex_named(&q);
                } else if self.idx.simple.contains_key(&q) {
                    self.problem("extension of a simple type");
                } else {
                    self.problem(format!("unresolved type reference {}", q.local));
                }
            }
        }
        let mut own: Option<Particle> = None;
        let mut attrs: Vec<AttributeDecl> = Vec::new();
        for c in el.elements().filter(|c| is_xs(c)) {
            match c.local.as_str() {
                "annotation" => {}
                "sequence" | "choice" => {
                    if own.is_some() {
                        self.problem("more than one content model");
                    }
                    own = self.model_group(d, c);
                }
                "attribute" => {
                    if let Some(a) = self.attribute(d, c) {
                        attrs.push(a);
                    }
                }
                "attributeGroup" => {
                    let g = self.group_ref(c);
                    attrs.extend(g);
                }
                other => self.problem_at(c.line, format!("xs:{other}")),
            }
        }
        let (base_content, mut all_attrs) = match &base {
            Some(b) => (b.content.clone(), b.attributes.clone()),
            None => (None, Vec::new()),
        };
        all_attrs.extend(attrs);
        ct.attributes.extend(all_attrs);
        ct.content = match (base_content, own) {
            (None, x) | (x, None) => x,
            (Some(b), Some(o)) => Some(Particle::Sequence {
                items: vec![b, o],
                occurs: ONE,
            }),
        };
    }

    fn model_group(&mut self, d: usize, el: &'a Element) -> Option<Particle> {
        self.line = el.line;
        self.check_attrs(el, &["minOccurs", "maxOccurs"], &[]);
        let occurs = self.occurs(el);
        let mut items = Vec::new();
        for c in el.elements().filter(|c| is_xs(c)) {
            match c.local.as_str() {
                "annotation" => {}
                "element" => {
                    let o = self.occurs(c);
                    if let Some(decl) = self.element_decl(d, c, false) {
                        items.push(Particle::Element { decl, occurs: o });
                    }
                }
                "sequence" | "choice" => items.extend(self.model_group(d, c)),
                other => self.problem_at(c.line, format!("xs:{other}")),
            }
        }
        Some(if el.local == "choice" {
            Particle::Choice { items, occurs }
        } else {
            Particle::Sequence { items, occurs }
        })
    }

    // ---- attributes ----

    fn attribute(&mut self, d: usize, el: &'a Element) -> Option<AttributeDecl> {
        self.line = el.line;
        self.check_attrs(el, &["name", "type", "use", "fixed"], &[]);
        let Some(name) = el.attr("name") else {
            self.problem("attribute without name");
            return None;
        };
        let required = match el.attr("use") {
            None | Some("optional") => false,
            Some("required") => true,
            Some(_) => {
                self.problem("use=prohibited");
                false
            }
        };
        let mut ty: Option<Arc<SimpleType>> = None;
        if let Some(t) = el.attr("type") {
            ty = self.simple_ref(el, t);
        }
        for c in el.elements().filter(|c| is_xs(c)) {
            match c.local.as_str() {
                "annotation" => {}
                "simpleType" if el.attr("type").is_none() => ty = self.simple_def(d, c),
                other => self.problem_at(c.line, format!("xs:{other}")),
            }
        }
        if ty.is_none() && el.attr("type").is_none() {
            self.problem("attribute without a type");
        }
        Some(AttributeDecl {
            name: name.to_string(),
            ty: ty?,
            required,
            fixed: el.attr("fixed").map(String::from),
            loc: SrcLoc {
                file: self.cur.0.clone(),
                line: el.line,
            },
        })
    }

    fn group_ref(&mut self, el: &'a Element) -> Vec<AttributeDecl> {
        self.check_attrs(el, &["ref"], &[]);
        let Some(r) = el.attr("ref") else {
            self.problem("attributeGroup without ref");
            return Vec::new();
        };
        let q = self.qname(el, r);
        if let Some(g) = self.group_cache.get(&q) {
            return g.clone();
        }
        let Some(loc) = self.idx.attr_groups.get(&q).copied() else {
            self.problem(format!("unresolved attributeGroup reference {}", q.local));
            return Vec::new();
        };
        let Some(def) = self.at(loc) else {
            return Vec::new();
        };
        let Some(prev) = self.enter(loc.0, &q.local, format!("g:{q}")) else {
            return Vec::new();
        };
        self.check_attrs(def, &["name"], &[]);
        let mut out = Vec::new();
        for c in def.elements().filter(|c| is_xs(c)) {
            match c.local.as_str() {
                "annotation" => {}
                "attribute" => out.extend(self.attribute(loc.0, c)),
                "attributeGroup" => out.extend(self.group_ref(c)),
                other => self.problem_at(c.line, format!("xs:{other}")),
            }
        }
        self.leave(prev);
        self.group_cache.insert(q, out.clone());
        out
    }

    // ---- simple types ----

    fn builtin(&mut self, local: &str) -> Option<Arc<SimpleType>> {
        let (base, ws) = match local {
            "string" => (Builtin::String, WhiteSpace::Preserve),
            "token" => (Builtin::Token, WhiteSpace::Collapse),
            "boolean" => (Builtin::Boolean, WhiteSpace::Collapse),
            "nonNegativeInteger" => (Builtin::NonNegativeInteger, WhiteSpace::Collapse),
            "positiveInteger" => (Builtin::PositiveInteger, WhiteSpace::Collapse),
            "date" => (Builtin::Date, WhiteSpace::Collapse),
            "dateTime" => (Builtin::DateTime, WhiteSpace::Collapse),
            _ => {
                self.problem(format!("xs:{local}"));
                return None;
            }
        };
        Some(Arc::new(SimpleType {
            base,
            whitespace: ws,
            steps: Vec::new(),
        }))
    }

    fn simple_ref(&mut self, el: &Element, value: &str) -> Option<Arc<SimpleType>> {
        let q = self.qname(el, value);
        if q.ns.as_deref() == Some(XSD_NS) {
            return self.builtin(&q.local);
        }
        if self.idx.simple.contains_key(&q) {
            return self.simple_named(&q);
        }
        if self.idx.complex.contains_key(&q) {
            self.problem("complex type used as a simple type");
        } else {
            self.problem(format!("unresolved type reference {}", q.local));
        }
        None
    }

    fn simple_named(&mut self, q: &QName) -> Option<Arc<SimpleType>> {
        if let Some(s) = self.simple_cache.get(q) {
            return Some(s.clone());
        }
        let loc = *self.idx.simple.get(q)?;
        let el = self.at(loc)?;
        let prev = self.enter(loc.0, &q.local, format!("s:{q}"))?;
        let st = self.simple_def(loc.0, el);
        self.leave(prev);
        if let Some(s) = &st {
            self.simple_cache.insert(q.clone(), s.clone());
        }
        st
    }

    fn simple_def(&mut self, d: usize, el: &'a Element) -> Option<Arc<SimpleType>> {
        self.line = el.line;
        self.check_attrs(el, &["name", "final"], &[]);
        let mut out = None;
        for c in el.elements().filter(|c| is_xs(c)) {
            match c.local.as_str() {
                "annotation" => {}
                "restriction" => out = self.restriction(d, c),
                other => self.problem_at(c.line, format!("xs:{other}")),
            }
        }
        if out.is_none() {
            self.problem("simpleType without a restriction");
        }
        out
    }

    fn restriction(&mut self, d: usize, el: &'a Element) -> Option<Arc<SimpleType>> {
        self.line = el.line;
        self.check_attrs(el, &["base"], &[]);
        let base = self.restriction_base(d, el);
        let mut derived = (*base?).clone();
        let mut step = Step {
            loc: SrcLoc {
                file: self.cur.0.clone(),
                line: el.line,
            },
            ..Step::default()
        };
        for c in el.elements().filter(|c| is_xs(c)) {
            if matches!(c.local.as_str(), "annotation" | "simpleType") {
                continue;
            }
            self.check_attrs(c, &["value", "fixed"], &[]);
            let Some(v) = c.attr("value") else {
                self.problem(format!("xs:{} without value", c.local));
                continue;
            };
            self.apply_facet(c, v, &mut derived, &mut step);
        }
        derived.steps.push(step);
        Some(Arc::new(derived))
    }

    /// The base type of a restriction: `base=` or an inline `xs:simpleType`.
    fn restriction_base(&mut self, d: usize, el: &'a Element) -> Option<Arc<SimpleType>> {
        match el.attr("base") {
            Some(b) => self.simple_ref(el, b),
            None => {
                let inner = el.elements().find(|c| is_xs(c) && c.local == "simpleType");
                match inner {
                    Some(inner) => self.simple_def(d, inner),
                    None => {
                        self.problem("restriction without a base");
                        None
                    }
                }
            }
        }
    }

    /// One facet of a restriction. `derived` and `step` collect the result.
    fn apply_facet(&mut self, c: &Element, v: &str, derived: &mut SimpleType, step: &mut Step) {
        match c.local.as_str() {
            "enumeration" | "length" | "minLength" | "maxLength"
                if !matches!(derived.base, Builtin::String | Builtin::Token) =>
            {
                // Value-space facets of the other types are not supported.
                self.problem_at(c.line, format!("xs:{} on this base type", c.local));
            }
            "enumeration" => step
                .enumeration
                .get_or_insert_with(Vec::new)
                .push(v.to_string()),
            "pattern" => match super::pattern::compile(v) {
                Ok(re) => step.patterns.push(re),
                Err(e) => self.problem_at(c.line, format!("pattern: {e}")),
            },
            "whiteSpace" => self.facet_white_space(c, v, derived),
            "length" | "minLength" | "maxLength" => match v.parse::<usize>() {
                Ok(n) => match c.local.as_str() {
                    "length" => step.length = Some(n),
                    "minLength" => step.min_length = Some(n),
                    _ => step.max_length = Some(n),
                },
                Err(_) => self.problem(format!("xs:{} value", c.local)),
            },
            "minInclusive" => self.facet_min_inclusive(c, v, derived.base, step),
            other => self.problem_at(c.line, format!("xs:{other}")),
        }
    }

    fn facet_white_space(&mut self, c: &Element, v: &str, derived: &mut SimpleType) {
        let new = match v {
            "preserve" => Some(WhiteSpace::Preserve),
            "replace" => Some(WhiteSpace::Replace),
            "collapse" => Some(WhiteSpace::Collapse),
            _ => None,
        };
        match new {
            None => self.problem_at(c.line, "whiteSpace value"),
            // XSD: a derived type may only make white space handling stricter.
            Some(w) if ws_rank(w) < ws_rank(derived.whitespace) => {
                self.problem_at(c.line, "whiteSpace looser than the base type");
            }
            Some(w) => derived.whitespace = w,
        }
    }

    fn facet_min_inclusive(&mut self, c: &Element, v: &str, base: Builtin, step: &mut Step) {
        let ordered = matches!(
            base,
            Builtin::NonNegativeInteger
                | Builtin::PositiveInteger
                | Builtin::Date
                | Builtin::DateTime
        );
        if !ordered {
            self.problem_at(c.line, "xs:minInclusive on this base type");
        } else if super::validate::lexical_ok(base, v.trim()) {
            // The bound must be a value of the base type.
            step.min_inclusive = Some(v.trim().to_string());
        } else {
            self.problem_at(c.line, "xs:minInclusive value does not fit the base type");
        }
    }
}

fn ws_rank(w: WhiteSpace) -> u8 {
    match w {
        WhiteSpace::Preserve => 0,
        WhiteSpace::Replace => 1,
        WhiteSpace::Collapse => 2,
    }
}
