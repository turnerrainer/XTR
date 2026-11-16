//! Validation model: built once at boot from the lane's XSD documents.

use crate::inbound::contract::{QName, SchemaLoader};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One reason why the model cannot be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Schema file that holds the construct.
    pub file: String,
    /// Name of the global component that contains the construct.
    pub component: String,
    /// The unsupported construct, for example `xs:any`.
    pub construct: String,
    /// Line in `file`, counted from 1. `0` means unknown.
    pub line: u32,
}

/// Validation model of the messages of one lane.
#[derive(Debug)]
pub struct Model {
    /// Message roots by local name.
    pub roots: BTreeMap<String, Arc<ElementDecl>>,
}

/// Builds the model for the message roots `roots` (global element
/// names) from the entry schemas `entries` (`file name`, `content`).
/// `loader` resolves `xs:include` targets.
pub fn build(
    entries: &[(String, String)],
    loader: &SchemaLoader,
    roots: &[&str],
) -> Result<Model, Vec<Problem>> {
    super::builder::run(entries, loader, roots)
}

/// Position in a schema file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SrcLoc {
    /// File name as the loader gave it, never an absolute path.
    pub file: String,
    pub line: u32,
}

/// Element declaration.
#[derive(Debug)]
pub struct ElementDecl {
    pub name: QName,
    pub ty: TypeDef,
    pub loc: SrcLoc,
}

#[derive(Debug, Clone)]
pub enum TypeDef {
    Simple(Arc<SimpleType>),
    Complex(Arc<ComplexType>),
}

/// How often a particle may occur. `max == None` means unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occurs {
    pub min: u32,
    pub max: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum Particle {
    Element {
        decl: Arc<ElementDecl>,
        occurs: Occurs,
    },
    Sequence {
        items: Vec<Particle>,
        occurs: Occurs,
    },
    Choice {
        items: Vec<Particle>,
        occurs: Occurs,
    },
}

/// Complex type with the base type already merged in: `content` holds
/// the base content first, then the own content.
#[derive(Debug, Default)]
pub struct ComplexType {
    pub content: Option<Particle>,
    pub attributes: Vec<AttributeDecl>,
}

#[derive(Debug, Clone)]
pub struct AttributeDecl {
    /// Attributes are in no namespace.
    pub name: String,
    pub ty: Arc<SimpleType>,
    pub required: bool,
    pub fixed: Option<String>,
    pub loc: SrcLoc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    String,
    Token,
    Boolean,
    NonNegativeInteger,
    PositiveInteger,
    Date,
    DateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteSpace {
    Preserve,
    Replace,
    Collapse,
}

/// Facets of one derivation step. A value must satisfy the facets of
/// every step (AND). Inside a step, `enumeration` values and `pattern`s
/// combine with OR.
#[derive(Debug, Clone, Default)]
pub struct Step {
    pub enumeration: Option<Vec<String>>,
    pub patterns: Vec<regex::Regex>,
    pub length: Option<usize>,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub min_inclusive: Option<String>,
    /// Where this step is declared in a schema file.
    pub loc: SrcLoc,
}

#[derive(Debug, Clone)]
pub struct SimpleType {
    pub base: Builtin,
    pub whitespace: WhiteSpace,
    pub steps: Vec<Step>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const XS: &str = r#"xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:t" targetNamespace="urn:t" elementFormDefault="qualified""#;

    fn no_files(_: &str) -> Option<(String, PathBuf, String)> {
        None
    }

    fn try_build(body: &str, root: &str) -> Result<Model, Vec<Problem>> {
        let xsd = format!("<xs:schema {XS}>{body}</xs:schema>");
        build(&[("m.xsd".into(), xsd)], &no_files, &[root])
    }

    fn constructs(body: &str, root: &str) -> Vec<String> {
        match try_build(body, root) {
            Ok(_) => vec![],
            Err(p) => p.into_iter().map(|p| p.construct).collect(),
        }
    }

