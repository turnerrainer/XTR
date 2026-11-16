//! Differential test: the Rust XSD validator against Apache Xerces.
//!
//! Xerces runs only here, in a container without network. See
//! `tests/xsd-oracle/README.md`.
//!
//! Environment variables:
//! * `XTR_XSD_ORACLE_REQUIRED=1`: a missing Docker, jar, schema directory or
//!   sample set is an error and not a skip. Zero compared cases is an error.
//! * `XTR_ERRU_XSD_DIR`: directory with the ERRU XSD files (differential test).
//! * `XTR_ORACLE_SAMPLES_DIR`: directory with valid sample messages.
//!   The test never writes generated messages to the repository.
//! * `XTR_ORACLE_DIR`: directory of the oracle scripts (default: next to this file).

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use xtr_on_rust::inbound::dom::{self, DomErrorKind};
use xtr_on_rust::xsdval::{build, validate, Model};

// ---------------------------------------------------------------- oracle

fn required() -> bool {
    std::env::var("XTR_XSD_ORACLE_REQUIRED").is_ok_and(|v| v == "1")
}

fn oracle_dir() -> PathBuf {
    std::env::var_os("XTR_ORACLE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/xsd-oracle"))
}

/// Skip with a clear message, or fail when `XTR_XSD_ORACLE_REQUIRED=1`.
fn unavailable(why: &str) {
    if required() {
        panic!("xsd-oracle required but not available: {why}");
    }
    eprintln!("xsd-oracle skipped: {why}");
}

/// Runs the oracle script. `None` means "skipped" (message already printed).
fn run_oracle(args: &[&Path]) -> Option<Value> {
    let script = oracle_dir().join("run-oracle.sh");
    if !script.is_file() {
        unavailable("run-oracle.sh not found");
        return None;
    }
    let out = Command::new("sh")
        .arg(&script)
        .args(args)
        .output()
        .expect("cannot start sh");
    match out.status.code() {
        Some(0) => Some(serde_json::from_slice(&out.stdout).expect("oracle JSON")),
        Some(10) => {
            unavailable(String::from_utf8_lossy(&out.stderr).trim());
            None
        }
        _ => panic!(
            "oracle failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

fn oracle(xsd: &Path, msgs: &Path) -> Option<Value> {
    run_oracle(&[xsd, msgs])
}

fn mapping() -> (BTreeMap<String, Vec<String>>, BTreeSet<String>) {
    let text = std::fs::read_to_string(oracle_dir().join("rule-classes.json")).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    let map = v["mapping"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            let list = v
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect();
            (k.clone(), list)
        })
        .collect();
    let wrappers = v["wrappers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    (map, wrappers)
}

fn expected_difference_ids() -> BTreeSet<String> {
    let text = std::fs::read_to_string(oracle_dir().join("expected-differences")).unwrap();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.split(':').next().unwrap().trim().to_string())
        .collect()
}

/// Closed list of differences that the owner approved on purpose
/// (`owner-approved-accept`). The report counts them apart.
fn owner_approved_ids() -> BTreeSet<String> {
    let text = std::fs::read_to_string(oracle_dir().join("owner-approved-accept")).unwrap();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.split(':').next().unwrap().trim().to_string())
        .collect()
}

// ------------------------------------------------------------------ Rust side

#[derive(Debug)]
enum Rust {
    /// The message was refused before validation.
    Refused(DomErrorKind, String),
    /// Violations as (path, code). Empty = valid.
    Done(Vec<(String, String)>),
}

struct Models<'a> {
    xsd_dir: &'a Path,
    cache: HashMap<(String, String), Model>,
}

impl<'a> Models<'a> {
    fn new(xsd_dir: &'a Path) -> Self {
        Models {
            xsd_dir,
            cache: HashMap::new(),
        }
    }

    fn check(&mut self, schema: &str, bytes: &[u8]) -> Rust {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return Rust::Refused(DomErrorKind::Encoding, "not UTF-8".into());
        };
        let root = match dom::parse_message(text) {
            Ok(r) => r,
            Err(e) => return Rust::Refused(e.1, e.0),
        };
        let key = (schema.to_string(), root.local.clone());
        if !self.cache.contains_key(&key) {
            let loader = xtr_on_rust::inbound::contract::fs_loader(self.xsd_dir);
            let xml = std::fs::read_to_string(self.xsd_dir.join(schema)).unwrap();
            let model = build(
                &[(schema.to_string(), xml)],
                &loader,
                &[root.local.as_str()],
            )
            .unwrap_or_else(|p| panic!("XTR model of {schema} does not build: {p:?}"));
            self.cache.insert(key.clone(), model);
        }
        let v = validate(&self.cache[&key], &root);
        Rust::Done(
            v.into_iter()
                .map(|x| (x.path, x.code.to_string()))
                .collect(),
        )
    }
}

// ------------------------------------------------------------------ comparison

