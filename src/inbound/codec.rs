//! Schema-guided XML ⇄ JSON codec for the inbound lane.
//!
//! JSON convention (stable, documented in `book/src/soap-lanes.md`):
//!
//! * element with only text → JSON string (never coerced — X-Road
//!   identifiers such as `"007"` must survive byte-exact);
//! * attributes → `"@name"` keys (local name, namespace dropped);
//! * text next to attributes/children → `"#text"`;
//! * repeated element → JSON array. When the schema says the element
//!   repeats (`maxOccurs > 1`) it is an array even with one item, so
//!   backends never have to branch on "object or array";
//! * namespace prefixes are dropped (senders choose prefixes freely).
//!
//! JSON → XML reverses it. Child order follows the XSD sequence
//! (incl. `extension` base types); keys unknown to the schema follow
//! in map order. `null` omits the element/attribute.

use super::contract::{QName, Schema, TypeDef};
use super::dom::{xml_safe, Element, Node};
use quick_xml::escape::escape;
use serde_json::{Map, Value};

pub fn element_to_json(el: &Element, ty: Option<&TypeDef>, schema: &Schema) -> Value {
    let complex_by_schema = ty.is_some();
    let has_children = el.has_element_children();
    let attrs: Vec<_> = el.attrs.iter().filter(|a| !is_xsi_nil(a)).collect();
    let nil = el.attrs.iter().any(|a| is_xsi_nil(a) && a.value == "true");
    if nil && !has_children {
        return Value::Null;
    }
    if attrs.is_empty() && !has_children {
        return if complex_by_schema && el.text().trim().is_empty() {
            Value::Object(Map::new())
        } else {
            Value::String(el.text())
        };
    }
    let mut obj = Map::new();
    for a in attrs {
        obj.insert(format!("@{}", a.local), Value::String(a.value.clone()));
    }
    let mut text = String::new();
    for n in &el.children {
        match n {
            Node::Text(t) => text.push_str(t),
            Node::Elem(child) => {
                let particle = ty.and_then(|td| schema.particle(td, &child.local));
                let child_ty = particle.and_then(|p| schema.resolve(&p.ty));
                let v = element_to_json(child, child_ty, schema);
                let repeated = particle.map(|p| p.repeated).unwrap_or(false);
                match obj.get_mut(&child.local) {
                    // element_to_json never returns an array itself, so an
                    // existing array always means "siblings collected here".
                    Some(Value::Array(arr)) => arr.push(v),
                    Some(existing) => {
                        let prev = std::mem::take(existing);
                        *existing = Value::Array(vec![prev, v]);
                    }
                    None => {
                        obj.insert(
                            child.local.clone(),
                            if repeated { Value::Array(vec![v]) } else { v },
                        );
                    }
                }
            }
        }
    }
    if !text.trim().is_empty() {
        obj.insert("#text".into(), Value::String(text));
    }
    Value::Object(obj)
}

fn is_xsi_nil(a: &super::dom::Attr) -> bool {
    a.local == "nil" && a.ns.as_deref() == Some("http://www.w3.org/2001/XMLSchema-instance")
}

/// Render `value` as the global element `root` (e.g. the operation's
/// output element).
///
/// Every child's namespace comes from its own XSD declaration
/// (`Particle::ns`: `form=`, `elementFormDefault` of the schema document
/// that declares it, `ref=` into another namespace). Names stay
/// unprefixed; `xmlns="…"` / `xmlns=""` is written only where the
/// effective default namespace changes. Keys the schema doesn't know
/// inherit the current default namespace.
pub fn json_to_root_element(root: &QName, value: &Value, schema: &Schema) -> String {
    let ty = schema.element(root).and_then(|t| schema.resolve(t));
    let mut out = String::new();
    let ns = root.ns.as_deref();
    if schema.root_qualified(root) || ns.is_none() {
        // Root and (typically) its children share the default namespace.
        let decl = ns
            .map(|ns| format!(" xmlns=\"{}\"", escape(ns)))
            .unwrap_or_default();
        write_value(&mut out, &root.local, &decl, value, ty, ns, schema);
    } else {
        // Unqualified schema: prefix the root so unprefixed children
        // stay in no namespace without an `xmlns=""` on each of them.
        let decl = format!(" xmlns:tns=\"{}\"", escape(ns.unwrap_or_default()));
        write_value(
            &mut out,
            &format!("tns:{}", root.local),
            &decl,
            value,
            ty,
            None,
            schema,
        );
    }
    out
}

