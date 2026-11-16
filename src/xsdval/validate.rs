//! Validation of one parsed message against the model.
//!
//! The walk is deterministic. The UPA check of the model guarantees
//! that the name of the next element decides which particle matches.
//! A violation names the element path, a rule code and the position in
//! the schema. It never holds a value from the message.

use super::model::{
    AttributeDecl, Builtin, ComplexType, ElementDecl, Model, Occurs, Particle, SimpleType, SrcLoc,
    TypeDef, WhiteSpace,
};
use crate::inbound::dom::{Element, Node};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;

/// The most violations that one message reports.
pub const MAX_VIOLATIONS: usize = 10;

/// One rule that a message breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Element path, for example `/Msg/Body/Item[2]/@code`.
    pub path: String,
    /// Stable rule code, for example `enumeration`.
    pub code: &'static str,
    /// Line of the element in the message (0 = unknown).
    pub line: u32,
    /// Schema file, relative to the schema directory.
    pub schema_file: String,
    /// Line of the rule in the schema file (0 = unknown).
    pub schema_line: u32,
}

/// Validates `root`. The list is empty when the message is valid. It
/// holds at most [`MAX_VIOLATIONS`] entries.
pub fn validate(model: &Model, root: &Element) -> Vec<Violation> {
    let mut w = Walk { out: Vec::new() };
    let decl = model
        .roots
        .values()
        .find(|d| d.name.local == root.local && d.name.ns == root.ns);
    match decl {
        None => {
            let path = [Seg {
                name: Cow::Borrowed(&root.local),
                idx: None,
            }];
            w.add(&path, "root_unknown", root.line, &SrcLoc::default());
        }
        Some(d) => {
            let mut path = vec![Seg {
                name: Cow::Borrowed(&root.local),
                idx: None,
            }];
            w.element(d, root, &mut path);
        }
    }
    w.out
}

const XSI_NS: &str = "http://www.w3.org/2001/XMLSchema-instance";

struct Seg<'a> {
    name: Cow<'a, str>,
    /// 1-based position among siblings of the same name, when there is
    /// more than one.
    idx: Option<u32>,
}

fn render(path: &[Seg]) -> String {
    let mut s = String::new();
    for seg in path {
        s.push('/');
        s.push_str(&seg.name);
        if let Some(i) = seg.idx {
            s.push_str(&format!("[{i}]"));
        }
    }
    s
}

struct Walk {
    out: Vec<Violation>,
}

impl Walk {
    fn full(&self) -> bool {
        self.out.len() >= MAX_VIOLATIONS
    }

    fn add(&mut self, path: &[Seg], code: &'static str, line: u32, loc: &SrcLoc) {
        if self.full() {
            return;
        }
        self.out.push(Violation {
            path: render(path),
            code,
            line,
            schema_file: loc.file.clone(),
            schema_line: loc.line,
        });
    }

