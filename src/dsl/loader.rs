//! DSL loader — walks `config.dsl_path` and populates the service map.
//!
//! File layout convention (per JVM XTR, unchanged):
//!
//! ```text
//! DSL/
//! ├── ar/
//! │   ├── lihtandmed_v3.yml       → POST /ar/lihtandmed_v3
//! │   └── ettevottegaSeotudIsikud_v1.yml
//! └── xroad/
//!     ├── listMethods.yml         → POST /xroad/listMethods
//!     └── allowedMethods.yml
//! ```
//!
//! Directory name → group; filename stem (before `.yml`/`.yaml`) →
//! service. URL becomes `POST /<group>/<service>`.

use crate::dsl::XRoadTemplate;
use crate::error::XtrError;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Key: `(group, service)`. Value: parsed DSL, `Arc`-wrapped so
/// clones for concurrent handlers are cheap.
pub type ServiceMap = HashMap<(String, String), Arc<XRoadTemplate>>;

/// Walk `root` recursively; parse every `.yml` / `.yaml` file into
/// an `XRoadTemplate`; index by `(parent-dir-name, file-stem)`.
/// Files whose group or stem cannot be extracted are skipped with
/// a warning.
pub fn load_all(root: &Path) -> Result<ServiceMap, XtrError> {
    let mut map = ServiceMap::new();
    if !root.exists() {
        tracing::warn!(
            "dsl_path {} does not exist; no services will be loaded",
            root.display()
        );
        return Ok(map);
    }
    walk(root, &mut map)?;
    tracing::info!("loaded {} DSL(s) from {}", map.len(), root.display());
    Ok(map)
}

fn walk(dir: &Path, out: &mut ServiceMap) -> Result<(), XtrError> {
    for entry in std::fs::read_dir(dir)
        .map_err(|e| XtrError::Internal(format!("reading dir {}: {}", dir.display(), e)))?
    {
        let entry = entry.map_err(|e| XtrError::Internal(format!("reading dir entry: {}", e)))?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if is_dsl_file(&path) {
            if let Some((group, service)) = derive_key(&path) {
                let template = parse_file(&path)?;
                // Fail fast on unparseable Handlebars — turns a
                // future first-request 500 into a startup error the
                // operator can fix before deploying.
                validate_template(&path, &template)?;
                let key = (group, service);
                if out.contains_key(&key) {
                    tracing::warn!(
                        "duplicate DSL {}/{} — the file at {} overrides an earlier load. \
                         Check for both .yml and .yaml, or a rename gone wrong.",
                        key.0,
                        key.1,
                        path.display()
                    );
                }
                tracing::debug!("loaded DSL: {}/{}", key.0, key.1);
                out.insert(key, Arc::new(template));
            } else {
                tracing::warn!(
                    "skipping {}: could not derive (group, service) key",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

/// Try to compile the DSL's Handlebars envelope so a bad template
/// blows up at startup, not on the first live request.
fn validate_template(path: &Path, tpl: &XRoadTemplate) -> Result<(), XtrError> {
    ::handlebars::Template::compile(&tpl.envelope).map_err(|e| {
        XtrError::Internal(format!(
            "DSL {}: envelope failed Handlebars validation: {}",
            path.display(),
            e
        ))
    })?;
    Ok(())
}

fn is_dsl_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("yml") | Some("yaml")
    )
}

fn derive_key(path: &Path) -> Option<(String, String)> {
    let group = path.parent()?.file_name()?.to_str()?.to_string();
    let service = path.file_stem()?.to_str()?.to_string();
    Some((group, service))
}

fn parse_file(path: &Path) -> Result<XRoadTemplate, XtrError> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| XtrError::Internal(format!("reading {}: {}", path.display(), e)))?;
    serde_yaml_ng::from_str(&body)
        .map_err(|e| XtrError::Internal(format!("parsing {}: {}", path.display(), e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let full = root.join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, body).unwrap();
    }

    #[test]
    fn walks_recursively_and_keys_by_group_and_stem() {
        let tmp = TempDir::new().unwrap();
        let dsl_body = "params:\n  - x\nmethod: POST\nenvelope: <soap>{{x}}</soap>\n";
        write(tmp.path(), "ar/lihtandmed_v3.yml", dsl_body);
        write(tmp.path(), "xroad/listMethods.yml", dsl_body);

        let map = load_all(tmp.path()).unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key(&("ar".into(), "lihtandmed_v3".into())));
        assert!(map.contains_key(&("xroad".into(), "listMethods".into())));
    }

    #[test]
    fn missing_dsl_path_is_a_warning_not_an_error() {
        let map = load_all(Path::new("/nope/does/not/exist")).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn accepts_yaml_extension_too() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "svc/foo.yaml",
            "params: []\nmethod: POST\nenvelope: <x/>\n",
        );
        let map = load_all(tmp.path()).unwrap();
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn skips_non_yaml_files() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "svc/readme.md", "not a dsl");
        write(
            tmp.path(),
            "svc/foo.yml",
            "params: []\nmethod: POST\nenvelope: <x/>\n",
        );
        let map = load_all(tmp.path()).unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&("svc".into(), "foo".into())));
    }

    #[test]
    fn parse_error_propagates() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "svc/broken.yml", "not: valid: yaml: :");
        let err = load_all(tmp.path()).unwrap_err();
        assert!(matches!(err, XtrError::Internal(_)));
    }

    #[test]
    fn broken_handlebars_envelope_fails_at_load_time() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "svc/bad.yml",
            "params: []\nmethod: POST\nenvelope: <x>{{ unclosed </x>\n",
        );
        let err = load_all(tmp.path()).unwrap_err();
        assert!(
            matches!(&err, XtrError::Internal(m) if m.contains("Handlebars")),
            "expected Handlebars-validation error, got {err:?}"
        );
    }

    #[test]
    fn duplicate_group_service_across_yml_yaml_warns() {
        // Same group + stem in two files (one .yml, one .yaml).
        // The second load wins silently today; we just want to
        // exercise the code path so a warning is emitted.
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "svc/foo.yml",
            "params: []\nmethod: POST\nenvelope: <a/>\n",
        );
        write(
            tmp.path(),
            "svc/foo.yaml",
            "params: []\nmethod: POST\nenvelope: <b/>\n",
        );
        let map = load_all(tmp.path()).unwrap();
        // Only one entry survives — the warning is behavioural
        // documentation; the last read wins by HashMap semantics.
        assert_eq!(map.len(), 1);
    }
}
