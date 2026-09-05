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

/// Hard cap on element nesting depth. Real X-Road envelopes rarely
/// exceed 10 levels; 128 leaves plenty of headroom while stopping a
/// pathological "billion laughs"-style nested-element DoS from
/// blowing the stack via unbounded recursion in `parse_children`.
/// Lowered from 512 in audit-v1: debug builds have ~2 MB test-thread
/// stacks and the higher cap was on the edge — a defensive cap that
/// can itself cause a stack overflow is a self-own.
const MAX_NESTING_DEPTH: u32 = 128;

/// Audit-v1 C2 second-layer cap. Depth doesn't catch "wide-and-flat"
/// bombs (a million siblings at depth 3); this per-document event
/// counter does. Legit X-Road envelopes are hundreds to low
/// thousands of events. 100k is well above real payloads (a 16 MiB
/// response body of `<a/>` elements is roughly ~2M events) and
/// still bounds parse-time work regardless of payload shape.
/// Also serves as a safety net if quick-xml's DOCTYPE/entity
/// posture ever regresses.
const MAX_XML_EVENTS: usize = 100_000;

pub fn translate_soap(xml: &str) -> Result<Value, XtrError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut event_budget: usize = MAX_XML_EVENTS;
    let root = parse_element(&mut reader, &mut event_budget)?;

    // Envelope's children should include Header + Body. Extract
    // both under stable keys. If neither present, return the raw
    // parsed shape under `body` for tolerance.
    let (headers, body) = extract_header_and_body(&root);

    // Task 010: a <Fault> child in <Body> is a *business* error
    // that rides on HTTP 200 — reject it explicitly instead of
    // silently translating it as a successful response.
    if let Some(fault) = find_child_endswith(&body, "Fault") {
        return Err(fault_to_error(fault));
    }

    Ok(json!({
        "headers": headers,
        "body": body,
    }))
}

/// Best-effort SOAP-Fault extraction used by the executor to
/// recognise faults on *non-2xx* responses (e.g. Ariregister
/// wraps its faults in HTTP 500). Returns `Some(UpstreamSoapFault)`
/// only when the bytes are a valid SOAP envelope containing a
/// `<Body><Fault>…</Fault></Body>` subtree. Any parse failure or
/// missing fault → `None`, letting the caller fall back to
/// `UpstreamHttpError`.
pub fn try_extract_soap_fault(xml: &str) -> Option<XtrError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut event_budget: usize = MAX_XML_EVENTS;
    let root = parse_element(&mut reader, &mut event_budget).ok()?;
    let (_, body) = extract_header_and_body(&root);
    let fault = find_child_endswith(&body, "Fault")?;
    Some(fault_to_error(fault))
}

/// Convert a parsed `<Fault>` subtree into an `UpstreamSoapFault`
/// error. Handles both SOAP 1.1 (`faultcode`/`faultstring`) and
/// SOAP 1.2 (`Code/Value`/`Reason/Text`) shapes, along with
/// namespace-prefixed variants (`env:faultcode` etc).
fn fault_to_error(fault: &Value) -> XtrError {
    let code = pluck_soap12_code(fault)
        .or_else(|| pluck_string(fault, "faultcode"))
        .unwrap_or_else(|| "unknown".to_string());
    let string = pluck_soap12_reason(fault)
        .or_else(|| pluck_string(fault, "faultstring"))
        .unwrap_or_else(|| "no faultstring".to_string());
    let detail = find_child_endswith(fault, "detail")
        .or_else(|| find_child_endswith(fault, "Detail"))
        .cloned();
    XtrError::UpstreamSoapFault {
        code,
        string,
        detail,
    }
}

/// SOAP 1.2: `<Code><Value>env:Sender</Value></Code>`
fn pluck_soap12_code(fault: &Value) -> Option<String> {
    let code_node = find_child_endswith(fault, "Code")?;
    let val = find_child_endswith(code_node, "Value")?;
    val.as_str().map(str::to_string)
}