    fn element<'a>(&mut self, decl: &ElementDecl, el: &'a Element, path: &mut Vec<Seg<'a>>) {
        if self.full() {
            return;
        }
        match &decl.ty {
            TypeDef::Simple(st) => {
                self.attributes(&[], el, &decl.loc, path);
                if el.has_element_children() {
                    self.add(path, "simple_content_expected", el.line, &decl.loc);
                } else {
                    self.simple(st, &el.text(), &decl.loc, el.line, path);
                }
            }
            TypeDef::Complex(ct) => {
                self.attributes(&ct.attributes, el, &decl.loc, path);
                if el
                    .children
                    .iter()
                    .any(|n| matches!(n, Node::Text(t) if !is_space(t)))
                {
                    self.add(path, "text_not_allowed", el.line, &decl.loc);
                }
                self.content(ct, el, &decl.loc, path);
            }
        }
    }

    // ---- attributes ----

    fn attributes<'a>(
        &mut self,
        declared: &[AttributeDecl],
        el: &'a Element,
        loc: &SrcLoc,
        path: &mut Vec<Seg<'a>>,
    ) {
        for a in &el.attrs {
            if self.full() {
                return;
            }
            path.push(Seg {
                name: Cow::Owned(format!("@{}", a.local)),
                idx: None,
            });
            if a.ns.as_deref() == Some(XSI_NS) {
                match a.local.as_str() {
                    "schemaLocation" | "noNamespaceSchemaLocation" => {}
                    "type" | "nil" => self.add(path, "xsi_not_allowed", el.line, loc),
                    _ => self.add(path, "attribute_unexpected", el.line, loc),
                }
            } else if let Some(d) = declared
                .iter()
                .find(|d| a.ns.is_none() && d.name == a.local)
            {
                let before = self.out.len();
                self.simple(&d.ty, &a.value, &d.loc, el.line, path);
                if self.out.len() == before {
                    if let Some(f) = &d.fixed {
                        if normalize(&d.ty, &a.value) != normalize(&d.ty, f) {
                            self.add(path, "attribute_fixed", el.line, &d.loc);
                        }
                    }
                }
            } else {
                self.add(path, "attribute_unexpected", el.line, loc);
            }
            path.pop();
        }
        for d in declared.iter().filter(|d| d.required) {
            if !el.attrs.iter().any(|a| a.ns.is_none() && a.local == d.name) {
                path.push(Seg {
                    name: Cow::Owned(format!("@{}", d.name)),
                    idx: None,
                });
                self.add(path, "attribute_missing", el.line, &d.loc);
                path.pop();
            }
        }
    }

    // ---- content model ----

    fn content<'a>(
        &mut self,
        ct: &ComplexType,
        el: &'a Element,
        loc: &SrcLoc,
        path: &mut Vec<Seg<'a>>,
    ) {
        let kids: Vec<&'a Element> = el.elements().collect();
        let segs = sibling_indexes(&kids);
        let mut pos = 0usize;
        let ok = match &ct.content {
            Some(p) => {
                let mut m = Match {
                    kids: &kids,
                    segs: &segs,
                    loc,
                    line: el.line,
                };
                self.particle(p, &mut m, &mut pos, path)
            }
            None => true,
        };
        if ok && pos < kids.len() {
            self.unexpected(&kids, &segs, pos, loc, path);
        }
    }

    fn unexpected<'a>(
        &mut self,
        kids: &[&'a Element],
        segs: &[Option<u32>],
        pos: usize,
        loc: &SrcLoc,
        path: &mut Vec<Seg<'a>>,
    ) {
        path.push(Seg {
            name: Cow::Borrowed(&kids[pos].local),
            idx: segs[pos],
        });
        self.add(path, "element_unexpected", kids[pos].line, loc);
        path.pop();
    }

    /// A required particle did not match: the next child is unexpected,
    /// or the content ends too early.
    fn fail<'a>(
        &mut self,
        m: &Match<'_, 'a>,
        pos: usize,
        path: &mut Vec<Seg<'a>>,
        decl: Option<&ElementDecl>,
    ) {
        if pos < m.kids.len() {
            self.unexpected(m.kids, m.segs, pos, m.loc, path);
        } else {
            let loc = decl.map_or(m.loc, |d| &d.loc);
            self.add(path, "element_missing", m.line, loc);
        }
    }

    fn particle<'a>(
        &mut self,
        p: &Particle,
        m: &mut Match<'_, 'a>,
        pos: &mut usize,
        path: &mut Vec<Seg<'a>>,
    ) -> bool {
        if self.full() {
            return false;
        }
        match p {
            Particle::Element { decl, occurs } => self.element_particle(decl, occurs, m, pos, path),
            Particle::Sequence { items, occurs } | Particle::Choice { items, occurs } => {
                self.group_particle(p, items, occurs, m, pos, path)
            }
        }
    }

    /// An element particle: match the element as often as the occurrence allows.
    fn element_particle<'a>(
        &mut self,
        decl: &ElementDecl,
        occurs: &Occurs,
        m: &mut Match<'_, 'a>,
        pos: &mut usize,
        path: &mut Vec<Seg<'a>>,
    ) -> bool {
        let mut count = 0u32;
        while *pos < m.kids.len()
            && occurs.max.is_none_or(|mx| count < mx)
            && names_match(decl, m.kids[*pos])
        {
            path.push(Seg {
                name: Cow::Borrowed(&m.kids[*pos].local),
                idx: m.segs[*pos],
            });
            self.element(decl, m.kids[*pos], path);
            path.pop();
            *pos += 1;
            count += 1;
        }
        if count < occurs.min {
            self.fail(m, *pos, path, Some(decl));
            return false;
        }
        true
    }

    /// A sequence or a choice. `p` is the group, `items` its particles.
    fn group_particle<'a>(
        &mut self,
        p: &Particle,
        items: &[Particle],
        occurs: &Occurs,
        m: &mut Match<'_, 'a>,
        pos: &mut usize,
        path: &mut Vec<Seg<'a>>,
    ) -> bool {
        let is_choice = matches!(p, Particle::Choice { .. });
        let mut reps = 0u32;
        loop {
            if occurs.max.is_some_and(|mx| reps >= mx) {
                break;
            }
            let starts = *pos < m.kids.len() && can_start(p, m.kids[*pos]);
            if reps >= occurs.min && !starts {
                break;
            }
            let before = *pos;
            let ok = if is_choice {
                self.choice_once(items, m, pos, path)
            } else {
                items.iter().all(|i| self.particle(i, m, pos, path))
            };
            if !ok {
                return false;
            }
            reps += 1;
            if *pos == before {
                break;
            }
        }
        true
    }

    /// One pass of a choice: the branch that can start at the current element.
    fn choice_once<'a>(
        &mut self,
        items: &[Particle],
        m: &mut Match<'_, 'a>,
        pos: &mut usize,
        path: &mut Vec<Seg<'a>>,
    ) -> bool {
        let branch = items
            .iter()
            .find(|i| *pos < m.kids.len() && can_start(i, m.kids[*pos]));
        match branch {
            Some(branch) => self.particle(branch, m, pos, path),
            None if items.iter().any(nullable) => true,
            None => {
                self.fail(m, *pos, path, None);
                false
            }
        }
    }

    // ---- simple values ----

    fn simple(&mut self, ty: &SimpleType, raw: &str, loc: &SrcLoc, line: u32, path: &[Seg]) {
        let v = normalize(ty, raw);
        if !lexical_ok(ty.base, &v) {
            self.add(path, "type_invalid", line, loc);
            return;
        }
        for step in &ty.steps {
            if let Some(en) = &step.enumeration {
                if !en.iter().any(|e| normalize(ty, e) == v) {
                    self.add(path, "enumeration", line, &step.loc);
                }
            }
            if !step.patterns.is_empty() && !step.patterns.iter().any(|r| r.is_match(&v)) {
                self.add(path, "pattern", line, &step.loc);
            }
            let n = v.chars().count();
            if step.length.is_some_and(|l| n != l) {
                self.add(path, "length", line, &step.loc);
            }
            if step.min_length.is_some_and(|l| n < l) {
                self.add(path, "min_length", line, &step.loc);
            }
            if step.max_length.is_some_and(|l| n > l) {
                self.add(path, "max_length", line, &step.loc);
            }
            if let Some(min) = &step.min_inclusive {
                if !at_least(ty.base, &v, min) {
                    self.add(path, "min_inclusive", line, &step.loc);
                }
            }
        }
    }
}

