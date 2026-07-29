//! Boot-time WSDL ingestion pipeline.
//!
//! For each `<wsdl_watch_dir>/<group>/*.wsdl` file:
//!   1. Parse it (via `wsdl::parser`).
//!   2. If a sidecar `<file>.meta.yaml` exists, load it for
//!      X-Road envelope wrapping.
//!   3. Generate one DSL per operation (via `wsdl::generator`).
//!   4. Write to `<dsl_path>/<group>/<operation>.yml` — but only
//!      if the target file (a) doesn't exist or (b) exists AND
//!      carries our marker header. Files without the marker are
//!      hand-written overrides and are SKIPPED with a WARN.
//!
//! Boot-only in v1. Hot reload is a v2 follow-up (`notify` on
//! `wsdl_watch_dir`, atomic ServiceMap swap).
//!
//! Failure to process one WSDL does not abort ingestion — the
//! failure is logged as WARN and the pipeline continues. Every
//! successfully-generated DSL still lands. Overall: XTR should
//! always boot; broken WSDLs surface as missing endpoints, not
//! as a refused-to-start.

use crate::error::XtrError;
use crate::wsdl::generator::{generate_all, WsdlMeta};
use crate::wsdl::parser::parse_with_loader;
use crate::wsdl::MARKER;
use std::fs;
use std::path::{Path, PathBuf};

/// Scan `wsdl_watch_dir` recursively for `*.wsdl` files and write
/// generated DSLs into `dsl_path`, preserving the relative directory
/// path from `wsdl_watch_dir`. So:
///   - `wsdl/ar/foo.wsdl`         → `DSL/ar/*.yml`
///   - `wsdl/maa-amet/ads/1.wsdl` → `DSL/maa-amet/ads/*.yml`
///
/// The loader keys endpoints by the file's IMMEDIATE parent directory
/// name (loader::derive_key), so the URL is always
/// `POST /<immediate-parent>/<operation>` regardless of nesting
/// depth. The outer directories are purely organisational — group
/// by owner, ministry, tenant, whatever helps humans navigate.
///
/// Idempotent — running twice is safe as long as no hand-editor
/// sneaked in between.
pub fn ingest_all(wsdl_watch_dir: &Path, dsl_path: &Path) -> Result<(), XtrError> {
    if !wsdl_watch_dir.exists() {
        tracing::warn!(
            "wsdl_watch_dir {} does not exist; skipping WSDL ingestion",
            wsdl_watch_dir.display()
        );
        return Ok(());
    }
    let mut counters = Counters::default();
    walk_and_ingest(wsdl_watch_dir, wsdl_watch_dir, dsl_path, &mut counters)?;
    tracing::info!(
        "WSDL ingestion: {} WSDL(s) processed, {} DSL(s) generated, {} hand-written \
         override(s) preserved",
        counters.wsdls,
        counters.ops,
        counters.overrides
    );
    Ok(())
}

#[derive(Default)]
struct Counters {
    wsdls: usize,
    ops: usize,
    overrides: usize,
}