/// Wire namespace of child `name` of an element of type `parent_ty`.
fn child_namespace<'a>(
    schema: &'a Schema,
    parent_ty: Option<&'a TypeDef>,
    name: &str,
    default_ns: Option<&'a str>,
) -> Option<&'a str> {
    match parent_ty.and_then(|td| schema.particle(td, name)) {
        Some(p) => p.ns.as_deref(),
        None => default_ns,
    }
}

fn write_value(
    out: &mut String,
    name: &str,
    decl: &str,
    value: &Value,
    ty: Option<&TypeDef>,
    default_ns: Option<&str>,
    schema: &Schema,
) {
    match value {
        Value::Null => {}
        Value::Array(items) => {
            for it in items {
                write_value(out, name, decl, it, ty, default_ns, schema);
            }
        }
        Value::Object(obj) => {
            out.push('<');
            out.push_str(name);
            out.push_str(decl);
            for (k, v) in obj {
                if let Some(attr) = k.strip_prefix('@') {
                    // Names starting with "xml" (any case) are reserved
                    // (XML 1.0 §2.3) — `@xmlns` would redeclare or
                    // duplicate the element's namespace.
                    if !is_xml_name(attr) || attr.to_ascii_lowercase().starts_with("xml") {
                        tracing::warn!(key = ?k, "inbound: JSON attribute key is not a usable XML attribute name — dropped");
                        continue;
                    }
                    if let Some(s) = scalar(v) {
                        out.push_str(&format!(" {attr}=\"{}\"", escape(xml_safe(&s).as_ref())));
                    }
                }
            }
            let text = obj.get("#text").and_then(scalar);
            let mut keys: Vec<&String> = Vec::new();
            if let Some(td) = ty {
                for p in schema.particles(td) {
                    if let Some((k, _)) = obj.get_key_value(&p.name) {
                        if !keys.contains(&k) {
                            keys.push(k);
                        }
                    }
                }
            }
            for k in obj.keys() {
                if !k.starts_with('@') && k != "#text" && !keys.contains(&k) {
                    keys.push(k);
                }
            }
            // `null` and `[]` emit nothing, so they don't make a body.
            let has_body = text.is_some()
                || keys
                    .iter()
                    .filter(|k| is_xml_name(k))
                    .any(|k| match &obj[k.as_str()] {
                        Value::Null => false,
                        Value::Array(a) => !a.is_empty(),
                        _ => true,
                    });
            if !has_body {
                out.push_str("/>");
                return;
            }
            out.push('>');
            if let Some(t) = text {
                out.push_str(&escape(xml_safe(&t).as_ref()));
            }
            for k in keys {
                // Backend-controlled keys become element names — refuse
                // anything that isn't a plain NCName (markup injection,
                // undeclared prefixes). The root name comes from the schema.
                if !is_xml_name(k) {
                    tracing::warn!(key = ?k, "inbound: JSON key is not a valid XML name — dropped");
                    continue;
                }
                let child_ty = ty
                    .and_then(|td| schema.particle(td, k))
                    .and_then(|p| schema.resolve(&p.ty));
                let child_ns = child_namespace(schema, ty, k, default_ns);
                let child_decl = if child_ns == default_ns {
                    String::new()
                } else {
                    format!(" xmlns=\"{}\"", escape(child_ns.unwrap_or_default()))
                };
                write_value(
                    out,
                    k,
                    &child_decl,
                    &obj[k.as_str()],
                    child_ty,
                    child_ns,
                    schema,
                );
            }
            out.push_str(&format!("</{name}>"));
        }
        other => {
            let s = scalar(other).unwrap_or_default();
            out.push_str(&format!(
                "<{name}{decl}>{}</{name}>",
                escape(xml_safe(&s).as_ref())
            ));
        }
    }
}