/// SOAP 1.2: `<Reason><Text xml:lang="en">…</Text></Reason>`
fn pluck_soap12_reason(fault: &Value) -> Option<String> {
    let reason = find_child_endswith(fault, "Reason")?;
    let text = find_child_endswith(reason, "Text")?;
    // Text element may be a bare string OR an object with `#text`
    // (when it has attributes like xml:lang).
    text.as_str().map(str::to_string).or_else(|| {
        text.get("#text")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn pluck_string(fault: &Value, suffix: &str) -> Option<String> {
    let child = find_child_endswith(fault, suffix)?;
    child.as_str().map(str::to_string).or_else(|| {
        child
            .get("#text")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
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

/// Bump the per-document event budget. Returns Err when exhausted
/// — the counter stops wide-and-flat XML bombs (millions of siblings
/// at shallow depth) that the depth cap wouldn't catch. Also a
/// safety net if quick-xml's DOCTYPE/entity posture ever regresses.
fn charge_event(budget: &mut usize) -> Result<(), XtrError> {
    if *budget == 0 {
        return Err(XtrError::XmlParseError(format!(
            "XML event budget exhausted ({MAX_XML_EVENTS}) — refusing possible XML bomb"
        )));
    }
    *budget -= 1;
    Ok(())
}

/// Recursively parse the next element (assumes reader is
/// positioned at the start of the document or immediately after
/// a Start event for the parent). Returns a single-key JSON
/// object `{ "elementName": <content> }`.
fn parse_element(
    reader: &mut Reader<&[u8]>,
    event_budget: &mut usize,
) -> Result<Value, XtrError> {
    loop {
        charge_event(event_budget)?;
        match reader.read_event() {
            Ok(Event::Decl(_)) | Ok(Event::Comment(_)) | Ok(Event::PI(_)) => continue,
            Ok(Event::Start(e)) => {
                let name = element_name(&e);
                let content =
                    parse_children(reader, &name, attrs_to_object(&e), 1, event_budget)?;
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
    depth: u32,
    event_budget: &mut usize,
) -> Result<Value, XtrError> {
    if depth > MAX_NESTING_DEPTH {
        return Err(XtrError::XmlParseError(format!(
            "XML nesting depth exceeded ({MAX_NESTING_DEPTH})"
        )));
    }
    let mut obj = attrs_seed
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect::<serde_json::Map<String, Value>>();
    let mut text_buf = String::new();

    loop {
        charge_event(event_budget)?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let child_name = element_name(&e);
                let child_content = parse_children(
                    reader,
                    &child_name,
                    attrs_to_object(&e),
                    depth + 1,
                    event_budget,
                )?;
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
                // quick-xml 0.41 dropped BytesText::unescape() —
                // manually decode UTF-8 then run the entity
                // unescape from the escape module.
                let decoded = t
                    .decode()
                    .map_err(|e| XtrError::XmlParseError(format!("text decode: {e}")))?;
                let text = quick_xml::escape::unescape(&decoded)
                    .map_err(|e| XtrError::XmlParseError(format!("text unescape: {e}")))?;
                text_buf.push_str(&text);
            }
            Ok(Event::CData(t)) => {
                let cdata = t
                    .decode()
                    .map_err(|e| XtrError::XmlParseError(format!("cdata decode: {e}")))?;
                text_buf.push_str(&cdata);
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
                            coerce_leaf_value(trimmed)
                        });
                    }
                    if !trimmed.is_empty() {
                        // Attributed leaves keep #text as a string to
                        // preserve the raw payload verbatim — coercion
                        // is only applied to bare leaves where the
                        // consumer's expectation is clearest.
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
            // quick-xml 0.41 emits GeneralRef for entity references
            // that aren't in the XML-predefined set. Two flavours:
            //   * Character references (&#228;, &#xE4;) — always
            //     resolve to a Unicode codepoint. Standard XML.
            //   * Custom entities (&nbsp;, &copy;) — require a
            //     DOCTYPE, which we don't accept (XXE risk).
            //     Rejected explicitly.
            Ok(Event::GeneralRef(g)) => {
                let name = String::from_utf8_lossy(g.as_ref()).into_owned();
                if let Some(ch) = decode_char_ref(&name) {
                    text_buf.push(ch);
                    continue;
                }
                return Err(XtrError::XmlParseError(format!(
                    "unresolved general entity &{}; — custom entities are not supported (XXE risk)",
                    name
                )));
            }
            Err(e) => return Err(XtrError::XmlParseError(format!("{e}"))),
        }
    }
}

/// Task 012 — narrow, opt-in type coercion for bare-leaf text.
/// Rules:
///   - `"true"` / `"false"` (exact case) → boolean.
///   - Pure-digit string that fits `i64` → number. Leading zeros
///     preserved as string ("007", "01") — those are almost
///     always identifiers, not numbers.
///   - Everything else stays `Value::String`. In particular NO
///     float coercion (precision loss on decimals is a footgun)
///     and NO date parsing (locale-dependent).
fn coerce_leaf_value(s: &str) -> Value {
    if s == "true" {
        return Value::Bool(true);
    }
    if s == "false" {
        return Value::Bool(false);
    }
    if looks_like_int(s) {
        if let Ok(n) = s.parse::<i64>() {
            return Value::Number(n.into());
        }
        // Overflow (or any parse failure): keep the raw string so
        // no digits are lost.
    }
    Value::String(s.to_string())
}

/// Resolves an XML character reference of the form `#nnn` (decimal)
/// or `#xhh` / `#Xhh` (hex) into a Unicode `char`. Anything else
/// (custom entity name) → None.
fn decode_char_ref(name: &str) -> Option<char> {
    let rest = name.strip_prefix('#')?;
    let code_point = if let Some(hex) = rest.strip_prefix(['x', 'X']) {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        rest.parse::<u32>().ok()?
    };
    char::from_u32(code_point)
}

fn looks_like_int(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let (sign_len, digits) = match bytes[0] {
        b'-' | b'+' => (1, &bytes[1..]),
        _ => (0, bytes),
    };
    if digits.is_empty() {
        return false;
    }
    // Leading-zero guard: multi-digit values starting with '0' are
    // opaque identifiers (registry codes, product SKUs) — keep as
    // string.
    if digits.len() > 1 && digits[0] == b'0' {
        return false;
    }
    // "-0" is silly but not invalid; reject leading '-' followed
    // by only '0's to keep behavior predictable.
    if sign_len == 1 && bytes[0] == b'-' && digits.iter().all(|&b| b == b'0') {
        return false;
    }
    digits.iter().all(|b| b.is_ascii_digit())
}

fn element_name(e: &BytesStart) -> String {
    String::from_utf8_lossy(e.name().as_ref()).into_owned()
}

fn attrs_to_object(e: &BytesStart) -> Value {
    let mut out = serde_json::Map::new();
    for attr in e.attributes().flatten() {
        let key = format!("@{}", String::from_utf8_lossy(attr.key.as_ref()));
        // quick-xml 0.41: `unescape_value` is deprecated;
        // `normalized_value` requires an XmlVersion so is a
        // heavier API change. Do the same manual dance as text:
        // decode UTF-8 → run entity unescape.
        let val = std::str::from_utf8(&attr.value)
            .ok()
            .and_then(|s| quick_xml::escape::unescape(s).ok().map(|c| c.into_owned()))
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
    fn xml_hex_char_ref_decoded() {
        // Hex form of &#228; (U+00E4) — Estonian ä.
        let xml =
            "<soap:Envelope><soap:Body><name>Peeter K&#xE4;rp</name></soap:Body></soap:Envelope>";
        let v = translate_soap(xml).unwrap();
        assert_eq!(body(&v), &json!({ "name": "Peeter Kärp" }));
    }

    #[test]
    fn custom_entity_rejected_no_xxe() {
        // A custom entity (e.g. &nbsp;) requires a DOCTYPE, which
        // opens the door to XXE. quick-xml already refuses DOCTYPE
        // entity resolution; we double-down by rejecting any
        // unresolved GeneralRef with an explicit error.
        let xml = "<soap:Envelope><soap:Body><n>hello&nbsp;world</n></soap:Body></soap:Envelope>";
        let err = translate_soap(xml).unwrap_err();
        assert!(
            matches!(&err, XtrError::XmlParseError(m) if m.contains("nbsp") && m.contains("XXE")),
            "expected XXE-guard message, got: {err:?}"
        );
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
    fn soap_1_1_fault_returns_upstream_soap_fault_error() {
        let xml = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
            <soap:Body>
                <soap:Fault>
                    <faultcode>Client.MissingParam</faultcode>
                    <faultstring>reg_code required</faultstring>
                </soap:Fault>
            </soap:Body>
        </soap:Envelope>"#;
        let err = translate_soap(xml).unwrap_err();
        match err {
            XtrError::UpstreamSoapFault {
                code,
                string,
                detail,
            } => {
                assert_eq!(code, "Client.MissingParam");
                assert_eq!(string, "reg_code required");
                assert!(detail.is_none());
            }
            other => panic!("expected UpstreamSoapFault, got {other:?}"),
        }
    }

    #[test]
    fn try_extract_soap_fault_recognises_fault_regardless_of_wrapper() {
        // Real Ariregister returns this shape inside HTTP 500.
        let xml = r#"<SOAP-ENV:Envelope xmlns:SOAP-ENV="http://schemas.xmlsoap.org/soap/envelope/">
            <SOAP-ENV:Header></SOAP-ENV:Header>
            <SOAP-ENV:Body>
                <SOAP-ENV:Fault>
                    <faultcode>SOAP-ENV:Server</faultcode>
                    <faultstring>Incorrect user name or password.</faultstring>
                </SOAP-ENV:Fault>
            </SOAP-ENV:Body>
        </SOAP-ENV:Envelope>"#;
        let err = try_extract_soap_fault(xml).expect("should extract fault");
        match err {
            XtrError::UpstreamSoapFault { code, string, .. } => {
                assert_eq!(code, "SOAP-ENV:Server");
                assert_eq!(string, "Incorrect user name or password.");
            }
            other => panic!("expected UpstreamSoapFault, got {other:?}"),
        }
    }

    #[test]
    fn try_extract_soap_fault_returns_none_on_non_fault_xml() {
        // Ordinary success envelope → no fault present.
        let xml = r#"<soap:Envelope><soap:Body><result>ok</result></soap:Body></soap:Envelope>"#;
        assert!(try_extract_soap_fault(xml).is_none());
    }

    #[test]
    fn try_extract_soap_fault_returns_none_on_garbage() {
        // Random non-XML body must not blow up — just return None
        // so the caller falls back to the opaque UpstreamHttpError.
        assert!(try_extract_soap_fault("not xml at all").is_none());
        assert!(try_extract_soap_fault("<unclosed").is_none());
        assert!(try_extract_soap_fault("").is_none());
    }

    #[test]
    fn soap_1_2_fault_returns_upstream_soap_fault_error() {
        let xml = r#"<env:Envelope xmlns:env="http://www.w3.org/2003/05/soap-envelope">
            <env:Body>
                <env:Fault>
                    <env:Code><env:Value>env:Sender</env:Value></env:Code>
                    <env:Reason><env:Text xml:lang="en">Bad payload</env:Text></env:Reason>
                </env:Fault>
            </env:Body>
        </env:Envelope>"#;
        let err = translate_soap(xml).unwrap_err();
        match err {
            XtrError::UpstreamSoapFault { code, string, .. } => {
                assert_eq!(code, "env:Sender");
                assert_eq!(string, "Bad payload");
            }
            other => panic!("expected UpstreamSoapFault, got {other:?}"),
        }
    }

    #[test]
    fn soap_1_1_fault_with_detail_preserves_detail_body() {
        let xml = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
            <soap:Body>
                <soap:Fault>
                    <faultcode>Server.BackendDown</faultcode>
                    <faultstring>backend unavailable</faultstring>
                    <detail>
                        <retry_after>30</retry_after>
                    </detail>
                </soap:Fault>
            </soap:Body>
        </soap:Envelope>"#;
        let err = translate_soap(xml).unwrap_err();
        match err {
            XtrError::UpstreamSoapFault { detail, .. } => {
                let d = detail.expect("detail should be present");
                // Task 012 coercion: bare integer leaf → Number.
                assert_eq!(d["retry_after"], 30);
            }
            other => panic!("expected UpstreamSoapFault, got {other:?}"),
        }
    }

    #[test]
    fn non_fault_body_still_translates_as_success() {
        // Regression guard: only a real <Fault> child should trip
        // the new code path. Ordinary <result> etc should not.
        let xml = r#"<soap:Envelope><soap:Body>
            <not_a_fault><faultcode>looks like it</faultcode></not_a_fault>
        </soap:Body></soap:Envelope>"#;
        let v = translate_soap(xml).unwrap();
        assert_eq!(
            body(&v),
            &json!({ "not_a_fault": { "faultcode": "looks like it" } })
        );
    }

    #[test]
    fn namespaced_element_names_preserve_prefix() {
        // Use a non-numeric value so this test focuses purely on
        // namespace-prefix preservation, not the (separate) type
        // coercion story.
        let xml = r#"<soap:Envelope><soap:Body>
            <prod:keha><prod:name>foo</prod:name></prod:keha>
        </soap:Body></soap:Envelope>"#;
        let v = translate_soap(xml).unwrap();
        assert_eq!(
            body(&v),
            &json!({
                "prod:keha": {
                    "prod:name": "foo"
                }
            })
        );
    }

    // ---------- Task 012: type coercion tests ----------

    fn coerced(xml: &str) -> Value {
        let v = translate_soap(&format!(
            "<soap:Envelope><soap:Body>{xml}</soap:Body></soap:Envelope>"
        ))
        .unwrap();
        body(&v).clone()
    }

    #[test]
    fn coerce_int_leaf() {
        assert_eq!(coerced("<n>42</n>"), json!({ "n": 42 }));
        assert_eq!(coerced("<n>-42</n>"), json!({ "n": -42 }));
        assert_eq!(coerced("<n>0</n>"), json!({ "n": 0 }));
    }

    #[test]
    fn coerce_bool_leaf() {
        assert_eq!(coerced("<b>true</b>"), json!({ "b": true }));
        assert_eq!(coerced("<b>false</b>"), json!({ "b": false }));
    }

    #[test]
    fn coerce_bool_is_case_sensitive() {
        // "True", "TRUE", "yes", etc stay as strings — safer than
        // guessing what the upstream meant.
        assert_eq!(coerced("<b>True</b>"), json!({ "b": "True" }));
        assert_eq!(coerced("<b>TRUE</b>"), json!({ "b": "TRUE" }));
        assert_eq!(coerced("<b>yes</b>"), json!({ "b": "yes" }));
    }

    #[test]
    fn leading_zero_stays_string_registry_code_safe() {
        // "007" or "01" are opaque identifiers, not integers.
        assert_eq!(coerced("<id>007</id>"), json!({ "id": "007" }));
        assert_eq!(coerced("<id>01</id>"), json!({ "id": "01" }));
        // But a bare "0" is a genuine number.
        assert_eq!(coerced("<n>0</n>"), json!({ "n": 0 }));
    }

    #[test]
    fn decimal_stays_string_no_float_precision_loss() {
        assert_eq!(coerced("<x>3.14</x>"), json!({ "x": "3.14" }));
        assert_eq!(coerced("<x>3.10</x>"), json!({ "x": "3.10" }));
        assert_eq!(coerced("<x>0.5</x>"), json!({ "x": "0.5" }));
    }

    #[test]
    fn i64_overflow_stays_string() {
        // 20 nines — well over i64::MAX (~9.2e18). Must not lose
        // digits by silently truncating to a float.
        assert_eq!(
            coerced("<big>99999999999999999999</big>"),
            json!({ "big": "99999999999999999999" })
        );
    }

    #[test]
    fn deeply_nested_xml_bounded_by_depth_cap() {
        // Build a document deeper than MAX_NESTING_DEPTH. Without
        // the cap this would stack-overflow via unbounded recursion.
        let depth = (super::MAX_NESTING_DEPTH as usize) + 20;
        let opens: String = "<a>".repeat(depth);
        let closes: String = "</a>".repeat(depth);
        let xml = format!("<soap:Envelope><soap:Body>{opens}x{closes}</soap:Body></soap:Envelope>");
        let err = translate_soap(&xml).unwrap_err();
        assert!(
            matches!(&err, XtrError::XmlParseError(m) if m.contains("nesting depth")),
            "expected depth-cap error, got {err:?}"
        );
    }

    #[test]
    fn attributed_leaf_text_still_string() {
        // <#text> value under an attributed leaf keeps the raw
        // string — coercion only fires on bare leaves.
        let v = coerced(r#"<x kind="int">42</x>"#);
        assert_eq!(v, json!({ "x": { "@kind": "int", "#text": "42" } }));
    }

    // ---------- Audit-v1 C2 regression pins ----------

    #[test]
    fn audit_c2_billion_laughs_rejected_no_expansion() {
        // Classic entity-expansion bomb. quick-xml 0.41 rejects
        // custom entities at the parser level and our
        // Event::GeneralRef handler rejects unresolved references.
        // This test locks in that a bomb never expands, whether
        // via DOCTYPE or GeneralRef surface.
        let bomb = r#"<?xml version="1.0"?>
<!DOCTYPE lolz [
  <!ENTITY lol "lol">
  <!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
  <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
]>
<SOAP:Envelope xmlns:SOAP="http://schemas.xmlsoap.org/soap/envelope/">
  <SOAP:Body><result>&lol3;</result></SOAP:Body>
</SOAP:Envelope>"#;
        let err = translate_soap(bomb).unwrap_err();
        assert!(
            matches!(&err, XtrError::XmlParseError(m)
                if m.contains("custom entities") || m.contains("XXE")),
            "expected entity-guard error, got {err:?}"
        );
    }

    #[test]
    fn audit_c2_wide_and_flat_bomb_capped_by_event_budget() {
        // Depth cap alone doesn't stop a bomb that goes wide-and-
        // flat: 200k `<x/>` siblings at depth 3. Verify the
        // event-count safety net rejects it. Use MAX_XML_EVENTS +
        // some slack so a legit large payload just under the cap
        // still succeeds (covered by a separate test).
        let mut body = String::from("<soap:Envelope><soap:Body>");
        // Each `<x/>` is one Empty event, so > MAX_XML_EVENTS empties
        // will trip the counter regardless of nesting depth.
        for _ in 0..(super::MAX_XML_EVENTS + 100) {
            body.push_str("<x/>");
        }
        body.push_str("</soap:Body></soap:Envelope>");
        let err = translate_soap(&body).unwrap_err();
        assert!(
            matches!(&err, XtrError::XmlParseError(m) if m.contains("event budget")),
            "expected event-budget error, got {err:?}"
        );
    }

    #[test]
    fn audit_c2_doctype_without_entity_body_does_not_error() {
        // A DOCTYPE with no entity table is harmless (used e.g.
        // by some SOAP toolchains to name the root). The parser
        // should ignore it, not error out — that behaviour is
        // covered by the DocType branch of parse_children.
        let xml = r#"<?xml version="1.0"?>
<!DOCTYPE soap:Envelope>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body><ok>1</ok></soap:Body>
</soap:Envelope>"#;
        let v = translate_soap(xml).unwrap();
        assert_eq!(body(&v), &json!({ "ok": 1 }));
    }
}
