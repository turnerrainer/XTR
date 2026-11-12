//! Inventory test: every message root of the ERRU 3.5 XSD bundle
//! builds a validation model, and the bundle needs no unsupported
//! construct. The bundle is not in this repository. Set
//! `XTR_ERRU_XSD_DIR` to the directory that holds the XSD files.
//! Without the variable the test prints a note and passes.

use std::path::PathBuf;
use xtr_on_rust::inbound::contract::fs_loader;
use xtr_on_rust::xsdval;

fn bundle_dir() -> Option<PathBuf> {
    std::env::var_os("XTR_ERRU_XSD_DIR").map(PathBuf::from)
}

#[test]
fn every_erru_root_element_builds_a_model_without_unsupported_constructs() {
    let Some(dir) = bundle_dir() else {
        eprintln!("XTR_ERRU_XSD_DIR is not set: inventory test skipped");
        return;
    };
    let loader = fs_loader(&dir);
    let mut stems: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".xsd").map(String::from))
        .filter(|s| !s.ends_with("_Types"))
        .collect();
    stems.sort();
    assert!(stems.len() >= 10, "found only {stems:?}");
    let mut failed = Vec::new();
    for stem in &stems {
        let (name, _path, xml) = loader(&format!("{stem}.xsd")).unwrap();
        match xsdval::build(&[(name, xml)], &loader, &[stem.as_str()]) {
            Ok(_) => {}
            Err(p) => failed.push(format!("{stem}: {p:?}")),
        }
    }
    assert!(
        failed.is_empty(),
        "unsupported constructs:\n{}",
        failed.join("\n")
    );
}