/// Conservative XML `NCName` check: letters, digits, `_ - .`; must not
/// start with a digit/`-`/`.`. No `:` — a prefixed key would need a
/// namespace declaration the backend cannot provide (prefixes are
/// dropped on decode, so they are never needed on encode either).
fn is_xml_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Render arbitrary JSON as XML without a schema (fault `<detail>`
/// entries whose key is not a known global element).
pub fn json_to_plain(name: &str, value: &Value, schema: &Schema) -> String {
    if !is_xml_name(name) {
        tracing::warn!(key = ?name, "inbound: fault detail key is not a valid XML name — dropped");
        return String::new();
    }
    if let Some((q, _)) = schema.global_by_local(name) {
        return json_to_root_element(q, value, schema);
    }
    let mut out = String::new();
    write_value(&mut out, name, "", value, None, None, schema);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbound::{contract, dom};
    use serde_json::json;

    fn schema() -> contract::Contract {
        contract::load(
            r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:t" targetNamespace="urn:t">
            <wsdl:types><xs:schema targetNamespace="urn:t" elementFormDefault="qualified">
              <xs:element name="R"><xs:complexType><xs:sequence>
                <xs:element name="Header"><xs:complexType><xs:attribute name="id" type="xs:string"/></xs:complexType></xs:element>
                <xs:element name="first" type="xs:string"/>
                <xs:element name="item" type="xs:string" maxOccurs="unbounded"/>
                <xs:element name="last" type="xs:string"/>
              </xs:sequence></xs:complexType></xs:element>
            </xs:schema></wsdl:types></wsdl:definitions>"#,
            &|_| None,
        )
        .unwrap()
    }

    #[test]
    fn xml_to_json_attributes_arrays_and_verbatim_strings() {
        let c = schema();
        let q = QName {
            ns: Some("urn:t".into()),
            local: "R".into(),
        };
        let td = c.schema.resolve(c.schema.element(&q).unwrap());
        let el = dom::parse(r#"<x:R xmlns:x="urn:t"><x:Header id="007"/><x:first>0012</x:first><x:item>only</x:item><x:last/></x:R>"#).unwrap();
        let v = element_to_json(&el, td, &c.schema);
        assert_eq!(
            v,
            json!({"Header": {"@id": "007"}, "first": "0012", "item": ["only"], "last": ""})
        );
    }

    #[test]
    fn json_to_xml_follows_schema_order_and_repeats() {
        let c = schema();
        let q = QName {
            ns: Some("urn:t".into()),
            local: "R".into(),
        };
        // Keys deliberately out of order (serde_json sorts them anyway).
        let v = json!({"last": "z", "item": ["a", "b"], "first": 1, "Header": {"@id": "h&1"}, "extra": true});
        let xml = json_to_root_element(&q, &v, &c.schema);
        assert_eq!(
            xml,
            r#"<R xmlns="urn:t"><Header id="h&amp;1"/><first>1</first><item>a</item><item>b</item><last>z</last><extra>true</extra></R>"#
        );
    }

    #[test]
    fn invalid_json_keys_never_reach_the_wire() {
        let c = schema();
        let q = QName {
            ns: Some("urn:t".into()),
            local: "R".into(),
        };
        let xml = json_to_root_element(
            &q,
            &json!({"a><evil": "x", "@on\"x": "y", "first": "ok"}),
            &c.schema,
        );
        assert_eq!(xml, r#"<R xmlns="urn:t"><first>ok</first></R>"#);
    }

    #[test]
    fn xml_forbidden_chars_from_backend_are_replaced() {
        let c = schema();
        let q = QName {
            ns: Some("urn:t".into()),
            local: "R".into(),
        };
        let xml = json_to_root_element(
            &q,
            &json!({"Header": {"@id": "a\u{1}b"}, "first": "x\u{8}y\u{FFFE}", "last": {"#text": "t\u{1F}"}}),
            &c.schema,
        );
        assert_eq!(
            xml,
            "<R xmlns=\"urn:t\"><Header id=\"a\u{FFFD}b\"/><first>x\u{FFFD}y\u{FFFD}</first><last>t\u{FFFD}</last></R>"
        );
        assert!(
            crate::inbound::dom::parse(&xml).is_ok(),
            "peer can parse it"
        );
    }

    #[test]
    fn empty_array_child_yields_self_closing_parent() {
        let c = schema();
        let q = QName {
            ns: Some("urn:t".into()),
            local: "R".into(),
        };
        let xml = json_to_root_element(&q, &json!({"first": "a", "Header": {"x": []}}), &c.schema);
        assert_eq!(xml, r#"<R xmlns="urn:t"><Header/><first>a</first></R>"#);
    }

    #[test]
    fn prefixed_json_keys_are_dropped() {
        let c = schema();
        let q = QName {
            ns: Some("urn:t".into()),
            local: "R".into(),
        };
        let xml = json_to_root_element(
            &q,
            &json!({"a:b": "x", "tns:last": "y", "Header": {"@xsi:type": "T", "@xmlns": "urn:evil", "@XmlLang": "et", "@id": "1", "tns:x": "z"}, "first": "ok"}),
            &c.schema,
        );
        assert_eq!(
            xml,
            r#"<R xmlns="urn:t"><Header id="1"/><first>ok</first></R>"#
        );
        assert!(
            crate::inbound::dom::parse(&xml).is_ok(),
            "always well-formed"
        );
    }

    #[test]
    fn local_form_unqualified_and_types_from_other_namespace() {
        // Schema A (qualified) declares R; child `plain` is form="unqualified";
        // child `ext` uses complexType T defined in qualified schema B, so
        // T's children are in B's namespace.
        let c = contract::load(
            r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:b="urn:b" targetNamespace="urn:a">
            <wsdl:types>
              <xs:schema targetNamespace="urn:b" elementFormDefault="qualified">
                <xs:complexType name="T"><xs:sequence><xs:element name="v" type="xs:string"/></xs:sequence></xs:complexType>
              </xs:schema>
              <xs:schema targetNamespace="urn:a" elementFormDefault="qualified">
                <xs:element name="R"><xs:complexType><xs:sequence>
                  <xs:element name="q" type="xs:string"/>
                  <xs:element name="plain" type="xs:string" form="unqualified"/>
                  <xs:element name="ext" type="b:T"/>
                </xs:sequence></xs:complexType></xs:element>
              </xs:schema>
            </wsdl:types></wsdl:definitions>"#,
            &|_| None,
        )
        .unwrap();
        let q = QName {
            ns: Some("urn:a".into()),
            local: "R".into(),
        };
        let xml = json_to_root_element(
            &q,
            &json!({"q": "1", "plain": "2", "ext": {"v": "3"}}),
            &c.schema,
        );
        assert_eq!(
            xml,
            r#"<R xmlns="urn:a"><q>1</q><plain xmlns="">2</plain><ext><v xmlns="urn:b">3</v></ext></R>"#
        );
        let back = crate::inbound::dom::parse(&xml).unwrap();
        assert_eq!(back.child("q").unwrap().ns.as_deref(), Some("urn:a"));
        assert_eq!(back.child("plain").unwrap().ns, None);
        assert_eq!(back.child("ext").unwrap().ns.as_deref(), Some("urn:a"));
        assert_eq!(
            back.child("ext").unwrap().child("v").unwrap().ns.as_deref(),
            Some("urn:b")
        );
    }

    #[test]
    fn ref_to_element_in_other_namespace_gets_its_own_xmlns() {
        let c = contract::load(
            r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:o="urn:other" targetNamespace="urn:main">
            <wsdl:types>
              <xs:schema targetNamespace="urn:other" elementFormDefault="qualified">
                <xs:element name="Ext"><xs:complexType><xs:sequence><xs:element name="v" type="xs:string"/></xs:sequence></xs:complexType></xs:element>
              </xs:schema>
              <xs:schema targetNamespace="urn:main"><xs:element name="R"><xs:complexType><xs:sequence>
                <xs:element name="a" type="xs:string"/><xs:element ref="o:Ext"/>
              </xs:sequence></xs:complexType></xs:element></xs:schema>
            </wsdl:types></wsdl:definitions>"#,
            &|_| None,
        )
        .unwrap();
        let q = QName {
            ns: Some("urn:main".into()),
            local: "R".into(),
        };
        let xml = json_to_root_element(&q, &json!({"a": "1", "Ext": {"v": "2"}}), &c.schema);
        assert_eq!(
            xml,
            r#"<tns:R xmlns:tns="urn:main"><a>1</a><Ext xmlns="urn:other"><v>2</v></Ext></tns:R>"#
        );
        let back = crate::inbound::dom::parse(&xml).unwrap();
        assert_eq!(back.child("a").unwrap().ns, None);
        assert_eq!(
            back.child("Ext").unwrap().child("v").unwrap().ns.as_deref(),
            Some("urn:other")
        );
    }

    #[test]
    fn unqualified_schema_prefixes_only_the_root() {
        let c = contract::load(
            r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:u">
            <wsdl:types><xs:schema targetNamespace="urn:u"><xs:element name="R"><xs:complexType><xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence></xs:complexType></xs:element></xs:schema></wsdl:types></wsdl:definitions>"#,
            &|_| None,
        )
        .unwrap();
        let q = QName {
            ns: Some("urn:u".into()),
            local: "R".into(),
        };
        assert_eq!(
            json_to_root_element(&q, &json!({"a": "1", "tns:b": "2"}), &c.schema),
            r#"<tns:R xmlns:tns="urn:u"><a>1</a></tns:R>"#
        );
    }
}