#[derive(Default, Debug)]
struct Report {
    compared: usize,
    allowed: BTreeMap<String, usize>,
    /// Hits of `owner-approved-accept`, counted apart from `allowed`.
    owner_approved: BTreeMap<String, usize>,
    /// Files that hit `utf8_label`. The oracle runs them again with the label replaced.
    utf8_files: Vec<String>,
    /// XTR result (`Debug` text) of each `utf8_label` file before the label is replaced.
    utf8_rust: BTreeMap<String, String>,
    /// A strict report allows no owner-approved difference.
    strict: bool,
    diffs: Vec<String>,
}

/// The first XML declaration of a message, when there is one.
fn xml_decl(text: &str) -> Option<&str> {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = t.strip_prefix("<?xml")?;
    let end = rest.find("?>")?;
    Some(&rest[..end])
}

/// True when the declaration has exactly `encoding="utf8"` (any case, either quote).
fn has_utf8_alias_label(text: &str) -> bool {
    let Some(decl) = xml_decl(text) else {
        return false;
    };
    regex::Regex::new(r#"(?i-u)\sencoding\s*=\s*(?:"utf8"|'utf8')"#)
        .unwrap()
        .is_match(decl)
}

/// The message with the label `utf8` replaced by `UTF-8` in the declaration.
fn replace_utf8_label(text: &str) -> String {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bom = &text[..text.len() - t.len()];
    let Some(decl) = xml_decl(t) else {
        return text.to_string();
    };
    let new_decl = regex::Regex::new(r#"(?i-u)(\sencoding\s*=\s*)(["'])utf8(["'])"#)
        .unwrap()
        .replace(decl, "${1}${2}UTF-8${3}")
        .into_owned();
    format!("{bom}<?xml{new_decl}{}", &t["<?xml".len() + decl.len()..])
}

/// True when the message has a date or dateTime shape with a year of 10 or more digits.
fn has_long_year(text: &str) -> bool {
    regex::Regex::new(r"(^|[^0-9-])-?[0-9]{10,}-[0-9]{2}-[0-9]{2}")
        .unwrap()
        .is_match(text)
}

/// Compares one oracle result with the XTR result. Differences go to the report.
fn compare(rep: &mut Report, r: &Value, rust: &Rust, text: &str, allowed_ids: &BTreeSet<String>) {
    let (map, wrappers) = mapping();
    let file = r["file"].as_str().unwrap();
    let verdict = r["verdict"].as_str().unwrap();
    let cn = r["cn_chars"].as_u64().unwrap_or(0);
    let errors = r["errors"].as_array().unwrap();
    rep.compared += 1;
    let allow = |rep: &mut Report, id: &str, why: &str| {
        if allowed_ids.contains(id) {
            *rep.allowed.entry(id.to_string()).or_default() += 1;
        } else {
            rep.diffs
                .push(format!("{file}: {why} ({id} is not in the list)"));
        }
    };
    for e in errors {
        let c = e["code"].as_str().unwrap();
        if !map.contains_key(c) && !wrappers.contains(c) {
            rep.diffs
                .push(format!("{file}: Xerces code {c} has no mapping"));
        }
    }
    match (verdict, rust) {
        ("refused" | "not_wellformed", Rust::Refused(..)) => {}
        ("refused" | "not_wellformed", Rust::Done(_)) => {
            let owner = owner_approved_ids();
            if verdict == "not_wellformed"
                && !rep.strict
                && owner.contains("utf8_label")
                && has_utf8_alias_label(text)
            {
                *rep.owner_approved.entry("utf8_label".into()).or_default() += 1;
                rep.utf8_files.push(file.to_string());
                rep.utf8_rust.insert(file.to_string(), format!("{rust:?}"));
            } else {
                rep.diffs.push(format!(
                    "{file}: Xerces says {verdict}, XTR accepted the message"
                ));
            }
        }
        ("valid" | "invalid" | "no_schema", Rust::Refused(kind, text)) => {
            if matches!(kind, DomErrorKind::Encoding) {
                allow(rep, "non_utf8_declaration", "XTR refuses the encoding");
            } else if text.starts_with("XML declaration") {
                allow(rep, "strict_prolog", "XTR refuses the prolog");
            } else {
                rep.diffs.push(format!(
                    "{file}: XTR refused ({text}), Xerces says {verdict}"
                ));
            }
        }
        ("no_schema", Rust::Done(_)) => {}
        ("valid", Rust::Done(v)) if v.is_empty() => {}
        ("valid", Rust::Done(v)) => {
            if v[0].1 == "type_invalid" && has_long_year(text) {
                allow(
                    rep,
                    "year_over_nine_digits",
                    "XTR refuses a long year, Xerces accepts",
                );
            } else if cn > 0 {
                allow(rep, "unicode_unassigned", "XTR refuses, Xerces accepts");
            } else {
                rep.diffs
                    .push(format!("{file}: Xerces says valid, XTR reports {}", v[0].1));
            }
        }
        ("invalid", Rust::Done(v)) if v.is_empty() => {
            rep.diffs.push(format!(
                "{file}: Xerces says invalid ({}), XTR says valid",
                errors[0]["code"].as_str().unwrap()
            ));
        }
        ("invalid", Rust::Done(v)) => {
            let first = &errors[0];
            let code = first["code"].as_str().unwrap();
            let path = first["path"].as_str().unwrap();
            let ok_codes = map.get(code).cloned().unwrap_or_default();
            if !ok_codes.contains(&v[0].1) {
                rep.diffs.push(format!(
                    "{file}: class differs: Xerces {code}, XTR {}",
                    v[0].1
                ));
            } else if v[0].0 != path {
                rep.diffs.push(format!(
                    "{file}: path differs for {code}: Xerces {path}, XTR {}",
                    v[0].0
                ));
            }
        }
        (other, _) => rep.diffs.push(format!("{file}: unknown verdict {other}")),
    }
}

/// U1: for each message that used the `utf8_label` exception, the oracle replaces the label
/// with UTF-8 and runs Xerces again. The verdict and the rule class must then agree in full.
fn rerun_utf8_label(xsd: &Path, msgs: &Path, rep: &mut Report) {
    if rep.utf8_files.is_empty() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    for f in &rep.utf8_files {
        let text = String::from_utf8_lossy(&std::fs::read(msgs.join(f)).unwrap()).into_owned();
        std::fs::write(tmp.path().join(f), replace_utf8_label(&text)).unwrap();
    }
    let Some(out) = oracle(xsd, tmp.path()) else {
        // The first run had the oracle. A missing oracle now is a fault, not a skip.
        rep.diffs
            .push("utf8_label rerun: the oracle gave no result for the changed messages".into());
        return;
    };
    let results = out["results"].as_array().unwrap();
    if results.len() != rep.utf8_files.len() {
        rep.diffs.push(format!(
            "utf8_label rerun: {} results for {} messages",
            results.len(),
            rep.utf8_files.len()
        ));
    }
    let mut models = Models::new(xsd);
    let mut strict = Report {
        strict: true,
        ..Report::default()
    };
    let none = BTreeSet::new();
    for r in results {
        let f = r["file"].as_str().unwrap();
        let bytes = std::fs::read(tmp.path().join(f)).unwrap();
        let rust = match r["schema"].as_str() {
            Some(schema) => models.check(schema, &bytes),
            None => Rust::Done(vec![]),
        };
        // The change of the label must not change what XTR says.
        if rep.utf8_rust.get(f).map(String::as_str) != Some(format!("{rust:?}").as_str()) {
            rep.diffs.push(format!(
                "utf8_label rerun: XTR gives another result for {f} after the label is replaced"
            ));
        }
        compare(
            &mut strict,
            r,
            &rust,
            &String::from_utf8_lossy(&bytes),
            &none,
        );
    }
    for d in strict.diffs {
        rep.diffs
            .push(format!("utf8_label rerun (label replaced by UTF-8): {d}"));
    }
    assert!(
        strict.allowed.is_empty(),
        "the rerun must agree in full, without any allowed difference: {:?}",
        strict.allowed
    );
}

/// Runs the oracle and XTR on a message directory and compares them.
fn differential(xsd: &Path, msgs: &Path) -> Option<Report> {
    let out = oracle(xsd, msgs)?;
    for s in out["schemas"].as_array().unwrap() {
        assert_eq!(
            s["status"], "ok",
            "oracle cannot compile {}: {}",
            s["file"], s["class"]
        );
    }
    let ids = expected_difference_ids();
    let mut models = Models::new(xsd);
    let mut rep = Report::default();
    for r in out["results"].as_array().unwrap() {
        let bytes = std::fs::read(msgs.join(r["file"].as_str().unwrap())).unwrap();
        let rust = match r["schema"].as_str() {
            Some(schema) => models.check(schema, &bytes),
            None => match std::str::from_utf8(&bytes).map(dom::parse_message) {
                Ok(Err(e)) => Rust::Refused(e.1, e.0),
                Err(_) => Rust::Refused(DomErrorKind::Encoding, "not UTF-8".into()),
                Ok(Ok(_)) => Rust::Done(vec![]),
            },
        };
        compare(&mut rep, r, &rust, &String::from_utf8_lossy(&bytes), &ids);
    }
    rerun_utf8_label(xsd, msgs, &mut rep);
    Some(rep)
}

fn selftest_dir() -> PathBuf {
    oracle_dir().join("selftest")
}

fn assert_clean(rep: &Report) {
    assert!(
        rep.diffs.is_empty(),
        "differences:\n{}",
        rep.diffs.join("\n")
    );
}

// ------------------------------------------------------------------ tests: oracle

#[test]
fn oracle_prints_both_unicode_versions() {
    let Some(info) = run_oracle(&[Path::new("--info")]) else {
        return;
    };
    // Probes: first letters added in Unicode 14, 15, 15.1 and 16.
    let probes = ["2C2F", "31350", "2EBF0", "10D50"];
    let re = regex::Regex::new(r"^\p{L}$").unwrap();
    let mut line = String::new();
    for p in probes {
        let c = char::from_u32(u32::from_str_radix(p, 16).unwrap()).unwrap();
        line += &format!(
            " U+{p}: jdk={} regex-crate={};",
            info["unicode_probe"][p].as_str().unwrap(),
            if re.is_match(&c.to_string()) {
                "letter"
            } else {
                "not-letter"
            }
        );
    }
    eprintln!(
        "oracle {} on Java {}.{line}",
        info["oracle"].as_str().unwrap(),
        info["java"].as_str().unwrap()
    );
}

#[test]
fn selftest_oracle_verdicts_match_the_file_names() {
    let dir = selftest_dir();
    let msgs = dir.join("messages");
    let Some(out) = oracle(&dir, &msgs) else {
        return;
    };
    for s in out["schemas"].as_array().unwrap() {
        assert_eq!(s["status"], "ok", "{s}");
    }
    let results = out["results"].as_array().unwrap();
    assert!(results.len() >= 20, "too few self-test messages");
    for r in results {
        let f = r["file"].as_str().unwrap();
        let expect = if f == "bad_root_unknown.xml" {
            "no_schema"
        } else if f.starts_with("ok_") || f.starts_with("diff_") {
            "valid"
        } else if f.starts_with("bad_") {
            "invalid"
        } else if f.starts_with("refused_") {
            "refused"
        } else if f.starts_with("malformed") {
            "not_wellformed"
        } else {
            panic!("unknown file name {f}")
        };
        assert_eq!(r["verdict"], expect, "{f}");
    }
}

#[test]
fn selftest_rust_validator_agrees_with_xerces() {
    let dir = selftest_dir();
    let Some(rep) = differential(&dir, &dir.join("messages")) else {
        return;
    };
    eprintln!(
        "compared {}, allowed {:?}, owner approved {:?}",
        rep.compared, rep.allowed, rep.owner_approved
    );
    assert!(rep.compared >= 20);
    assert_clean(&rep);
}

// ------------------------------------------------------------------ tests: resolver

fn schema_with(body: &str) -> String {
    format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:t" xmlns="urn:t" elementFormDefault="qualified">{body}<xs:element name="Root" type="xs:string"/></xs:schema>"#
    )
}

/// One schema directory with many root files, so that one oracle run covers all cases.
/// Returns the (status, class) of every `<name>.xsd` file.
fn run_cases(cases: &[(&str, String)]) -> Option<BTreeMap<String, (String, String)>> {
    let d = tempfile::tempdir().unwrap();
    for (name, xsd) in cases {
        std::fs::write(d.path().join(format!("{name}.xsd")), xsd).unwrap();
    }
    std::fs::write(
        d.path().join("other.xsd"),
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:t" xmlns="urn:t"><xs:simpleType name="S"><xs:restriction base="xs:string"/></xs:simpleType></xs:schema>"#,
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hostname", d.path().join("link.xsd")).unwrap();
    let msgs = tempfile::tempdir().unwrap();
    let out = oracle(d.path(), msgs.path())?;
    Some(
        out["schemas"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["file"]
                        .as_str()
                        .unwrap()
                        .trim_end_matches(".xsd")
                        .to_string(),
                    (
                        s["status"].as_str().unwrap().to_string(),
                        s["class"].as_str().unwrap().to_string(),
                    ),
                )
            })
            .collect(),
    )
}

