//! SOAP XML → JSON translation.
//!
//! Design goals (from DESIGN.md §8.5):
//! * Parse the SOAP envelope with `quick-xml` (UTF-8 by default;
//!   task 008 has the explicit character-fidelity test).
//! * Emit `{"headers": …, "body": …}` — both. The JVM version
//!   dropped SOAP headers on the floor (bug #7).
//! * Element attributes are surfaced as `@name` keys on the same
//!   object (loose XML→JSON convention).
//! * Text content of an element with children lives under `#text`.
//! * Repeated same-name children become an array.
//! * Namespace prefixes are preserved verbatim in element names
//!   (`prod:keha`) — X-Road payloads mix namespaces meaningfully.

use crate::error::XtrError;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use serde_json::{json, Value};

pub fn translate_soap(xml: &str) -> Result<Value, XtrError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let root = parse_element(&mut reader)?;

    // Envelope's children should include Header + Body. Extract
    // both under stable keys. If neither present, return the raw
    // parsed shape under `body` for tolerance.
    let (headers, body) = extract_header_and_body(&root);
    Ok(json!({
        "headers": headers,
        "body": body,
    }))
}

/// Walk the top-level `<Envelope>` and pull out `<Header>` +
/// `<Body>` regardless of the namespace prefix.
fn extract_header_and_body(root: &Value) -> (Value, Value) {
    let inner = if let Some(env) = find_child_endswith(root, "Envelope") {
        env
    } else {
        root
    };
    let headers = find_child_endswith(inner, "Header")
        .cloned()
        .unwrap_or(Value::Null);
    let body = find_child_endswith(inner, "Body")
        .cloned()
        .unwrap_or_else(|| inner.clone());
    (headers, body)
}

fn find_child_endswith<'a>(node: &'a Value, suffix: &str) -> Option<&'a Value> {
    node.as_object()?.iter().find_map(|(k, v)| {
        if k == suffix || k.ends_with(&format!(":{suffix}")) {
            Some(v)
        } else {
            None
        }
    })
}

/// Recursively parse the next element (assumes reader is
/// positioned at the start of the document or immediately after
/// a Start event for the parent). Returns a single-key JSON
/// object `{ "elementName": <content> }`.
fn parse_element(reader: &mut Reader<&[u8]>) -> Result<Value, XtrError> {
    loop {
        match reader.read_event() {
            Ok(Event::Decl(_)) | Ok(Event::Comment(_)) | Ok(Event::PI(_)) => continue,
            Ok(Event::Start(e)) => {
                let name = element_name(&e);
                let content = parse_children(reader, &name, attrs_to_object(&e))?;
                return Ok(json!({ name: content }));
            }
            Ok(Event::Empty(e)) => {
                let name = element_name(&e);
                let obj = attrs_to_object(&e);
                if obj.as_object().unwrap().is_empty() {
                    return Ok(json!({ name: Value::Null }));
                }
                // Self-closing element with attributes only.
                return Ok(json!({ name: obj }));
            }
            Ok(Event::Eof) => {
                return Err(XtrError::XmlParseError(
                    "unexpected EOF at document root".into(),
                ));
            }
            Ok(_) => continue,
            Err(e) => return Err(XtrError::XmlParseError(format!("reading root: {e}"))),
        }
    }
}

fn parse_children(
    reader: &mut Reader<&[u8]>,
    close_name: &str,
    attrs_seed: Value,
) -> Result<Value, XtrError> {
    let mut obj = attrs_seed
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect::<serde_json::Map<String, Value>>();
    let mut text_buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let child_name = element_name(&e);
                let child_content = parse_children(reader, &child_name, attrs_to_object(&e))?;
                insert_merging(&mut obj, &child_name, child_content);
            }
            Ok(Event::Empty(e)) => {
                let child_name = element_name(&e);
                let attrs = attrs_to_object(&e);
                let val = if attrs.as_object().unwrap().is_empty() {
                    Value::Null
                } else {
                    attrs
                };
                insert_merging(&mut obj, &child_name, val);
            }
            Ok(Event::Text(t)) => {
                let text = t
                    .unescape()
                    .map_err(|e| XtrError::XmlParseError(format!("text unescape: {e}")))?;
                text_buf.push_str(&text);
            }
            Ok(Event::CData(t)) => {
                text_buf.push_str(
                    std::str::from_utf8(&t)
                        .map_err(|e| XtrError::XmlParseError(format!("cdata utf-8: {e}")))?,
                );
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == close_name {
                    let trimmed = text_buf.trim();
                    // Leaf-with-only-text → return the string.
                    // Leaf-with-attributes-and-only-text → object
                    // with `#text` key. Element-with-children →
                    // return the object; drop trimmed empty text.
                    if obj.is_empty() {
                        return Ok(if trimmed.is_empty() {
                            Value::Null
                        } else {
                            Value::String(trimmed.to_string())
                        });
                    }
                    if !trimmed.is_empty() {
                        obj.insert("#text".into(), Value::String(trimmed.to_string()));
                    }
                    return Ok(Value::Object(obj));
                }
                // Non-matching end tag — malformed but tolerate.
                return Err(XtrError::XmlParseError(format!(
                    "unexpected </{}> while parsing <{}>",
                    name, close_name
                )));
            }
            Ok(Event::Eof) => {
                return Err(XtrError::XmlParseError(format!(
                    "unexpected EOF while parsing <{}>",
                    close_name
                )));
            }
            Ok(Event::Decl(_))
            | Ok(Event::Comment(_))
            | Ok(Event::PI(_))
            | Ok(Event::DocType(_)) => {
                continue;
            }
            Err(e) => return Err(XtrError::XmlParseError(format!("{e}"))),
        }
    }
}