struct Match<'k, 'a> {
    kids: &'k [&'a Element],
    segs: &'k [Option<u32>],
    loc: &'k SrcLoc,
    /// Line of the parent element in the message.
    line: u32,
}

fn is_space(s: &str) -> bool {
    s.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

fn names_match(decl: &ElementDecl, el: &Element) -> bool {
    el.local == decl.name.local && el.ns == decl.name.ns
}

fn can_start(p: &Particle, el: &Element) -> bool {
    match p {
        Particle::Element { decl, .. } => names_match(decl, el),
        Particle::Choice { items, .. } => items.iter().any(|i| can_start(i, el)),
        Particle::Sequence { items, .. } => {
            for i in items {
                if can_start(i, el) {
                    return true;
                }
                if !nullable(i) {
                    return false;
                }
            }
            false
        }
    }
}

fn nullable(p: &Particle) -> bool {
    match p {
        Particle::Element { occurs, .. } => occurs.min == 0,
        Particle::Sequence { items, occurs } => occurs.min == 0 || items.iter().all(nullable),
        Particle::Choice { items, occurs } => occurs.min == 0 || items.iter().any(nullable),
    }
}

/// For each child: its 1-based position among siblings of the same
/// name, or `None` when the name is unique.
fn sibling_indexes(kids: &[&Element]) -> Vec<Option<u32>> {
    let mut total: HashMap<(&Option<String>, &str), u32> = HashMap::new();
    for k in kids {
        *total.entry((&k.ns, k.local.as_str())).or_insert(0) += 1;
    }
    let mut seen: HashMap<(&Option<String>, &str), u32> = HashMap::new();
    kids.iter()
        .map(|k| {
            let key = (&k.ns, k.local.as_str());
            let n = seen.entry(key).or_insert(0);
            *n += 1;
            (total[&key] > 1).then_some(*n)
        })
        .collect()
}

// ---- value space ----

fn normalize(ty: &SimpleType, raw: &str) -> String {
    match ty.whitespace {
        WhiteSpace::Preserve => raw.to_string(),
        WhiteSpace::Replace => raw.replace(['\t', '\n', '\r'], " "),
        WhiteSpace::Collapse => raw
            .split([' ', '\t', '\n', '\r'])
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    }
}

pub(super) fn lexical_ok(base: Builtin, v: &str) -> bool {
    match base {
        Builtin::String | Builtin::Token => true,
        Builtin::Boolean => matches!(v, "true" | "false" | "1" | "0"),
        Builtin::NonNegativeInteger => int_parts(v).is_some_and(|(neg, d)| !neg || is_zero(d)),
        Builtin::PositiveInteger => int_parts(v).is_some_and(|(neg, d)| !neg && !is_zero(d)),
        Builtin::Date => parse_dt(v, false).is_some(),
        Builtin::DateTime => parse_dt(v, true).is_some(),
    }
}

/// Sign and digits of an integer literal.
fn int_parts(v: &str) -> Option<(bool, &str)> {
    let (neg, d) = match v.as_bytes().first()? {
        b'-' => (true, &v[1..]),
        b'+' => (false, &v[1..]),
        _ => (false, v),
    };
    (!d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())).then_some((neg, d))
}

fn is_zero(d: &str) -> bool {
    d.bytes().all(|b| b == b'0')
}

fn at_least(base: Builtin, v: &str, min: &str) -> bool {
    match base {
        Builtin::NonNegativeInteger | Builtin::PositiveInteger => {
            match (int_parts(v), int_parts(min)) {
                (Some(a), Some(b)) => cmp_int(a, b) != Ordering::Less,
                _ => false,
            }
        }
        Builtin::Date | Builtin::DateTime => {
            let dt = base == Builtin::DateTime;
            match (parse_dt(v, dt), parse_dt(min, dt)) {
                (Some(a), Some(b)) => dt_at_least(&a, &b),
                _ => false,
            }
        }
        _ => true,
    }
}

