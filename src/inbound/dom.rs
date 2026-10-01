//! Minimal namespace-aware XML DOM for the inbound lane.
//!
//! Used for three things: parsing inbound SOAP envelopes, reading
//! operator-supplied WSDL/XSD contracts, and re-serialising the
//! inbound SOAP `<Header>` children so the response can echo them
//! (X-Road SOAP profile requires the provider to echo the header).
//!
//! Security posture mirrors `translate::xml_to_json`: DOCTYPE is
//! rejected outright (no DTD, no custom entities → no XXE / billion
//! laughs), only the five predefined entities and character
//! references are resolved, nesting depth is capped.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::sync::Arc;

/// Hard cap on element nesting. Real SOAP/WSDL documents stay well
/// under 30; the cap exists to bound recursion in the codec.
const MAX_DEPTH: usize = 128;

/// In-scope namespace bindings: `(prefix, uri)`, `""` = default ns.
/// Shared via `Arc` so unchanged scopes cost one pointer per element.
pub type Scope = Arc<Vec<(String, String)>>;

#[derive(Debug, Clone)]
pub struct Element {
    /// Resolved namespace URI (`None` = no namespace).
    pub ns: Option<String>,
    pub local: String,
    /// Prefix as written in the source (`None` = unprefixed).
    pub prefix: Option<String>,
    /// Non-`xmlns` attributes.
    pub attrs: Vec<Attr>,
    pub children: Vec<Node>,
    /// Namespace bindings in scope at this element — needed to
    /// resolve QName-valued attributes (`element="tns:Foo"`).
    pub scope: Scope,
    /// `xmlns` / `xmlns:p` declarations written on this element itself,
    /// in source order — re-emitted by `serialize` so declarations that
    /// are only used inside attribute values (QNames) are not lost.
    pub decls: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct Attr {
    pub ns: Option<String>,
    pub local: String,
    pub prefix: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone)]
pub enum Node {
    Elem(Element),
    Text(String),
}

impl Element {
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Elem(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    pub fn child(&self, local: &str) -> Option<&Element> {
        self.elements().find(|e| e.local == local)
    }

    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.local == local && a.ns.is_none())
            .map(|a| a.value.as_str())
    }

    pub fn attr_ns(&self, ns: &str, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.local == local && a.ns.as_deref() == Some(ns))
            .map(|a| a.value.as_str())
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for n in &self.children {
            if let Node::Text(t) = n {
                s.push_str(t);
            }
        }
        s
    }

    pub fn has_element_children(&self) -> bool {
        self.elements().next().is_some()
    }

    /// Resolve a QName-valued attribute (`tns:Foo`) against this
    /// element's scope → `(namespace, local)`.
    pub fn resolve_qname(&self, qname: &str) -> (Option<String>, String) {
        let (prefix, local) = match qname.split_once(':') {
            Some((p, l)) => (p, l),
            None => ("", qname),
        };
        (lookup(&self.scope, prefix), local.to_string())
    }
}

fn lookup(scope: &[(String, String)], prefix: &str) -> Option<String> {
    if prefix == "xml" {
        return Some("http://www.w3.org/XML/1998/namespace".into());
    }
    scope
        .iter()
        .rev()
        .find(|(p, _)| p == prefix)
        .map(|(_, u)| u.clone())
        .filter(|u| !u.is_empty())
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct DomError(pub String);

fn err(msg: impl Into<String>) -> DomError {
    DomError(msg.into())
}

/// Parse a complete document and return its root element.
pub fn parse(xml: &str) -> Result<Element, DomError> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let empty_scope: Scope = Arc::new(Vec::new());
    loop {
        let ev = reader
            .read_event()
            .map_err(|e| err(format!("XML parse error: {e}")))?;
        match ev {
            Event::Start(e) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(err(format!("XML nesting deeper than {MAX_DEPTH}")));
                }
                let parent_scope = stack
                    .last()
                    .map(|p| p.scope.clone())
                    .unwrap_or_else(|| empty_scope.clone());
                stack.push(open_element(&e, parent_scope)?);
            }
            Event::Empty(e) => {
                let parent_scope = stack
                    .last()
                    .map(|p| p.scope.clone())
                    .unwrap_or_else(|| empty_scope.clone());
                let el = open_element(&e, parent_scope)?;
                attach(&mut stack, &mut root, el)?;
            }
            Event::End(_) => {
                let el = stack.pop().ok_or_else(|| err("unbalanced end tag"))?;
                attach(&mut stack, &mut root, el)?;
            }
            Event::Text(t) => {
                let raw = t.decode().map_err(|e| err(format!("text decode: {e}")))?;
                let text = quick_xml::escape::unescape(&raw)
                    .map_err(|e| err(format!("text unescape: {e}")))?;
                push_text(&mut stack, &text);
            }
            Event::CData(t) => {
                let raw = t.decode().map_err(|e| err(format!("cdata decode: {e}")))?;
                push_text(&mut stack, &raw);
            }
            Event::GeneralRef(g) => {
                let name = String::from_utf8_lossy(g.as_ref()).into_owned();
                let ch = resolve_entity(&name).ok_or_else(|| {
                    err(format!(
                        "unresolved entity &{name}; — custom entities are not supported (XXE risk)"
                    ))
                })?;
                push_text(&mut stack, ch.encode_utf8(&mut [0u8; 4]));
            }
            Event::DocType(_) => {
                return Err(err("DOCTYPE is not allowed (XXE / entity-expansion risk)"));
            }
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
            Event::Eof => break,
        }
    }
    if !stack.is_empty() {
        return Err(err("unexpected end of document (unclosed elements)"));
    }
    root.ok_or_else(|| err("document has no root element"))
}