    #[test]
    fn supported_subset_builds() {
        let body = r#"
          <xs:simpleType name="Code"><xs:restriction base="xs:token">
            <xs:whiteSpace value="collapse"/><xs:enumeration value="A"/><xs:enumeration value="B"/>
          </xs:restriction></xs:simpleType>
          <xs:complexType name="Base"><xs:sequence><xs:element name="a" type="t:Code"/></xs:sequence>
            <xs:attribute name="id" type="xs:string" use="required"/></xs:complexType>
          <xs:complexType name="Ext"><xs:complexContent><xs:extension base="t:Base"><xs:sequence>
            <xs:choice><xs:element name="x" type="xs:date"/><xs:element name="y" type="xs:dateTime"/></xs:choice>
            <xs:element name="n" type="xs:nonNegativeInteger" minOccurs="0" maxOccurs="3"/>
          </xs:sequence></xs:extension></xs:complexContent></xs:complexType>
          <xs:element name="Msg" type="t:Ext"/>"#;
        assert!(try_build(body, "Msg").is_ok());
    }

    #[test]
    fn unsupported_constructs_reachable_from_a_root_stop_the_build() {
        let cases: [(&str, &str); 11] = [
            ("xs:any", r#"<xs:sequence><xs:any/></xs:sequence>"#),
            ("xs:anyAttribute", r#"<xs:sequence/><xs:anyAttribute/>"#),
            (
                "xs:all",
                r#"<xs:all><xs:element name="a" type="xs:string"/></xs:all>"#,
            ),
            ("mixed", r#"<xs:sequence/>"#),
            (
                "nillable",
                r#"<xs:sequence><xs:element name="a" type="xs:string" nillable="true"/></xs:sequence>"#,
            ),
            (
                "substitutionGroup",
                r#"<xs:sequence><xs:element name="a" type="xs:string" substitutionGroup="t:Msg"/></xs:sequence>"#,
            ),
            (
                "xs:union",
                r#"<xs:sequence><xs:element name="a"><xs:simpleType><xs:union memberTypes="xs:string"/></xs:simpleType></xs:element></xs:sequence>"#,
            ),
            (
                "xs:list",
                r#"<xs:sequence><xs:element name="a"><xs:simpleType><xs:list itemType="xs:string"/></xs:simpleType></xs:element></xs:sequence>"#,
            ),
            (
                "xs:key",
                r#"<xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>"#,
            ),
            ("abstract", r#"<xs:sequence/>"#),
            (
                "xs:integer",
                r#"<xs:sequence><xs:element name="a" type="xs:integer"/></xs:sequence>"#,
            ),
        ];
        for (construct, inner) in cases {
            let (attr, extra) = match construct {
                "mixed" => (r#" mixed="true""#, ""),
                "abstract" => (r#" abstract="true""#, ""),
                "xs:key" => (
                    "",
                    r#"<xs:key name="k"><xs:selector xpath="a"/><xs:field xpath="."/></xs:key>"#,
                ),
                _ => ("", ""),
            };
            let body = format!(
                r#"<xs:complexType name="T"{attr}>{inner}</xs:complexType>
                   <xs:element name="Msg" type="t:T">{extra}</xs:element>"#
            );
            let got = constructs(&body, "Msg");
            assert!(
                got.iter().any(|c| c == construct),
                "{construct}: build must fail and name the construct, got {got:?}"
            );
        }
    }

    #[test]
    fn unsupported_construct_in_a_base_type_is_reached_through_extension() {
        let body = r#"
          <xs:complexType name="Base"><xs:sequence><xs:any/></xs:sequence></xs:complexType>
          <xs:complexType name="Ext"><xs:complexContent><xs:extension base="t:Base"><xs:sequence>
            <xs:element name="b" type="xs:string"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>
          <xs:element name="Msg" type="t:Ext"/>"#;
        assert_eq!(constructs(body, "Msg"), vec!["xs:any".to_string()]);
    }

    #[test]
    fn unsupported_construct_that_no_root_reaches_is_ignored() {
        let body = r#"
          <xs:complexType name="Unused"><xs:sequence><xs:any/></xs:sequence></xs:complexType>
          <xs:element name="Other" type="t:Unused"/>
          <xs:element name="Msg"><xs:complexType><xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence></xs:complexType></xs:element>"#;
        assert!(try_build(body, "Msg").is_ok());
    }

    #[test]
    fn a_problem_names_the_file_and_the_component_but_no_value() {
        let body = r#"<xs:complexType name="T"><xs:sequence><xs:any/></xs:sequence></xs:complexType>
                      <xs:element name="Msg" type="t:T"/>"#;
        let p = try_build(body, "Msg").unwrap_err();
        assert_eq!(p[0].file, "m.xsd");
        assert_eq!(p[0].component, "T");
    }

    #[test]
    fn unknown_root_is_a_problem() {
        let body = r#"<xs:element name="Msg" type="xs:string"/>"#;
        assert!(try_build(body, "Nope").is_err());
    }

    // ---- unique particle attribution (UPA) ----

    const UPA: &str = "ambiguous content model";

    fn upa(body: &str) -> Vec<String> {
        constructs(body, "Msg")
    }

    fn el(n: &str, extra: &str) -> String {
        format!(r#"<xs:element name="{n}" type="xs:string" {extra}/>"#)
    }

    fn msg(model: &str) -> String {
        format!(r#"<xs:element name="Msg"><xs:complexType>{model}</xs:complexType></xs:element>"#)
    }

    #[test]
    fn upa_two_branches_of_a_choice_with_the_same_name_stop_the_build() {
        let m = msg(&format!(
            "<xs:choice>{}{}</xs:choice>",
            el("a", ""),
            el("a", "")
        ));
        assert_eq!(upa(&m), vec![UPA.to_string()]);
    }

    #[test]
    fn upa_optional_element_before_the_same_name_in_a_sequence_stops_the_build() {
        let m = msg(&format!(
            r#"<xs:sequence>{}{}</xs:sequence>"#,
            el("a", r#"minOccurs="0""#),
            el("a", "")
        ));
        assert_eq!(upa(&m), vec![UPA.to_string()]);
    }

    #[test]
    fn upa_across_the_extension_boundary_stops_the_build() {
        let body = format!(
            r#"<xs:complexType name="Base"><xs:sequence>{}</xs:sequence></xs:complexType>
               <xs:element name="Msg"><xs:complexType><xs:complexContent>
                 <xs:extension base="t:Base"><xs:sequence>{}</xs:sequence></xs:extension>
               </xs:complexContent></xs:complexType></xs:element>"#,
            el("a", r#"minOccurs="0""#),
            el("a", "")
        );
        assert_eq!(upa(&body), vec![UPA.to_string()]);
    }

    #[test]
    fn upa_through_an_empty_group_stops_the_build() {
        let m = msg(&format!(
            r#"<xs:sequence>{}<xs:sequence/>{}</xs:sequence>"#,
            el("a", r#"minOccurs="0""#),
            el("a", "")
        ));
        assert_eq!(upa(&m), vec![UPA.to_string()]);
        let m = msg(&format!(
            r#"<xs:sequence>{}<xs:sequence minOccurs="0">{}</xs:sequence>{}</xs:sequence>"#,
            el("a", r#"minOccurs="0""#),
            el("b", ""),
            el("a", "")
        ));
        assert_eq!(upa(&m), vec![UPA.to_string()]);
    }

    #[test]
    fn upa_a_repeated_group_whose_first_set_meets_the_follow_set_stops_the_build() {
        let m = msg(&format!(
            r#"<xs:sequence><xs:sequence maxOccurs="unbounded">{}{}</xs:sequence>{}</xs:sequence>"#,
            el("a", ""),
            el("b", r#"minOccurs="0""#),
            el("a", "")
        ));
        assert_eq!(upa(&m), vec![UPA.to_string()]);
    }

    #[test]
    fn upa_an_element_that_repeats_meets_the_same_name_after_it() {
        let m = msg(&format!(
            r#"<xs:sequence>{}{}</xs:sequence>"#,
            el("a", r#"maxOccurs="unbounded""#),
            el("a", "")
        ));
        assert_eq!(upa(&m), vec![UPA.to_string()]);
    }

    #[test]
    fn upa_unambiguous_models_build() {
        let ok = [
            format!("<xs:sequence>{}{}</xs:sequence>", el("a", ""), el("b", "")),
            format!("<xs:choice>{}{}</xs:choice>", el("a", ""), el("b", "")),
            format!(
                r#"<xs:sequence>{}{}</xs:sequence>"#,
                el("a", r#"maxOccurs="unbounded""#),
                el("b", "")
            ),
            format!(
                r#"<xs:sequence maxOccurs="unbounded">{}{}</xs:sequence>"#,
                el("a", ""),
                el("b", "")
            ),
            format!(
                r#"<xs:sequence>{}{}{}</xs:sequence>"#,
                el("a", r#"minOccurs="0""#),
                el("b", r#"minOccurs="0""#),
                el("c", "")
            ),
            format!(
                r#"<xs:sequence>{}<xs:choice>{}{}</xs:choice>{}</xs:sequence>"#,
                el("a", ""),
                el("b", ""),
                el("c", ""),
                el("d", "")
            ),
        ];
        for model in ok {
            let m = msg(&model);
            assert!(try_build(&m, "Msg").is_ok(), "{model}");
        }
    }

    // ---- review findings B1 to B4 ----

    fn build_files(files: &[(&str, &str)], entry: &str, root: &str) -> Result<Model, Vec<Problem>> {
        let map: std::collections::HashMap<String, String> = files
            .iter()
            .map(|(n, c)| (n.to_string(), c.to_string()))
            .collect();
        let loader = move |loc: &str| {
            let name = loc.rsplit('/').next()?.to_string();
            map.get(&name)
                .map(|c| (name.clone(), PathBuf::from(&name), c.clone()))
        };
        let entry_xml = files
            .iter()
            .find(|(n, _)| *n == entry)
            .unwrap()
            .1
            .to_string();
        build(&[(entry.to_string(), entry_xml)], &loader, &[root])
    }

    #[test]
    fn a_problem_carries_the_line_of_the_construct() {
        let xsd = format!(
            "<xs:schema {XS}>\n<xs:complexType name=\"T\">\n  <xs:sequence>\n    <xs:any/>\n  </xs:sequence>\n</xs:complexType>\n<xs:element name=\"Msg\" type=\"t:T\"/>\n</xs:schema>"
        );
        let p = build(&[("m.xsd".into(), xsd)], &no_files, &["Msg"]).unwrap_err();
        assert_eq!((p[0].construct.as_str(), p[0].line), ("xs:any", 4));
    }

    #[test]
    fn a_global_definition_that_occurs_twice_is_a_problem() {
        let body = r#"<xs:complexType name="T"><xs:sequence/></xs:complexType>
          <xs:complexType name="T"><xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence></xs:complexType>
          <xs:element name="Msg" type="t:T"/>"#;
        let c = constructs(body, "Msg");
        assert!(
            c.iter().any(|c| c.starts_with("duplicate definition")),
            "{c:?}"
        );
        let body =
            r#"<xs:element name="Msg" type="xs:string"/><xs:element name="Msg" type="xs:token"/>"#;
        let c = constructs(body, "Msg");
        assert!(
            c.iter().any(|c| c.starts_with("duplicate definition")),
            "{c:?}"
        );
    }

    #[test]
    fn a_duplicate_across_an_include_is_a_problem() {
        let main = format!(
            r#"<xs:schema {XS}><xs:include schemaLocation="inc.xsd"/><xs:simpleType name="S"><xs:restriction base="xs:string"/></xs:simpleType><xs:element name="Msg" type="t:S"/></xs:schema>"#
        );
        let inc = format!(
            r#"<xs:schema {XS}><xs:simpleType name="S"><xs:restriction base="xs:token"/></xs:simpleType></xs:schema>"#
        );
        let r = build_files(&[("main.xsd", &main), ("inc.xsd", &inc)], "main.xsd", "Msg");
        let c: Vec<String> = r.unwrap_err().into_iter().map(|p| p.construct).collect();
        assert!(
            c.iter().any(|c| c.starts_with("duplicate definition")),
            "{c:?}"
        );
    }

    #[test]
    fn an_include_with_another_target_namespace_is_a_problem() {
        let main = format!(
            r#"<xs:schema {XS}><xs:include schemaLocation="inc.xsd"/><xs:element name="Msg" type="xs:string"/></xs:schema>"#
        );
        let inc = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:other"><xs:simpleType name="S"><xs:restriction base="xs:string"/></xs:simpleType></xs:schema>"#;
        let r = build_files(&[("main.xsd", &main), ("inc.xsd", inc)], "main.xsd", "Msg");
        let c: Vec<String> = r.unwrap_err().into_iter().map(|p| p.construct).collect();
        assert!(c.iter().any(|c| c.contains("targetNamespace")), "{c:?}");
    }

    #[test]
    fn a_chameleon_include_is_a_problem() {
        let main = format!(
            r#"<xs:schema {XS}><xs:include schemaLocation="inc.xsd"/><xs:element name="Msg" type="xs:string"/></xs:schema>"#
        );
        let inc = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:simpleType name="S"><xs:restriction base="xs:string"/></xs:simpleType></xs:schema>"#;
        let r = build_files(&[("main.xsd", &main), ("inc.xsd", inc)], "main.xsd", "Msg");
        let c: Vec<String> = r.unwrap_err().into_iter().map(|p| p.construct).collect();
        assert!(c.iter().any(|c| c.contains("chameleon")), "{c:?}");
    }

    #[test]
    fn max_occurs_zero_is_a_problem() {
        let m = msg(&format!(
            r#"<xs:sequence>{}{}</xs:sequence>"#,
            el("a", r#"maxOccurs="0" minOccurs="0""#),
            el("b", "")
        ));
        assert!(
            upa(&m).iter().any(|c| c.contains("maxOccurs")),
            "{:?}",
            upa(&m)
        );
    }

    // ---- review findings I5 and I6 ----

    #[test]
    fn a_derived_type_cannot_loosen_white_space() {
        let body = r#"
          <xs:simpleType name="B"><xs:restriction base="xs:token"/></xs:simpleType>
          <xs:simpleType name="V"><xs:restriction base="t:B"><xs:whiteSpace value="preserve"/></xs:restriction></xs:simpleType>
          <xs:element name="Msg" type="t:V"/>"#;
        let c = constructs(body, "Msg");
        assert!(c.iter().any(|c| c.contains("whiteSpace")), "{c:?}");
        let ok = r#"
          <xs:simpleType name="B"><xs:restriction base="xs:string"><xs:whiteSpace value="replace"/></xs:restriction></xs:simpleType>
          <xs:simpleType name="V"><xs:restriction base="t:B"><xs:whiteSpace value="collapse"/></xs:restriction></xs:simpleType>
          <xs:element name="Msg" type="t:V"/>"#;
        assert!(try_build(ok, "Msg").is_ok());
        let same = r#"<xs:simpleType name="V"><xs:restriction base="xs:token"><xs:whiteSpace value="collapse"/></xs:restriction></xs:simpleType>
          <xs:element name="Msg" type="t:V"/>"#;
        assert!(try_build(same, "Msg").is_ok());
    }

    #[test]
    fn enumeration_and_length_facets_on_a_non_string_base_are_a_problem() {
        for (base, facet) in [
            ("xs:nonNegativeInteger", r#"<xs:enumeration value="1"/>"#),
            ("xs:boolean", r#"<xs:enumeration value="true"/>"#),
            ("xs:date", r#"<xs:enumeration value="2026-01-01"/>"#),
            ("xs:positiveInteger", r#"<xs:maxLength value="3"/>"#),
            ("xs:dateTime", r#"<xs:length value="20"/>"#),
            ("xs:boolean", r#"<xs:minLength value="1"/>"#),
        ] {
            let body = format!(
                r#"<xs:simpleType name="V"><xs:restriction base="{base}">{facet}</xs:restriction></xs:simpleType>
                   <xs:element name="Msg" type="t:V"/>"#
            );
            let c = constructs(&body, "Msg");
            assert!(!c.is_empty(), "{base} {facet} must be a problem");
        }
        let ok = r#"<xs:simpleType name="V"><xs:restriction base="xs:token"><xs:enumeration value="A"/><xs:maxLength value="3"/></xs:restriction></xs:simpleType>
                    <xs:element name="Msg" type="t:V"/>"#;
        assert!(try_build(ok, "Msg").is_ok());
    }

    #[test]
    fn a_min_inclusive_value_must_fit_the_base_type() {
        for (base, bad) in [
            ("xs:positiveInteger", "abc"),
            ("xs:nonNegativeInteger", "-5"),
            ("xs:dateTime", "2026-13-01T00:00:00Z"),
            ("xs:date", "yesterday"),
        ] {
            let body = format!(
                r#"<xs:simpleType name="V"><xs:restriction base="{base}"><xs:minInclusive value="{bad}"/></xs:restriction></xs:simpleType>
                   <xs:element name="Msg" type="t:V"/>"#
            );
            let c = constructs(&body, "Msg");
            assert!(
                c.iter().any(|c| c.contains("minInclusive")),
                "{base} {bad}: {c:?}"
            );
        }
        let ok = r#"<xs:simpleType name="V"><xs:restriction base="xs:dateTime"><xs:minInclusive value="1753-01-01T00:00:00Z"/></xs:restriction></xs:simpleType>
                    <xs:element name="Msg" type="t:V"/>"#;
        assert!(try_build(ok, "Msg").is_ok());
    }
}
