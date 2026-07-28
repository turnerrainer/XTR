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
                tracing::debug!("loaded DSL: {}/{}", group, service);
                out.insert((group, service), Arc::new(template));
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
}