#[test]
fn resolver_serves_local_files_and_refuses_everything_else() {
    let inc = |loc: &str| schema_with(&format!(r#"<xs:include schemaLocation="{loc}"/>"#));
    let imp = |loc: &str| {
        schema_with(&format!(
            r#"<xs:import namespace="urn:o" schemaLocation="{loc}"/>"#
        ))
    };
    let refused: Vec<(&str, String)> = vec![
        ("dotdot", inc("../outside.xsd")),
        ("dotdot_in_path", inc("sub/../../outside.xsd")),
        ("absolute", inc("/etc/hostname")),
        ("file_uri", inc("file:///etc/hostname")),
        ("file_upper", inc("FILE:///etc/hostname")),
        ("file_mixed", inc("File:///etc/hostname")),
        ("encoded_dots", inc("%2e%2e/outside.xsd")),
        ("encoded_slash", inc("..%2foutside.xsd")),
        ("backslash", inc("..\\outside.xsd")),
        ("http", inc("http://127.0.0.1:9/a.xsd")),
        ("ftp", inc("ftp://127.0.0.1:9/a.xsd")),
        ("jar", inc("jar:file:///x.jar!/a.xsd")),
        ("import_outside", imp("../outside.xsd")),
        ("import_http", imp("http://127.0.0.1:9/a.xsd")),
        (
            "redefine_outside",
            schema_with(r#"<xs:redefine schemaLocation="../outside.xsd"/>"#),
        ),
        ("symlink", inc("link.xsd")),
    ];
    let mut cases = refused.clone();
    cases.push(("local_ok", inc("./other.xsd")));
    cases.push((
        "doctype",
        format!("<!DOCTYPE x [<!ENTITY a \"b\">]>{}", schema_with("")),
    ));
    // XSD 1.1 override and XInclude are not part of XSD 1.0. Xerces must not load them.
    cases.push((
        "override",
        schema_with(r#"<xs:override schemaLocation="../outside.xsd"/>"#),
    ));
    cases.push((
        "xinclude",
        schema_with(
            r#"<xi:include xmlns:xi="http://www.w3.org/2001/XInclude" href="../outside.xsd"/>"#,
        ),
    ));
    let Some(got) = run_cases(&cases) else {
        return;
    };
    assert_eq!(got["local_ok"], ("ok".to_string(), String::new()));
    for (name, _) in &refused {
        #[cfg(not(unix))]
        if *name == "symlink" {
            continue;
        }
        assert_eq!(
            got[*name],
            ("error".to_string(), "resolver_refused".to_string()),
            "{name}"
        );
    }
    assert_eq!(
        got["doctype"],
        ("error".to_string(), "doctype_refused".to_string())
    );
    for name in ["override", "xinclude"] {
        assert_eq!(got[name].0, "error", "{name}");
        assert!(!got[name].1.is_empty(), "{name}");
    }
}

#[test]
fn external_xsi_schema_location_is_ignored_and_not_loaded() {
    // The self-test message ok_schemalocation.xml has a hint to a closed port.
    let dir = selftest_dir();
    let Some(out) = oracle(&dir, &dir.join("messages")) else {
        return;
    };
    let r = out["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["file"] == "ok_schemalocation.xml")
        .unwrap();
    assert_eq!(r["verdict"], "valid");
}

#[test]
fn schema_that_breaks_the_unique_particle_rule_is_refused_by_both() {
    let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:t" xmlns="urn:t" elementFormDefault="qualified"><xs:element name="Root"><xs:complexType><xs:sequence><xs:element name="a" type="xs:string" minOccurs="0"/><xs:element name="a" type="xs:string"/></xs:sequence></xs:complexType></xs:element></xs:schema>"#;
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("root.xsd"), xsd).unwrap();
    let msgs = tempfile::tempdir().unwrap();
    let Some(out) = oracle(d.path(), msgs.path()) else {
        return;
    };
    assert_eq!(
        out["schemas"][0]["status"], "error",
        "Xerces must refuse the schema"
    );
    let loader = xtr_on_rust::inbound::contract::fs_loader(d.path());
    let built = build(&[("root.xsd".into(), xsd.into())], &loader, &["Root"]);
    assert!(built.is_err(), "XTR must refuse the same schema at boot");
}

// ------------------------------------------------------------------ tests: expected differences

#[test]
fn expected_differences_file_is_closed() {
    let known: BTreeSet<String> = [
        "non_utf8_declaration",
        "strict_prolog",
        "unicode_unassigned",
        "year_over_nine_digits",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(expected_difference_ids(), known);
}

#[test]
fn owner_approved_file_is_closed_and_counted_apart() {
    let known: BTreeSet<String> = ["utf8_label"].iter().map(|s| s.to_string()).collect();
    assert_eq!(owner_approved_ids(), known);
    // The two lists do not share an entry.
    assert!(owner_approved_ids().is_disjoint(&expected_difference_ids()));
    let dir = selftest_dir();
    let Some(rep) = differential(&dir, &dir.join("messages")) else {
        return;
    };
    assert_clean(&rep);
    // U3: the hits are counted in their own table.
    assert!(
        rep.owner_approved.get("utf8_label").copied().unwrap_or(0) == 1,
        "only the alias message hits the exception: {rep:?}"
    );
    assert!(!rep.allowed.contains_key("utf8_label"));
}

/// U1 and U2: only the exact alias `utf8` is accepted. Other labels are refused by both sides.
#[test]
fn utf8_label_exception_is_narrow() {
    assert!(has_utf8_alias_label(
        r#"<?xml version="1.0" encoding="utf8"?><a/>"#
    ));
    assert!(has_utf8_alias_label(
        "<?xml version='1.0' encoding='UTF8'?><a/>"
    ));
    for no in [
        r#"<?xml version="1.0" encoding="utf_8"?><a/>"#,
        r#"<?xml version="1.0" encoding="utf-8 "?><a/>"#,
        r#"<?xml version="1.0" encoding="utf8x"?><a/>"#,
        r#"<?xml version="1.0" encoding="UTF-16"?><a/>"#,
        r#"<?xml version="1.0" encoding="latin1"?><a/>"#,
        r#"<a encoding="utf8"/>"#,
        r#"<a/><?xml version="1.0" encoding="utf8"?>"#,
    ] {
        assert!(!has_utf8_alias_label(no), "{no}");
    }
    assert_eq!(
        replace_utf8_label(
            "\u{feff}<?xml version=\"1.0\" encoding=\"UTF8\"?><a encoding=\"utf8\"/>"
        ),
        "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?><a encoding=\"utf8\"/>"
    );
    // The direction is fixed: the exception covers only "Xerces refuses, XTR accepts".
    let ids = expected_difference_ids();
    let mut rep = Report::default();
    let nw: Value =
        serde_json::json!({"file":"c.xml","verdict":"not_wellformed","cn_chars":0,"errors":[]});
    let utf8 = r#"<?xml version="1.0" encoding="utf8"?><a/>"#;
    compare(&mut rep, &nw, &Rust::Done(vec![]), utf8, &ids);
    assert!(
        rep.diffs.is_empty() && rep.utf8_files == ["c.xml"],
        "{rep:?}"
    );
    // A different label is a difference.
    let bad = r#"<?xml version="1.0" encoding="utf8x"?><a/>"#;
    compare(&mut rep, &nw, &Rust::Done(vec![]), bad, &ids);
    assert_eq!(rep.diffs.len(), 1);
    // The exception is off in a strict report.
    let mut strict = Report {
        strict: true,
        ..Report::default()
    };
    compare(&mut strict, &nw, &Rust::Done(vec![]), utf8, &ids);
    assert_eq!(strict.diffs.len(), 1);
}

#[test]
fn expected_difference_non_utf8_declaration() {
    let dir = selftest_dir();
    let Some(rep) = differential(&dir, &dir.join("messages")) else {
        return;
    };
    assert!(
        rep.allowed
            .get("non_utf8_declaration")
            .copied()
            .unwrap_or(0)
            >= 1,
        "the encoding message must be an allowed difference: {rep:?}"
    );
}

/// Direction check: an allowed difference never goes the other way.
#[test]
fn expected_differences_never_allow_xtr_accepts_xerces_refuses() {
    let mut rep = Report::default();
    let ids = expected_difference_ids();
    let r: Value = serde_json::json!({"file":"x.xml","verdict":"refused","cn_chars":5,"errors":[]});
    compare(&mut rep, &r, &Rust::Done(vec![]), "", &ids);
    let r: Value = serde_json::json!({"file":"y.xml","verdict":"invalid","cn_chars":5,
        "errors":[{"path":"/a","code":"cvc-enumeration-valid","line":1}]});
    compare(&mut rep, &r, &Rust::Done(vec![]), "", &ids);
    assert_eq!(rep.diffs.len(), 2, "{rep:?}");
    assert!(rep.allowed.is_empty());
}

#[test]
fn expected_difference_strict_prolog() {
    // The self-test set has prolog cases. XTR may be stricter than Xerces only for XML
    // declaration rules, and only in the allowed direction.
    let dir = selftest_dir();
    let Some(rep) = differential(&dir, &dir.join("messages")) else {
        return;
    };
    assert_clean(&rep);
    // The direction is machine-checked: Xerces accepts, XTR refuses with an XML declaration error.
    let ids = expected_difference_ids();
    let mut rep = Report::default();
    let valid: Value =
        serde_json::json!({"file":"a.xml","verdict":"valid","cn_chars":0,"errors":[]});
    let refused = Rust::Refused(
        DomErrorKind::Malformed,
        "XML declaration: version is required".into(),
    );
    compare(&mut rep, &valid, &refused, "", &ids);
    assert_eq!(rep.allowed.get("strict_prolog"), Some(&1));
    assert!(rep.diffs.is_empty());
    // Other refusals are not allowed.
    let other = Rust::Refused(DomErrorKind::Malformed, "unbalanced end tag".into());
    compare(&mut rep, &valid, &other, "", &ids);
    assert_eq!(rep.diffs.len(), 1);
    // XTR accepts, Xerces refuses: never allowed.
    let nw: Value =
        serde_json::json!({"file":"b.xml","verdict":"not_wellformed","cn_chars":0,"errors":[]});
    compare(&mut rep, &nw, &Rust::Done(vec![]), "", &ids);
    assert_eq!(rep.diffs.len(), 2);
}

#[test]
fn expected_difference_year_over_nine_digits() {
    let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:y" xmlns="urn:y" elementFormDefault="qualified"><xs:element name="Y"><xs:complexType><xs:sequence><xs:element name="D" type="xs:date"/><xs:element name="T" type="xs:dateTime"/></xs:sequence></xs:complexType></xs:element></xs:schema>"#;
    let sd = tempfile::tempdir().unwrap();
    std::fs::write(sd.path().join("y.xsd"), xsd).unwrap();
    let md = tempfile::tempdir().unwrap();
    let msg = |d: &str, t: &str| format!("<Y xmlns=\"urn:y\"><D>{d}</D><T>{t}</T></Y>");
    std::fs::write(
        md.path().join("long_date.xml"),
        msg("1234567890-01-01", "2026-01-01T00:00:00Z"),
    )
    .unwrap();
    std::fs::write(
        md.path().join("long_datetime.xml"),
        msg("2026-01-01", "1234567890-01-01T00:00:00Z"),
    )
    .unwrap();
    // Control: a bad date is not covered by the entry.
    std::fs::write(
        md.path().join("bad_month.xml"),
        msg("2026-13-01", "2026-01-01T00:00:00Z"),
    )
    .unwrap();
    let Some(rep) = differential(sd.path(), md.path()) else {
        return;
    };
    assert_eq!(
        rep.allowed.get("year_over_nine_digits").copied(),
        Some(2),
        "{rep:?}"
    );
    assert_clean(&rep);
}

#[test]
fn year_entry_is_narrow_and_directional() {
    assert!(has_long_year("<D>1234567890-01-01</D>"));
    assert!(has_long_year("<T>-1234567890-01-01T00:00:00Z</T>"));
    assert!(!has_long_year("<D>123456789-01-01</D>"));
    assert!(!has_long_year("<D>2026-13-01</D>"));
    let ids = expected_difference_ids();
    // Xerces invalid, XTR valid, long year: never allowed.
    let mut rep = Report::default();
    let r: Value = serde_json::json!({"file":"z.xml","verdict":"invalid","cn_chars":0,
        "errors":[{"path":"/D","code":"cvc-datatype-valid.1.2.1","line":1}]});
    compare(
        &mut rep,
        &r,
        &Rust::Done(vec![]),
        "<D>1234567890-01-01</D>",
        &ids,
    );
    assert_eq!(rep.diffs.len(), 1);
    // Xerces valid, XTR reports another code, long year in text: not allowed.
    let mut rep = Report::default();
    let r: Value = serde_json::json!({"file":"z.xml","verdict":"valid","cn_chars":0,"errors":[]});
    compare(
        &mut rep,
        &r,
        &Rust::Done(vec![("/D".into(), "pattern".into())]),
        "<D>1234567890-01-01</D>",
        &ids,
    );
    assert_eq!(rep.diffs.len(), 1);
}

#[test]
fn expected_difference_unicode_unassigned() {
    // U+10D50 is a letter since Unicode 16. A pattern with \P{L} shows a table difference
    // when the JDK table is older than the table of the regex crate.
    let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:u" xmlns="urn:u" elementFormDefault="qualified"><xs:simpleType name="N"><xs:restriction base="xs:string"><xs:pattern value="\P{L}+"/></xs:restriction></xs:simpleType><xs:element name="U" type="N"/></xs:schema>"#;
    let sd = tempfile::tempdir().unwrap();
    std::fs::write(sd.path().join("u.xsd"), xsd).unwrap();
    let md = tempfile::tempdir().unwrap();
    std::fs::write(md.path().join("u.xml"), "<U xmlns=\"urn:u\">\u{10D50}</U>").unwrap();
    let Some(rep) = differential(sd.path(), md.path()) else {
        return;
    };
    // Allowed only through the list; a difference in the other direction is an error.
    assert_clean(&rep);
    eprintln!("unicode difference cases allowed: {:?}", rep.allowed);
}

// ------------------------------------------------------------------ mutations

/// Start and end byte of every element, and its start tag end.
fn element_spans(x: &str) -> Vec<(usize, usize, usize)> {
    let b = x.as_bytes();
    let mut spans = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if x[i..].starts_with("<?") || x[i..].starts_with("<!") {
            i += x[i..].find('>').map_or(b.len(), |e| e + 1);
            continue;
        }
        let Some(end) = x[i..].find('>') else { break };
        let tag_end = i + end + 1;
        if b[i + 1] == b'/' {
            if let Some((s, te)) = stack.pop() {
                spans.push((s, tag_end, te));
            }
        } else if b[tag_end - 2] == b'/' {
            spans.push((i, tag_end, tag_end));
        } else {
            stack.push((i, tag_end));
        }
        i = tag_end;
    }
    spans.sort();
    spans
}

/// Attribute value ranges inside a start tag.
fn attr_values(x: &str, start: usize, tag_end: usize) -> Vec<(usize, usize, usize)> {
    let t = &x[start..tag_end];
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(eq) = t[i..].find('=') {
        let eq = i + eq;
        let q = t.as_bytes().get(eq + 1).copied();
        let Some(q @ (b'"' | b'\'')) = q else { break };
        let Some(close) = t[eq + 2..].find(q as char) else {
            break;
        };
        let name_start = t[..eq]
            .rfind(|c: char| c.is_whitespace())
            .map_or(0, |p| p + 1);
        out.push((start + name_start, start + eq + 2, start + eq + 2 + close));
        i = eq + 2 + close + 1;
    }
    out
}

/// Deterministic text mutations of one valid message. At most `cap` results.
fn mutate(x: &str, cap: usize) -> Vec<String> {
    let spans = element_spans(x);
    let mut out = Vec::new();
    let step = (spans.len() / (cap / 12).max(1)).max(1);
    for &(s, e, te) in spans.iter().step_by(step) {
        // Elements.
        out.push(format!("{}{}", &x[..s], &x[e..]));
        out.push(format!("{}{}{}", &x[..e], &x[s..e], &x[e..]));
        out.push(format!("{}<Unexpected/>{}", &x[..te], &x[te..]));
        out.push(format!("{}<Unexpected/>{}", &x[..s], &x[s..]));
        // Text.
        if te < e && x[te..e].find('<').is_none_or(|p| p > 0) {
            let close = x[te..e].find('<').map_or(e, |p| te + p);
            for t in [
                "",
                "!!!",
                "  padded  ",
                &"z".repeat(300),
                "-1",
                "0",
                "2026-13-01",
            ] {
                out.push(format!("{}{}{}", &x[..te], t, &x[close..]));
            }
        } else {
            out.push(format!("{}text{}", &x[..te], &x[te..]));
        }
        // Attributes.
        let attrs = attr_values(x, s, te);
        for &(name_start, vs, ve) in &attrs {
            out.push(format!("{}{}", &x[..name_start], &x[ve + 1..]));
            out.push(format!("{}{}", &x[..vs], &x[ve..]));
            out.push(format!("{}x{}", &x[..ve], &x[ve..]));
            out.push(format!("{}!{}", &x[..vs], &x[vs..]));
        }
        out.push(format!(
            "{} zz=\"1\"{}",
            x[..te - 1].trim_end_matches('/'),
            &x[te - 1..]
        ));
    }
    out.retain(|m| m != x);
    out.truncate(cap);
    out
}

#[test]
fn mutations_are_deterministic_and_change_the_message() {
    let x = "<a xmlns=\"urn:t\" k=\"1\"><b>t</b><c/></a>";
    let m = mutate(x, 100);
    assert!(m.len() > 10);
    assert!(m.iter().all(|s| s != x));
    assert_eq!(m, mutate(x, 100));
}

#[test]
fn selftest_mutations_agree_with_xerces() {
    let dir = selftest_dir();
    let tmp = tempfile::tempdir().unwrap();
    let mut n = 0;
    for f in ["ok_full.xml", "ok_minimal.xml"] {
        let base = std::fs::read_to_string(dir.join("messages").join(f)).unwrap();
        for m in mutate(&base, 400) {
            std::fs::write(tmp.path().join(format!("m{n:05}.xml")), m).unwrap();
            n += 1;
        }
    }
    let Some(rep) = differential(&dir, tmp.path()) else {
        return;
    };
    eprintln!(
        "mutations: compared {}, allowed {:?}",
        rep.compared, rep.allowed
    );
    assert!(rep.compared >= 100);
    assert_clean(&rep);
}

// ------------------------------------------------------------------ gate test

/// Gate of the ADR: XTR and Xerces must agree on the ERRU samples and on
/// generated invalid messages. Run it with `cargo test --test xsd_oracle -- --ignored`.
#[test]
#[ignore = "needs XTR_ERRU_XSD_DIR and XTR_ORACLE_SAMPLES_DIR; runs in the manual workflow"]
fn gate_erru_samples_and_mutations_have_zero_differences() {
    let Some(xsd) = std::env::var_os("XTR_ERRU_XSD_DIR").map(PathBuf::from) else {
        unavailable("XTR_ERRU_XSD_DIR is not set");
        return;
    };
    assert!(xsd.is_dir() || !required(), "XSD directory is missing");
    let Some(samples) = std::env::var_os("XTR_ORACLE_SAMPLES_DIR").map(PathBuf::from) else {
        unavailable("XTR_ORACLE_SAMPLES_DIR is not set");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let mut n = 0;
    let mut files: Vec<_> = std::fs::read_dir(&samples)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "xml"))
        .collect();
    files.sort();
    for f in &files {
        let base = std::fs::read_to_string(f).unwrap();
        std::fs::write(tmp.path().join(format!("s{n:05}.xml")), &base).unwrap();
        n += 1;
        for m in mutate(&base, 600) {
            std::fs::write(tmp.path().join(format!("m{n:05}.xml")), m).unwrap();
            n += 1;
        }
    }
    let Some(rep) = differential(&xsd, tmp.path()) else {
        return;
    };
    eprintln!(
        "gate: {} samples, compared {}, allowed {:?}",
        files.len(),
        rep.compared,
        rep.allowed
    );
    assert!(rep.compared > 0 || !required(), "zero cases were compared");
    assert_clean(&rep);
}