fn attach(stack: &mut [Element], root: &mut Option<Element>, el: Element) -> Result<(), DomError> {
    match stack.last_mut() {
        Some(parent) => parent.children.push(Node::Elem(el)),
        None => {
            if root.is_some() {
                return Err(err("more than one root element"));
            }
            *root = Some(el);
        }
    }
    Ok(())
}

fn push_text(stack: &mut [Element], text: &str) {
    // Text outside the root (whitespace between prolog and root) is ignored.
    if let Some(parent) = stack.last_mut() {
        if let Some(Node::Text(prev)) = parent.children.last_mut() {
            prev.push_str(text);
        } else {
            parent.children.push(Node::Text(text.to_string()));
        }
    }
}

/// Characters XML 1.0 forbids even when escaped (§2.2 `Char`): C0
/// controls other than TAB/LF/CR, and U+FFFE / U+FFFF.
pub fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || ((c as u32) >= 0x20 && c != '\u{FFFE}' && c != '\u{FFFF}')
}

/// Replace characters XML 1.0 cannot carry with U+FFFD, so text from a
/// backend (e.g. a database value with a stray control byte) never
/// produces a document the SOAP peer cannot parse.
pub fn xml_safe(s: &str) -> std::borrow::Cow<'_, str> {
    if s.chars().all(is_xml_char) {
        std::borrow::Cow::Borrowed(s)
    } else {
        std::borrow::Cow::Owned(
            s.chars()
                .map(|c| if is_xml_char(c) { c } else { '\u{FFFD}' })
                .collect(),
        )
    }
}

fn resolve_entity(name: &str) -> Option<char> {
    match name {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "apos" => Some('\''),
        "quot" => Some('"'),
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse::<u32>().ok()?,
            };
            // `&#1;` etc. are not well-formed XML 1.0.
            char::from_u32(code).filter(|c| is_xml_char(*c))
        }
    }
}

fn split_qname(raw: &str) -> (Option<String>, String) {
    match raw.split_once(':') {
        Some((p, l)) => (Some(p.to_string()), l.to_string()),
        None => (None, raw.to_string()),
    }
}

fn open_element(e: &BytesStart, parent_scope: Scope) -> Result<Element, DomError> {
    let mut decls: Vec<(String, String)> = Vec::new();
    let mut raw_attrs: Vec<(String, String)> = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|e| err(format!("attribute parse error: {e}")))?;
        let key = std::str::from_utf8(a.key.as_ref())
            .map_err(|_| err("attribute name is not UTF-8"))?
            .to_string();
        let raw = std::str::from_utf8(&a.value).map_err(|_| err("attribute value is not UTF-8"))?;
        let value = quick_xml::escape::unescape(raw)
            .map_err(|e| err(format!("attribute unescape: {e}")))?
            .into_owned();
        if key == "xmlns" {
            decls.push((String::new(), value));
        } else if let Some(p) = key.strip_prefix("xmlns:") {
            decls.push((p.to_string(), value));
        } else {
            raw_attrs.push((key, value));
        }
    }
    let scope = if decls.is_empty() {
        parent_scope
    } else {
        let mut s = (*parent_scope).clone();
        s.extend(decls.iter().cloned());
        Arc::new(s)
    };
    let name = std::str::from_utf8(e.name().as_ref())
        .map_err(|_| err("element name is not UTF-8"))?
        .to_string();
    let (prefix, local) = split_qname(&name);
    let ns = lookup(&scope, prefix.as_deref().unwrap_or(""));
    if prefix.is_some() && ns.is_none() {
        return Err(err(format!("undeclared namespace prefix in <{name}>")));
    }
    let attrs = raw_attrs
        .into_iter()
        .map(|(key, value)| {
            let (prefix, local) = split_qname(&key);
            // Unprefixed attributes are in no namespace (XML Namespaces §6.2).
            let ns = prefix.as_deref().and_then(|p| lookup(&scope, p));
            Attr {
                ns,
                local,
                prefix,
                value,
            }
        })
        .collect();
    Ok(Element {
        ns,
        local,
        prefix,
        attrs,
        children: Vec::new(),
        scope,
        decls,
    })
}