fn cmp_int((an, ad): (bool, &str), (bn, bd): (bool, &str)) -> Ordering {
    let (ad, bd) = (ad.trim_start_matches('0'), bd.trim_start_matches('0'));
    let (an, bn) = (an && !ad.is_empty(), bn && !bd.is_empty());
    let mag = ad.len().cmp(&bd.len()).then_with(|| ad.cmp(bd));
    match (an, bn) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (false, false) => mag,
        (true, true) => mag.reverse(),
    }
}

/// A date or dateTime, in nanoseconds since the epoch of the local
/// time, with the time zone offset in minutes when there is one.
struct Dt {
    local_ns: i128,
    tz_min: Option<i32>,
}

const NS: i128 = 1_000_000_000;
const MAX_TZ_NS: i128 = 14 * 3600 * NS;

fn dt_at_least(v: &Dt, bound: &Dt) -> bool {
    let utc = |d: &Dt| d.local_ns - i128::from(d.tz_min.unwrap_or(0)) * 60 * NS;
    match (v.tz_min, bound.tz_min) {
        (None, None) | (Some(_), Some(_)) => utc(v) >= utc(bound),
        // XSD 1.0: the order is defined only when the instants differ by
        // more than 14 hours, whatever the time zone is.
        (None, Some(_)) => v.local_ns - MAX_TZ_NS >= utc(bound),
        (Some(_), None) => utc(v) >= bound.local_ns + MAX_TZ_NS,
    }
}

fn digits(s: &str, n: usize) -> Option<(u32, &str)> {
    let b = s.as_bytes();
    if b.len() < n || !b[..n].iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

fn parse_dt(v: &str, with_time: bool) -> Option<Dt> {
    let (neg, rest) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v),
    };
    let ylen = rest.bytes().take_while(u8::is_ascii_digit).count();
    // At most nine digits: the calendar arithmetic stays far from overflow.
    if !(4..=9).contains(&ylen) || (ylen > 4 && rest.starts_with('0')) {
        return None;
    }
    let year: i64 = rest[..ylen].parse().ok()?;
    let year = if neg { -year } else { year };
    if year == 0 {
        return None;
    }
    let rest = rest[ylen..].strip_prefix('-')?;
    let (mo, rest) = digits(rest, 2)?;
    let rest = rest.strip_prefix('-')?;
    let (d, mut rest) = digits(rest, 2)?;
    if !(1..=12).contains(&mo) || d < 1 || d > days_in_month(year, mo) {
        return None;
    }
    let (mut h, mut mi, mut s, mut frac_ns) = (0u32, 0u32, 0u32, 0i128);
    if with_time {
        rest = rest.strip_prefix('T')?;
        (h, rest) = digits(rest, 2)?;
        rest = rest.strip_prefix(':')?;
        (mi, rest) = digits(rest, 2)?;
        rest = rest.strip_prefix(':')?;
        (s, rest) = digits(rest, 2)?;
        if let Some(r) = rest.strip_prefix('.') {
            let n = r.bytes().take_while(u8::is_ascii_digit).count();
            if n == 0 {
                return None;
            }
            let mut f = r[..n].to_string();
            f.truncate(9);
            while f.len() < 9 {
                f.push('0');
            }
            frac_ns = f.parse().ok()?;
            rest = &r[n..];
        }
        if h > 24 || mi > 59 || s > 59 || (h == 24 && (mi != 0 || s != 0 || frac_ns != 0)) {
            return None;
        }
    }
    let tz_min = parse_tz(rest)?;
    let days = days_from_civil(year, mo, d);
    let secs =
        i128::from(days) * 86_400 + i128::from(h) * 3600 + i128::from(mi) * 60 + i128::from(s);
    Some(Dt {
        local_ns: secs * NS + frac_ns,
        tz_min,
    })
}