fn element_name(e: &BytesStart) -> String {
    String::from_utf8_lossy(e.name().as_ref()).into_owned()
}

fn attrs_to_object(e: &BytesStart) -> Value {
    let mut out = serde_json::Map::new();
    for attr in e.attributes().flatten() {
        let key = format!("@{}", String::from_utf8_lossy(attr.key.as_ref()));
        let val = attr
            .unescape_value()
            .map(|c| c.into_owned())
            .unwrap_or_default();
        out.insert(key, Value::String(val));
    }
    Value::Object(out)
}

/// If `key` isn't present → insert. If present as scalar/object →
/// promote to array. If already array → push.
fn insert_merging(obj: &mut serde_json::Map<String, Value>, key: &str, val: Value) {
    match obj.remove(key) {
        None => {
            obj.insert(key.into(), val);
        }
        Some(Value::Array(mut arr)) => {
            arr.push(val);
            obj.insert(key.into(), Value::Array(arr));
        }
        Some(existing) => {
            obj.insert(key.into(), Value::Array(vec![existing, val]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(v: &Value) -> &Value {
        v.get("body").unwrap()
    }

    fn headers(v: &Value) -> &Value {
        v.get("headers").unwrap()
    }

    #[test]
    fn extracts_body_and_headers_separately() {
        let xml = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
            <soap:Header>
                <token>abc</token>
            </soap:Header>
            <soap:Body>
                <result>ok</result>
            </soap:Body>
        </soap:Envelope>"#;
        let v = translate_soap(xml).unwrap();
        assert_eq!(headers(&v), &json!({ "token": "abc" }));
        assert_eq!(body(&v), &json!({ "result": "ok" }));
    }

    #[test]
    fn utf8_estonian_characters_round_trip() {
        // Belongs to task 008's coverage; guard-rail here so a
        // Phase E regression is caught before task 008's tests exist.
        let xml = "<soap:Envelope><soap:Body><name>Peeter Kärp</name></soap:Body></soap:Envelope>";
        let v = translate_soap(xml).unwrap();
        assert_eq!(body(&v), &json!({ "name": "Peeter Kärp" }));
    }

    #[test]
    fn xml_entity_reference_decoded() {
        let xml =
            "<soap:Envelope><soap:Body><name>Peeter K&#228;rp</name></soap:Body></soap:Envelope>";
        let v = translate_soap(xml).unwrap();
        assert_eq!(body(&v), &json!({ "name": "Peeter Kärp" }));
    }

    #[test]
    fn repeated_children_become_arrays() {
        let xml = r#"<soap:Envelope><soap:Body>
            <items>
                <item>one</item>
                <item>two</item>
                <item>three</item>
            </items>
        </soap:Body></soap:Envelope>"#;
        let v = translate_soap(xml).unwrap();
        assert_eq!(
            body(&v),
            &json!({
                "items": {
                    "item": ["one", "two", "three"]
                }
            })
        );
    }

    #[test]
    fn attributes_surface_as_at_prefix() {
        let xml = r#"<soap:Envelope><soap:Body>
            <link href="https://ex.com">click</link>
        </soap:Body></soap:Envelope>"#;
        let v = translate_soap(xml).unwrap();
        assert_eq!(
            body(&v),
            &json!({
                "link": { "@href": "https://ex.com", "#text": "click" }
            })
        );
    }

    #[test]
    fn empty_element_yields_null() {
        let xml = "<soap:Envelope><soap:Body><nada/></soap:Body></soap:Envelope>";
        let v = translate_soap(xml).unwrap();
        assert_eq!(body(&v), &json!({ "nada": Value::Null }));
    }

    #[test]
    fn malformed_xml_returns_parse_error() {
        let err = translate_soap("<soap:Envelope><unclosed>").unwrap_err();
        assert!(matches!(err, XtrError::XmlParseError(_)));
    }

    #[test]
    fn namespaced_element_names_preserve_prefix() {
        let xml = r#"<soap:Envelope><soap:Body>
            <prod:keha><prod:reg_code>42</prod:reg_code></prod:keha>
        </soap:Body></soap:Envelope>"#;
        let v = translate_soap(xml).unwrap();
        assert_eq!(
            body(&v),
            &json!({
                "prod:keha": {
                    "prod:reg_code": "42"
                }
            })
        );
    }
}