/// Serialise `el` as a standalone fragment, emitting exactly the
/// namespace declarations it needs (prefixes preserved as written).
pub fn serialize(el: &Element) -> String {
    let mut out = String::new();
    let mut scope: Vec<(String, String)> = Vec::new();
    write_el(el, &mut out, &mut scope);
    out
}

/// `p:local` with NCName-ish parts — candidate QName in a value.
fn qname_like(v: &str) -> Option<(&str, &str)> {
    let (p, l) = v.split_once(':')?;
    let ok = |s: &str| {
        let mut c = s.chars();
        matches!(c.next(), Some(f) if f.is_alphabetic() || f == '_')
            && c.all(|x| x.is_alphanumeric() || matches!(x, '_' | '-' | '.'))
    };
    (ok(p) && ok(l)).then_some((p, l))
}

/// Emit `xmlns[:prefix]` unless `prefix` is already bound to `uri`.
fn declare_ns(prefix: &str, uri: &str, out: &mut String, scope: &mut Vec<(String, String)>) {
    let current = scope
        .iter()
        .rev()
        .find(|(p, _)| p == prefix)
        .map(|(_, u)| u.as_str());
    if current.unwrap_or("") != uri {
        if prefix.is_empty() {
            out.push_str(&format!(" xmlns=\"{}\"", quick_xml::escape::escape(uri)));
        } else {
            out.push_str(&format!(
                " xmlns:{prefix}=\"{}\"",
                quick_xml::escape::escape(uri)
            ));
        }
        scope.push((prefix.to_string(), uri.to_string()));
    }
}