/// `Some(None)`: no zone. `Some(Some(min))`: a zone. `None`: malformed.
fn parse_tz(s: &str) -> Option<Option<i32>> {
    if s.is_empty() {
        return Some(None);
    }
    if s == "Z" {
        return Some(Some(0));
    }
    let sign = match s.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (h, r) = digits(&s[1..], 2)?;
    let (m, r) = digits(r.strip_prefix(':')?, 2)?;
    if !r.is_empty() || h > 14 || m > 59 || (h == 14 && m != 0) {
        return None;
    }
    Some(Some(sign * (h as i32 * 60 + m as i32)))
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if y.rem_euclid(4) == 0 && (y.rem_euclid(100) != 0 || y.rem_euclid(400) == 0) => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 (proleptic Gregorian calendar).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbound::dom;
    use std::path::PathBuf;

    const NS: &str = "urn:t";

    fn no_files(_: &str) -> Option<(String, PathBuf, String)> {
        None
    }

    fn model(types_and_root: &str) -> Model {
        let xsd = format!(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:t="{NS}" targetNamespace="{NS}" elementFormDefault="qualified">{types_and_root}</xs:schema>"#
        );
        super::super::build(&[("m.xsd".into(), xsd)], &no_files, &["Msg"])
            .unwrap_or_else(|p| panic!("model must build: {p:?}"))
    }

    fn check(m: &Model, xml: &str) -> Vec<Violation> {
        let root = dom::parse_message(xml).expect("message must parse");
        validate(m, &root)
    }

    fn codes(m: &Model, xml: &str) -> Vec<&'static str> {
        check(m, xml).iter().map(|v| v.code).collect()
    }

    /// A message root in the schema namespace.
    fn doc(inner: &str) -> String {
        format!(r#"<Msg xmlns="{NS}">{inner}</Msg>"#)
    }

    const SIMPLE: &str = r#"
      <xs:simpleType name="Code"><xs:restriction base="xs:token">
        <xs:whiteSpace value="collapse"/><xs:enumeration value="A"/><xs:enumeration value="B C"/>
      </xs:restriction></xs:simpleType>
      <xs:element name="Msg"><xs:complexType><xs:sequence>
        <xs:element name="code" type="t:Code"/>
        <xs:element name="n" type="xs:nonNegativeInteger" minOccurs="0"/>
      </xs:sequence></xs:complexType></xs:element>"#;

    #[test]
    fn a_valid_message_has_no_violations() {
        let m = model(SIMPLE);
        assert!(check(&m, &doc("<code>A</code><n>5</n>")).is_empty());
        assert!(check(&m, &doc("<code>A</code>")).is_empty());
    }

    #[test]
    fn a_root_that_the_model_does_not_know_is_a_violation() {
        let m = model(SIMPLE);
        assert_eq!(
            codes(&m, r#"<Other xmlns="urn:t"><code>A</code></Other>"#),
            ["root_unknown"]
        );
        assert_eq!(
            codes(&m, r#"<Msg xmlns="urn:other"><code>A</code></Msg>"#),
            ["root_unknown"]
        );
    }

    #[test]
    fn a_missing_required_element_is_a_violation() {
        let m = model(SIMPLE);
        let v = check(&m, &doc(""));
        assert_eq!(
            v.iter().map(|v| v.code).collect::<Vec<_>>(),
            ["element_missing"]
        );
        assert_eq!(v[0].path, "/Msg");
        assert_eq!(v[0].schema_file, "m.xsd");
        assert!(v[0].schema_line > 0);
    }

    #[test]
    fn an_unexpected_element_or_a_wrong_order_is_a_violation() {
        let m = model(SIMPLE);
        let v = check(&m, &doc("<code>A</code><x/>"));
        assert_eq!(v[0].code, "element_unexpected");
        assert_eq!(v[0].path, "/Msg/x");
        assert_eq!(
            codes(&m, &doc("<n>1</n><code>A</code>"))[0],
            "element_unexpected"
        );
        assert_eq!(
            codes(&m, &doc("<code>A</code><n>1</n><n>2</n>"))[0],
            "element_unexpected"
        );
        assert_eq!(
            codes(
                &m,
                r#"<Msg xmlns="urn:t"><code xmlns="urn:other">A</code></Msg>"#
            )[0],
            "element_unexpected"
        );
    }

    #[test]
    fn occurrence_limits_are_checked() {
        let m = model(
            r#"<xs:element name="Msg"><xs:complexType><xs:sequence>
                 <xs:element name="i" type="xs:string" minOccurs="2" maxOccurs="3"/>
               </xs:sequence></xs:complexType></xs:element>"#,
        );
        assert_eq!(codes(&m, &doc("<i>a</i>")), ["element_missing"]);
        assert!(check(&m, &doc("<i>a</i><i>b</i>")).is_empty());
        assert!(check(&m, &doc("<i>a</i><i>b</i><i>c</i>")).is_empty());
        assert_eq!(
            codes(&m, &doc("<i>a</i><i>b</i><i>c</i><i>d</i>"))[0],
            "element_unexpected"
        );
    }

    #[test]
    fn a_choice_takes_exactly_one_branch() {
        let m = model(
            r#"<xs:element name="Msg"><xs:complexType><xs:sequence>
                 <xs:choice>
                   <xs:element name="a" type="xs:string"/>
                   <xs:sequence><xs:element name="b" type="xs:string"/><xs:element name="c" type="xs:string"/></xs:sequence>
                 </xs:choice>
                 <xs:element name="end" type="xs:string"/>
               </xs:sequence></xs:complexType></xs:element>"#,
        );
        assert!(check(&m, &doc("<a>1</a><end>e</end>")).is_empty());
        assert!(check(&m, &doc("<b>1</b><c>2</c><end>e</end>")).is_empty());
        assert!(!check(&m, &doc("<end>e</end>")).is_empty());
        assert!(!check(&m, &doc("<a>1</a><b>1</b><c>2</c><end>e</end>")).is_empty());
        assert!(!check(&m, &doc("<b>1</b><end>e</end>")).is_empty());
    }

    #[test]
    fn a_repeated_group_matches_each_repetition() {
        let m = model(
            r#"<xs:element name="Msg"><xs:complexType><xs:sequence maxOccurs="unbounded">
                 <xs:element name="k" type="xs:string"/><xs:element name="v" type="xs:string" minOccurs="0"/>
               </xs:sequence></xs:complexType></xs:element>"#,
        );
        assert!(check(&m, &doc("<k>1</k><v>a</v><k>2</k><k>3</k><v>c</v>")).is_empty());
        assert!(!check(&m, &doc("<v>a</v>")).is_empty());
        assert!(!check(&m, &doc("")).is_empty());
    }

    #[test]
    fn extension_content_is_the_base_content_then_the_own_content() {
        let m = model(
            r#"<xs:complexType name="Base"><xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
                 <xs:attribute name="id" type="xs:string" use="required"/></xs:complexType>
               <xs:element name="Msg"><xs:complexType><xs:complexContent>
                 <xs:extension base="t:Base"><xs:sequence><xs:element name="b" type="xs:string"/></xs:sequence>
                   <xs:attribute name="x" type="xs:string"/></xs:extension>
               </xs:complexContent></xs:complexType></xs:element>"#,
        );
        let ok = format!(r#"<Msg xmlns="{NS}" id="1" x="2"><a>1</a><b>2</b></Msg>"#);
        assert!(check(&m, &ok).is_empty());
        let wrong = format!(r#"<Msg xmlns="{NS}" id="1"><b>2</b><a>1</a></Msg>"#);
        assert!(!check(&m, &wrong).is_empty());
        let no_id = format!(r#"<Msg xmlns="{NS}"><a>1</a><b>2</b></Msg>"#);
        assert_eq!(codes(&m, &no_id), ["attribute_missing"]);
    }

    fn typed(ty: &str) -> Model {
        model(&format!(
            r#"{ty}<xs:element name="Msg"><xs:complexType><xs:sequence>
               <xs:element name="v" type="t:V"/></xs:sequence></xs:complexType></xs:element>"#
        ))
    }

    fn value_codes(m: &Model, v: &str) -> Vec<&'static str> {
        codes(m, &doc(&format!("<v>{v}</v>")))
    }

    #[test]
    fn enumeration_and_white_space_apply_in_this_order() {
        let m = typed(
            SIMPLE
                .split("<xs:element")
                .next()
                .unwrap()
                .replace("\"Code\"", "\"V\"")
                .as_str(),
        );
        assert!(value_codes(&m, "A").is_empty());
        assert!(value_codes(&m, "  B   C  ").is_empty());
        assert_eq!(value_codes(&m, "B  D"), ["enumeration"]);
    }

    #[test]
    fn pattern_and_length_facets() {
        let m = typed(
            r#"<xs:simpleType name="V"><xs:restriction base="xs:string">
                 <xs:pattern value="[A-Z]{2}\d{3}"/><xs:pattern value="XX.*"/>
                 <xs:minLength value="2"/><xs:maxLength value="5"/>
               </xs:restriction></xs:simpleType>"#,
        );
        assert!(value_codes(&m, "AB123").is_empty());
        assert!(
            value_codes(&m, "XX").is_empty(),
            "patterns of one step are OR"
        );
        assert_eq!(value_codes(&m, "ab123"), ["pattern"]);
        assert_eq!(value_codes(&m, "XXXXXX"), ["max_length"]);
        assert_eq!(value_codes(&m, "X"), ["pattern", "min_length"]);
        let l = typed(
            r#"<xs:simpleType name="V"><xs:restriction base="xs:string"><xs:length value="3"/></xs:restriction></xs:simpleType>"#,
        );
        assert!(
            value_codes(&l, "a\u{e4}\u{20ac}").is_empty(),
            "length counts characters"
        );
        assert_eq!(value_codes(&l, "ab"), ["length"]);
    }

    #[test]
    fn patterns_of_different_derivation_steps_are_and() {
        let m = typed(
            r#"<xs:simpleType name="B"><xs:restriction base="xs:string"><xs:pattern value="[a-z]+"/></xs:restriction></xs:simpleType>
               <xs:simpleType name="V"><xs:restriction base="t:B"><xs:pattern value=".{3}"/></xs:restriction></xs:simpleType>"#,
        );
        assert!(value_codes(&m, "abc").is_empty());
        assert_eq!(value_codes(&m, "abcd"), ["pattern"]);
        assert_eq!(value_codes(&m, "ab1"), ["pattern"]);
    }

    #[test]
    fn built_in_types_check_the_lexical_form() {
        let cases: [(&str, &[&str], &[&str]); 6] = [
            (
                "boolean",
                &["true", "false", "1", "0", " true "],
                &["yes", "TRUE", ""],
            ),
            (
                "nonNegativeInteger",
                &["0", "7", "+5", "-0", "007"],
                &["-1", "1.5", "a", ""],
            ),
            (
                "positiveInteger",
                &["1", "+5", "0010"],
                &["0", "-1", "00", ""],
            ),
            (
                "date",
                &[
                    "2026-11-12",
                    "2024-02-29",
                    "2026-11-12Z",
                    "2026-11-12+02:00",
                    "-0044-03-15",
                ],
                &[
                    "2026-02-30",
                    "2025-02-29",
                    "2026-13-01",
                    "26-11-12",
                    "2026-11-12T10:00:00Z",
                    "2026-11-12+15:00",
                ],
            ),
            (
                "dateTime",
                &[
                    "2026-11-12T10:00:00Z",
                    "2026-11-12T10:00:00.123+02:00",
                    "2026-11-12T24:00:00",
                    "2026-11-12T10:00:00",
                ],
                &[
                    "2026-11-12",
                    "2026-11-12T25:00:00",
                    "2026-11-12T10:00",
                    "2026-11-12T24:00:01",
                    "2026-11-12 10:00:00Z",
                ],
            ),
            ("string", &["anything", ""], &[]),
        ];
        for (ty, good, bad) in cases {
            let m = typed(&format!(
                r#"<xs:simpleType name="V"><xs:restriction base="xs:{ty}"/></xs:simpleType>"#
            ));
            for v in good {
                assert!(value_codes(&m, v).is_empty(), "{ty}: {v:?} must be valid");
            }
            for v in bad {
                assert_eq!(
                    value_codes(&m, v),
                    ["type_invalid"],
                    "{ty}: {v:?} must be invalid"
                );
            }
        }
    }

    #[test]
    fn min_inclusive_compares_by_value() {
        let m = typed(
            r#"<xs:simpleType name="V"><xs:restriction base="xs:dateTime">
                 <xs:minInclusive value="1753-01-01T00:00:00Z"/></xs:restriction></xs:simpleType>"#,
        );
        assert!(value_codes(&m, "1753-01-01T00:00:00Z").is_empty());
        assert!(value_codes(&m, "1753-01-01T02:00:00+02:00").is_empty());
        assert!(value_codes(&m, "2026-01-01T00:00:00").is_empty());
        assert_eq!(value_codes(&m, "1752-12-31T23:59:59Z"), ["min_inclusive"]);
        assert_eq!(
            value_codes(&m, "1753-01-01T00:00:00+02:00"),
            ["min_inclusive"]
        );
        let n = typed(
            r#"<xs:simpleType name="V"><xs:restriction base="xs:positiveInteger">
                 <xs:minInclusive value="10"/></xs:restriction></xs:simpleType>"#,
        );
        assert!(value_codes(&n, "10").is_empty() && value_codes(&n, "100").is_empty());
        assert_eq!(value_codes(&n, "9"), ["min_inclusive"]);
    }

    #[test]
    fn attributes_are_checked() {
        let m = model(
            r#"<xs:element name="Msg"><xs:complexType><xs:sequence/>
                 <xs:attribute name="req" type="xs:string" use="required"/>
                 <xs:attribute name="ver" type="xs:string" fixed="3.5"/>
                 <xs:attribute name="n" type="xs:nonNegativeInteger"/>
               </xs:complexType></xs:element>"#,
        );
        let msg = |a: &str| format!(r#"<Msg xmlns="{NS}" {a}/>"#);
        assert!(check(&m, &msg(r#"req="x" ver="3.5" n="1""#)).is_empty());
        assert!(check(&m, &msg(r#"req="x""#)).is_empty());
        assert_eq!(codes(&m, &msg("")), ["attribute_missing"]);
        assert_eq!(codes(&m, &msg(r#"req="x" ver="3.6""#)), ["attribute_fixed"]);
        assert_eq!(codes(&m, &msg(r#"req="x" n="-1""#)), ["type_invalid"]);
        assert_eq!(
            codes(&m, &msg(r#"req="x" other="1""#)),
            ["attribute_unexpected"]
        );
        let v = check(&m, &msg(r#"req="x" other="1""#));
        assert_eq!(v[0].path, "/Msg/@other");
    }

    #[test]
    fn xsi_type_and_nil_are_refused_and_schema_location_is_ignored() {
        let m = model(
            r#"<xs:element name="Msg"><xs:complexType><xs:sequence/></xs:complexType></xs:element>"#,
        );
        let xsi = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;
        let msg = |a: &str| format!(r#"<Msg xmlns="{NS}" {xsi} {a}/>"#);
        assert_eq!(
            codes(&m, &msg(r#"xsi:type="t:Other""#)),
            ["xsi_not_allowed"]
        );
        assert_eq!(codes(&m, &msg(r#"xsi:nil="true""#)), ["xsi_not_allowed"]);
        assert!(check(&m, &msg(r#"xsi:schemaLocation="urn:t http://x/y.xsd""#)).is_empty());
        assert!(check(&m, &msg(r#"xsi:noNamespaceSchemaLocation="y.xsd""#)).is_empty());
    }

    #[test]
    fn text_is_valid_only_where_the_type_allows_it() {
        let m = model(
            r#"<xs:element name="Msg"><xs:complexType><xs:sequence>
                 <xs:element name="a" type="xs:string"/>
                 <xs:element name="e"><xs:complexType/></xs:element>
               </xs:sequence></xs:complexType></xs:element>"#,
        );
        assert!(check(&m, &doc("\n  <a>x</a>\n  <e/>\n")).is_empty());
        assert_eq!(codes(&m, &doc("text<a>x</a><e/>")), ["text_not_allowed"]);
        assert_eq!(codes(&m, &doc("<a>x</a><e>text</e>")), ["text_not_allowed"]);
        assert_eq!(
            codes(&m, &doc("<a><b/></a><e/>")),
            ["simple_content_expected"]
        );
    }

    #[test]
    fn a_violation_never_holds_a_value_or_an_absolute_path() {
        let m = typed(
            r#"<xs:simpleType name="V"><xs:restriction base="xs:token"><xs:enumeration value="A"/></xs:restriction></xs:simpleType>"#,
        );
        let v = check(&m, &doc("<v>SECRET-4711</v>"));
        assert_eq!(v.len(), 1);
        let text = format!("{v:?}");
        assert!(!text.contains("SECRET"), "{text}");
        assert!(!v[0].schema_file.contains('/'), "{}", v[0].schema_file);
        let a = model(
            r#"<xs:element name="Msg"><xs:complexType><xs:sequence/></xs:complexType></xs:element>"#,
        );
        let v = check(&a, &format!(r#"<Msg xmlns="{NS}" SECRET-4711="1"/>"#));
        assert_eq!(v[0].code, "attribute_unexpected");
        assert!(!v[0].path.contains("SECRET-4711") || v[0].path.ends_with("@SECRET-4711"));
    }

    #[test]
    fn repeated_siblings_get_an_index_in_the_path() {
        let m = model(
            r#"<xs:element name="Msg"><xs:complexType><xs:sequence>
                 <xs:element name="i" maxOccurs="unbounded"><xs:complexType><xs:sequence/>
                   <xs:attribute name="c" type="xs:nonNegativeInteger"/></xs:complexType></xs:element>
               </xs:sequence></xs:complexType></xs:element>"#,
        );
        let v = check(&m, &doc(r#"<i c="1"/><i c="x"/>"#));
        assert_eq!(v[0].path, "/Msg/i[2]/@c");
        let v = check(&m, &doc(r#"<i c="x"/>"#));
        assert_eq!(v[0].path, "/Msg/i/@c");
    }

    #[test]
    fn a_message_reports_at_most_ten_violations() {
        let m = model(
            r#"<xs:element name="Msg"><xs:complexType><xs:sequence>
                 <xs:element name="i" type="xs:nonNegativeInteger" maxOccurs="unbounded"/>
               </xs:sequence></xs:complexType></xs:element>"#,
        );
        let items = "<i>x</i>".repeat(50);
        assert_eq!(check(&m, &doc(&items)).len(), MAX_VIOLATIONS);
    }

    #[test]
    fn the_message_line_is_reported() {
        let m = typed(
            r#"<xs:simpleType name="V"><xs:restriction base="xs:token"><xs:enumeration value="A"/></xs:restriction></xs:simpleType>"#,
        );
        let v = check(&m, &doc("\n\n<v>B</v>"));
        assert_eq!(v[0].line, 3);
    }

    #[test]
    fn a_year_has_at_most_nine_digits() {
        let m =
            typed(r#"<xs:simpleType name="V"><xs:restriction base="xs:date"/></xs:simpleType>"#);
        assert!(value_codes(&m, "123456789-01-01").is_empty());
        assert_eq!(value_codes(&m, "1234567890-01-01"), ["type_invalid"]);
        assert_eq!(
            value_codes(&m, "9999999999999999999-01-01"),
            ["type_invalid"]
        );
        let m = typed(
            r#"<xs:simpleType name="V"><xs:restriction base="xs:dateTime">
                 <xs:minInclusive value="1753-01-01T00:00:00Z"/></xs:restriction></xs:simpleType>"#,
        );
        assert_eq!(
            value_codes(&m, "9999999999999999999-01-01T00:00:00Z"),
            ["type_invalid"]
        );
    }

    #[test]
    fn element_missing_reports_the_line_of_the_parent_element() {
        let m = model(SIMPLE);
        let v = check(&m, &doc("\n\n"));
        assert_eq!(v[0].code, "element_missing");
        assert_eq!(v[0].line, 1, "the parent starts on line 1");
        let n = model(
            r#"<xs:element name="Msg"><xs:complexType><xs:sequence>
                 <xs:element name="in"><xs:complexType><xs:sequence>
                   <xs:element name="a" type="xs:string"/>
                 </xs:sequence></xs:complexType></xs:element>
               </xs:sequence></xs:complexType></xs:element>"#,
        );
        let v = check(&n, &doc("\n<in>\n</in>"));
        assert_eq!(
            (v[0].code, v[0].path.as_str(), v[0].line),
            ("element_missing", "/Msg/in", 2)
        );
    }
}