/// Recurse `dir`, ingesting every `.wsdl` found. WSDL layout maps
/// to a FLAT DSL layout like this:
///
///   wsdl/<owner>/*.wsdl              → DSL/<owner>/<op>.yml
///   wsdl/<owner>/<subsystem>/*.wsdl  → DSL/<owner>/<subsystem>-<op>.yml
///   wsdl/<a>/<b>/<c>/*.wsdl          → DSL/<a>/<b>-<c>-<op>.yml  (etc)
///
/// Loader keys by immediate parent, so URLs are always
/// `POST /<owner>/<optionally-prefixed-op>` — 2 segments. The
/// nesting under `wsdl/` is purely for human browsability; the
/// URL stays owner-first and doesn't leak internal subsystem
/// codes unless there are name collisions to disambiguate.
fn walk_and_ingest(
    dir: &Path,
    root: &Path,
    dsl_path: &Path,
    counters: &mut Counters,
) -> Result<(), XtrError> {
    for entry in read_dir_sorted(dir)? {
        if entry.is_dir() {
            walk_and_ingest(&entry, root, dsl_path, counters)?;
            continue;
        }
        if !is_wsdl(&entry) {
            continue;
        }
        counters.wsdls += 1;
        // Relative subdir from wsdl_watch_dir: for
        // wsdl/maa-amet/ads/1.wsdl with root=wsdl, gives
        // ["maa-amet", "ads"].
        let rel = entry
            .parent()
            .and_then(|p| p.strip_prefix(root).ok())
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let segments: Vec<String> = rel
            .components()
            .filter_map(|c| c.as_os_str().to_str().map(String::from))
            .collect();
        // First segment = owner (URL group). Remaining segments
        // (if any) become a hyphen-joined operation-name prefix.
        let (group_dir, op_prefix): (PathBuf, String) = match segments.as_slice() {
            [] => (PathBuf::new(), String::new()),
            [owner] => (PathBuf::from(owner), String::new()),
            [owner, rest @ ..] => (PathBuf::from(owner), format!("{}-", rest.join("-"))),
        };
        match ingest_one(&entry, &group_dir, &op_prefix, dsl_path) {
            Ok(ing) => {
                counters.ops += ing.ops_written;
                counters.overrides += ing.skipped_overrides;
            }
            Err(e) => {
                tracing::warn!(
                    "WSDL {} failed to ingest: {} — skipping (XTR will boot without \
                     its endpoints)",
                    entry.display(),
                    e
                );
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct Ingested {
    ops_written: usize,
    skipped_overrides: usize,
}

fn ingest_one(
    wsdl_path: &Path,
    group: &Path,
    op_prefix: &str,
    dsl_path: &Path,
) -> Result<Ingested, XtrError> {
    let bytes = fs::read_to_string(wsdl_path)
        .map_err(|e| XtrError::Internal(format!("reading WSDL {}: {}", wsdl_path.display(), e)))?;
    let wsdl_dir = wsdl_path.parent().map(PathBuf::from).unwrap_or_default();
    let wsdl = parse_with_loader(&bytes, |location| resolve_local_schema(&wsdl_dir, location))?;
    let meta = load_meta_sidecar(wsdl_path)?;
    let group_dir = dsl_path.join(group);
    fs::create_dir_all(&group_dir).map_err(|e| {
        XtrError::Internal(format!(
            "creating DSL group dir {}: {}",
            group_dir.display(),
            e
        ))
    })?;
    let mut ing = Ingested::default();
    for (op_name, yaml) in generate_all(&wsdl, meta.as_ref())? {
        let out_path = group_dir.join(format!("{op_prefix}{op_name}.yml"));
        if is_hand_written_override(&out_path)? {
            tracing::warn!(
                "hand-written override at {} shadows generated DSL from {} (op={}) — \
                 not overwriting",
                out_path.display(),
                wsdl_path.display(),
                op_name
            );
            ing.skipped_overrides += 1;
            continue;
        }
        fs::write(&out_path, &yaml).map_err(|e| {
            XtrError::Internal(format!(
                "writing generated DSL {}: {}",
                out_path.display(),
                e
            ))
        })?;
        ing.ops_written += 1;
    }
    Ok(ing)
}

/// A file counts as a hand-written override when it exists AND
/// its first non-empty line is not our marker. Files carrying the
/// marker are ours to overwrite freely.
fn is_hand_written_override(path: &Path) -> Result<bool, XtrError> {
    match fs::read_to_string(path) {
        Ok(existing) => Ok(!existing.trim_start().starts_with(MARKER)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(XtrError::Internal(format!(
            "checking existing DSL {}: {}",
            path.display(),
            e
        ))),
    }
}

/// Resolve an `<xsd:include schemaLocation="URL-or-path"/>` reference
/// against the local filesystem. Looks for the *filename portion* of
/// the schemaLocation in the same directory as the WSDL. Absent →
/// None (parser logs a WARN, operations using it get skipped).
///
/// This is deliberately offline — XTR never fetches over HTTP at
/// boot. Operators wanting Ariregister-style WSDLs must download
/// the included XSDs alongside the WSDL (one-liner:
/// `for u in $(grep -oE 'schemaLocation="[^"]+"' foo.wsdl | cut -d'"' -f2); do curl -O "$u"; done`).
fn resolve_local_schema(wsdl_dir: &Path, location: &str) -> Option<String> {
    // Take the last path segment of the URL/path.
    let filename = location
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())?;
    let candidate = wsdl_dir.join(filename);
    fs::read_to_string(&candidate).ok()
}

/// Try `<wsdl>.meta.yaml` or `<wsdl>.meta.yml` next to the WSDL.
/// Absent = no metadata (plain SOAP envelope). Malformed → error.
fn load_meta_sidecar(wsdl_path: &Path) -> Result<Option<WsdlMeta>, XtrError> {
    for ext in ["meta.yaml", "meta.yml"] {
        let candidate = wsdl_path.with_extension(ext);
        if candidate.exists() {
            let body = fs::read_to_string(&candidate).map_err(|e| {
                XtrError::Internal(format!(
                    "reading meta sidecar {}: {}",
                    candidate.display(),
                    e
                ))
            })?;
            let meta: WsdlMeta = serde_yaml_ng::from_str(&body).map_err(|e| {
                XtrError::Internal(format!(
                    "parsing meta sidecar {}: {}",
                    candidate.display(),
                    e
                ))
            })?;
            return Ok(Some(meta));
        }
    }
    Ok(None)
}

fn is_wsdl(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("wsdl"))
        .unwrap_or(false)
}

/// Deterministic iteration order — filesystem readdir is
/// platform-dependent, we sort to make ingestion output stable.
fn read_dir_sorted(dir: &Path) -> Result<Vec<std::path::PathBuf>, XtrError> {
    let mut entries: Vec<std::path::PathBuf> = fs::read_dir(dir)
        .map_err(|e| XtrError::Internal(format!("reading dir {}: {}", dir.display(), e)))?
        .filter_map(|r| r.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const MINIMAL_WSDL: &str = r#"<?xml version="1.0"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
                  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
                  xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/"
                  xmlns:tns="http://ex/"
                  targetNamespace="http://ex/">
  <wsdl:types>
    <xsd:schema targetNamespace="http://ex/">
      <xsd:element name="lookup">
        <xsd:complexType>
          <xsd:sequence>
            <xsd:element name="q" type="xsd:string"/>
          </xsd:sequence>
        </xsd:complexType>
      </xsd:element>
    </xsd:schema>
  </wsdl:types>
  <wsdl:message name="in"><wsdl:part name="p" element="tns:lookup"/></wsdl:message>
  <wsdl:portType name="pt">
    <wsdl:operation name="lookup"><wsdl:input message="tns:in"/></wsdl:operation>
  </wsdl:portType>
  <wsdl:service name="s">
    <wsdl:port name="p" binding="tns:b">
      <soap:address location="https://example.com/soap"/>
    </wsdl:port>
  </wsdl:service>
</wsdl:definitions>"#;

    #[test]
    fn ingests_wsdl_and_writes_dsl_with_marker() {
        let watch = TempDir::new().unwrap();
        let dsl = TempDir::new().unwrap();
        let group_dir = watch.path().join("ex");
        std::fs::create_dir_all(&group_dir).unwrap();
        std::fs::write(group_dir.join("lookup.wsdl"), MINIMAL_WSDL).unwrap();

        ingest_all(watch.path(), dsl.path()).unwrap();

        let out = dsl.path().join("ex").join("lookup.yml");
        assert!(out.exists(), "expected generated DSL at {}", out.display());
        let contents = std::fs::read_to_string(&out).unwrap();
        assert!(contents.starts_with(MARKER));
        assert!(contents.contains("service: https://example.com/soap"));
        assert!(contents.contains("- q"));
    }

    #[test]
    fn owner_grouped_wsdl_layout_flattens_to_prefixed_dsl() {
        // Owner-grouped WSDL layout: wsdl/<owner>/<subsystem>/*.wsdl.
        // Generated DSLs FLATTEN one level to DSL/<owner>/
        // with the subsystem name prefixed onto the operation:
        //   wsdl/maa-amet/ads/1.wsdl (operation "lookup")
        //     → DSL/maa-amet/ads-lookup.yml
        //     → POST /maa-amet/ads-lookup
        // This keeps URLs 2-segment (unchanged router), keeps
        // the DSL/<owner>/*.yml shape the operator asked for,
        // and disambiguates when two subsystems under the same
        // owner have same-named operations.
        let watch = TempDir::new().unwrap();
        let dsl = TempDir::new().unwrap();
        let nested = watch.path().join("maa-amet").join("ads");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("1.wsdl"), MINIMAL_WSDL).unwrap();

        ingest_all(watch.path(), dsl.path()).unwrap();

        let out = dsl.path().join("maa-amet").join("ads-lookup.yml");
        assert!(
            out.exists(),
            "expected flattened DSL at {} — pipeline should collapse \
             wsdl/<owner>/<subsystem>/ into DSL/<owner>/<subsystem>-<op>.yml",
            out.display()
        );
        // No accidental nested DSL/<owner>/<subsystem>/ directory:
        let nested_dsl = dsl.path().join("maa-amet").join("ads");
        assert!(
            !nested_dsl.exists(),
            "did NOT expect nested DSL subdir at {}",
            nested_dsl.display()
        );
    }

    #[test]
    fn hand_written_dsl_survives_wsdl_regeneration() {
        let watch = TempDir::new().unwrap();
        let dsl = TempDir::new().unwrap();
        let group_dir = watch.path().join("ex");
        std::fs::create_dir_all(&group_dir).unwrap();
        std::fs::write(group_dir.join("lookup.wsdl"), MINIMAL_WSDL).unwrap();

        // Pre-existing hand-written DSL (no marker header).
        let out = dsl.path().join("ex").join("lookup.yml");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(
            &out,
            "params:\n  - custom\nservice: https://custom/\nmethod: POST\nenvelope: <x/>\n",
        )
        .unwrap();

        ingest_all(watch.path(), dsl.path()).unwrap();

        // Hand-written content should be preserved.
        let after = std::fs::read_to_string(&out).unwrap();
        assert!(after.contains("- custom"));
        assert!(after.contains("https://custom/"));
        // Marker must NOT have appeared — we didn't touch the file.
        assert!(!after.starts_with(MARKER));
    }

    #[test]
    fn previously_generated_dsl_gets_overwritten() {
        let watch = TempDir::new().unwrap();
        let dsl = TempDir::new().unwrap();
        let group_dir = watch.path().join("ex");
        std::fs::create_dir_all(&group_dir).unwrap();
        std::fs::write(group_dir.join("lookup.wsdl"), MINIMAL_WSDL).unwrap();

        // Simulate a previous generator run — file with marker.
        let out = dsl.path().join("ex").join("lookup.yml");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        let stale = format!("{MARKER}\nparams:\n  - stale\n");
        std::fs::write(&out, &stale).unwrap();

        ingest_all(watch.path(), dsl.path()).unwrap();

        let after = std::fs::read_to_string(&out).unwrap();
        // Fresh content wins (not "stale").
        assert!(!after.contains("- stale"));
        assert!(after.contains("- q"));
        assert!(after.starts_with(MARKER));
    }

    #[test]
    fn missing_watch_dir_is_a_warning_not_an_error() {
        let dsl = TempDir::new().unwrap();
        let bogus = std::path::PathBuf::from("/definitely/does/not/exist");
        // Should return Ok, not Err. Just logs a warning.
        ingest_all(&bogus, dsl.path()).unwrap();
    }

    #[test]
    fn xroad_meta_sidecar_triggers_xroad_envelope() {
        let watch = TempDir::new().unwrap();
        let dsl = TempDir::new().unwrap();
        let group_dir = watch.path().join("ex");
        std::fs::create_dir_all(&group_dir).unwrap();
        std::fs::write(group_dir.join("lookup.wsdl"), MINIMAL_WSDL).unwrap();
        std::fs::write(
            group_dir.join("lookup.meta.yaml"),
            "member_class: GOV\nmember_code: '70000000'\nsubsystem_code: mysub\n",
        )
        .unwrap();

        ingest_all(watch.path(), dsl.path()).unwrap();

        let contents = std::fs::read_to_string(dsl.path().join("ex").join("lookup.yml")).unwrap();
        assert!(contents.contains("{{{generate.client}}}"));
        assert!(contents.contains("<id:memberClass>GOV</id:memberClass>"));
    }

    #[test]
    fn malformed_wsdl_does_not_kill_other_wsdls() {
        let watch = TempDir::new().unwrap();
        let dsl = TempDir::new().unwrap();
        let group_dir = watch.path().join("ex");
        std::fs::create_dir_all(&group_dir).unwrap();
        std::fs::write(group_dir.join("good.wsdl"), MINIMAL_WSDL).unwrap();
        std::fs::write(group_dir.join("bad.wsdl"), "not xml at all").unwrap();

        // Should NOT return Err — the bad one is logged and
        // ingestion continues.
        ingest_all(watch.path(), dsl.path()).unwrap();

        // The good one still generated.
        assert!(dsl.path().join("ex").join("lookup.yml").exists());
    }
}