fn write_el(el: &Element, out: &mut String, scope: &mut Vec<(String, String)>) {
    let mark = scope.len();
    let qname = match &el.prefix {
        Some(p) => format!("{p}:{}", el.local),
        None => el.local.clone(),
    };
    out.push('<');
    out.push_str(&qname);
    // 1. The element's own declarations, as written (faithful copy, and
    //    covers prefixes used only inside QName-valued attributes).
    for (p, uri) in &el.decls {
        declare_ns(p, uri, out, scope);
    }
    // 2. Prefixes this element's name / attribute names need.
    declare_ns(
        el.prefix.as_deref().unwrap_or(""),
        el.ns.as_deref().unwrap_or(""),
        out,
        scope,
    );
    for a in &el.attrs {
        if let (Some(p), Some(ns)) = (&a.prefix, &a.ns) {
            declare_ns(p, ns, out, scope);
        }
    }
    // 3. Prefixes referenced by QName-looking attribute values or text
    //    (`element="tns:X"`, `xsi:type="xrd:T"`) bound by an ancestor
    //    outside the serialised fragment.
    let values = el
        .attrs
        .iter()
        .map(|a| a.value.as_str())
        .chain(el.children.iter().filter_map(|c| match c {
            Node::Text(t) => Some(t.as_str()),
            Node::Elem(_) => None,
        }));
    for v in values {
        if let Some((p, _)) = qname_like(v.trim()) {
            if let Some(uri) = el
                .scope
                .iter()
                .rev()
                .find(|(sp, _)| sp == p)
                .map(|(_, u)| u)
            {
                declare_ns(p, uri, out, scope);
            }
        }
    }
    for a in &el.attrs {
        let key = match &a.prefix {
            Some(p) => format!("{p}:{}", a.local),
            None => a.local.clone(),
        };
        out.push_str(&format!(
            " {key}=\"{}\"",
            quick_xml::escape::escape(xml_safe(&a.value).as_ref())
        ));
    }
    if el.children.is_empty() {
        out.push_str("/>");
    } else {
        out.push('>');
        for c in &el.children {
            match c {
                Node::Elem(child) => write_el(child, out, scope),
                Node::Text(t) => out.push_str(&quick_xml::escape::escape(xml_safe(t).as_ref())),
            }
        }
        out.push_str(&format!("</{qname}>"));
    }
    scope.truncate(mark);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_namespaces_and_attributes() {
        let root = parse(
            r#"<?xml version="1.0"?><e:Env xmlns:e="urn:e" xmlns="urn:d"><Item id:t="X" plain="1" xmlns:id="urn:id">a &amp; b &#228;</Item></e:Env>"#,
        )
        .unwrap();
        assert_eq!(root.ns.as_deref(), Some("urn:e"));
        let item = root.child("Item").unwrap();
        assert_eq!(item.ns.as_deref(), Some("urn:d"));
        assert_eq!(item.attr("plain"), Some("1"));
        assert_eq!(item.attr_ns("urn:id", "t"), Some("X"));
        assert_eq!(item.text(), "a & b ä");
    }

    #[test]
    fn rejects_doctype() {
        let e = parse(r#"<!DOCTYPE x [<!ENTITY a "b">]><x>&a;</x>"#).unwrap_err();
        assert!(e.0.contains("DOCTYPE"));
    }

    #[test]
    fn rejects_char_refs_illegal_in_xml_1_0() {
        assert!(parse("<x>&#1;</x>").is_err());
        assert!(parse("<x>&#xFFFE;</x>").is_err());
        assert!(parse("<x>&#9;&#10;&#13;</x>").is_ok());
    }

    #[test]
    fn xml_safe_replaces_only_forbidden_chars() {
        assert_eq!(xml_safe("a\u{1}b\tc\u{FFFF}"), "a\u{FFFD}b\tc\u{FFFD}");
        assert!(matches!(
            xml_safe("plain ÄÖ"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn rejects_undeclared_prefix() {
        assert!(parse("<p:x/>").is_err());
    }

    #[test]
    fn serialize_standalone_fragment_declares_namespaces() {
        let root = parse(
            r#"<S:Envelope xmlns:S="urn:s" xmlns:xrd="urn:xrd" xmlns:id="urn:id"><S:Header><xrd:client id:objectType="SUBSYSTEM"><id:memberCode>1</id:memberCode></xrd:client></S:Header></S:Envelope>"#,
        )
        .unwrap();
        let client = root.child("Header").unwrap().child("client").unwrap();
        let s = serialize(client);
        assert_eq!(
            s,
            r#"<xrd:client xmlns:xrd="urn:xrd" xmlns:id="urn:id" id:objectType="SUBSYSTEM"><id:memberCode>1</id:memberCode></xrd:client>"#
        );
        // Round-trips through the parser to the same namespaces.
        let back = parse(&s).unwrap();
        assert_eq!(back.ns.as_deref(), Some("urn:xrd"));
        assert_eq!(
            back.child("memberCode").unwrap().ns.as_deref(),
            Some("urn:id")
        );
    }

    #[test]
    fn serialize_keeps_prefixes_used_only_in_attribute_values() {
        let src = r#"<w:definitions xmlns:w="urn:w" xmlns:tns="urn:t" xmlns:xs="urn:xs"><w:message name="M"><w:part element="tns:Q"/></w:message><w:x xsi:type="xs:string" xmlns:xsi="urn:xsi">tns:Text</w:x></w:definitions>"#;
        let root = parse(src).unwrap();
        let out = serialize(&root);
        let back = parse(&out).unwrap();
        let part = back.child("message").unwrap().child("part").unwrap();
        assert_eq!(
            part.resolve_qname(part.attr("element").unwrap())
                .0
                .as_deref(),
            Some("urn:t")
        );
        // A fragment serialised alone still declares what its values use.
        let frag = serialize(root.child("message").unwrap());
        assert!(frag.contains(r#"xmlns:tns="urn:t""#), "{frag}");
        let x = serialize(root.child("x").unwrap());
        assert!(
            x.contains(r#"xmlns:xs="urn:xs""#) && x.contains(r#"xmlns:tns="urn:t""#),
            "{x}"
        );
    }

    #[test]
    fn serialize_unqualified_child_under_default_ns_resets_it() {
        let root = parse(r#"<a xmlns="urn:a"><b xmlns=""><c/></b></a>"#).unwrap();
        let s = serialize(&root);
        assert_eq!(s, r#"<a xmlns="urn:a"><b xmlns=""><c/></b></a>"#);
    }
}
