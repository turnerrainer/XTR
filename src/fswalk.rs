//! One directory walk for sidecar and WSDL discovery (`inbound`, `xmllane`,
//! `doctor`).
//!
//! * A directory is entered once. The walk tracks directories by canonical
//!   path, so a symlink cycle (`loop -> .`) adds nothing.
//! * A symlink to a directory is followed only when the target is inside the
//!   root. A link out of the root (`x -> /`, `x -> ../../other`) is ignored.
//! * A symlink to a file is listed only when the target is inside the root.
//! * The walk fails when it reads more than [`MAX_ENTRIES`] directories and
//!   files. A partial list would drop lanes without notice.
//! * Symlinks to files and to directories inside the root still work. Kubernetes volume mounts
//!   use them.
//! * A directory whose name starts with `.` is skipped. This covers the
//!   `..data` and `..<timestamp>` directories of a Kubernetes ConfigMap mount,
//!   which hold the same files a second time.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Upper limit of directories and files the walk reads.
pub const MAX_ENTRIES: usize = 50_000;

/// All files under `root`, sorted. The result has no path twice.
/// `Err` when the walk reads more than [`MAX_ENTRIES`] entries.
pub fn files(root: &Path) -> Result<Vec<PathBuf>, String> {
    files_limited(root, MAX_ENTRIES)
}

fn files_limited(root: &Path, limit: usize) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let Ok(canonical_root) = root.canonicalize() else {
        return Ok(out);
    };
    seen.insert(canonical_root.clone());
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for p in rd.flatten().map(|e| e.path()) {
            if out.len() + seen.len() > limit {
                return Err(format!(
                    "{}: more than {limit} directories and files; remove what is not a lane",
                    root.display()
                ));
            }
            if p.is_dir() {
                let hidden = p
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|n| n.starts_with('.'));
                if !hidden
                    && p.canonicalize()
                        .is_ok_and(|c| c.starts_with(&canonical_root) && seen.insert(c))
                {
                    stack.push(p);
                }
            } else if p
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                // A link to a file: keep it only when the target is a file inside the root.
                if p.canonicalize()
                    .is_ok_and(|c| c.is_file() && c.starts_with(&canonical_root))
                {
                    out.push(p);
                }
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    fn names(root: &Path) -> Vec<String> {
        files(root)
            .unwrap()
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().display().to_string())
            .collect()
    }

    #[test]
    fn one_cycle_adds_nothing() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("g/a.yaml"));
        symlink(".", d.path().join("g/loop")).unwrap();
        assert_eq!(names(d.path()), ["g/a.yaml"]);
    }

    #[test]
    fn many_cycles_finish_at_once() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("g/a.yaml"));
        for i in 0..6 {
            symlink(".", d.path().join(format!("g/loop{i}"))).unwrap();
            symlink("..", d.path().join(format!("g/up{i}"))).unwrap();
        }
        assert_eq!(names(d.path()), ["g/a.yaml"]);
    }

    #[test]
    fn a_cycle_through_two_directories_adds_nothing() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("a/f.yaml"));
        std::fs::create_dir_all(d.path().join("b")).unwrap();
        symlink("../b", d.path().join("a/to_b")).unwrap();
        symlink("../a", d.path().join("b/to_a")).unwrap();
        assert_eq!(names(d.path()), ["a/f.yaml"]);
    }

    /// A symlink must not lead out of the root: files outside are not ours.
    #[test]
    fn a_symlink_out_of_the_root_is_not_followed() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("wsdl");
        touch(&root.join("g/a.yaml"));
        touch(&d.path().join("outside/secret.yaml"));
        symlink("../../outside", root.join("g/escape")).unwrap();
        assert_eq!(names(&root), ["g/a.yaml"]);
    }

    /// A file symlink out of the root is not listed (its content is not ours).
    #[test]
    fn a_file_symlink_out_of_the_root_is_not_listed() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("wsdl");
        touch(&root.join("g/a.yaml"));
        touch(&d.path().join("secret.yaml"));
        symlink("../../secret.yaml", root.join("g/stolen.yaml")).unwrap();
        assert_eq!(names(&root), ["g/a.yaml"]);
    }

    #[test]
    fn a_symlink_to_the_filesystem_root_is_not_followed() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("g/a.yaml"));
        symlink("/", d.path().join("g/everything")).unwrap();
        assert_eq!(names(d.path()), ["g/a.yaml"]);
    }

    #[test]
    fn too_many_entries_is_an_error_not_a_partial_list() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..20 {
            touch(&d.path().join(format!("g/f{i}.yaml")));
        }
        assert!(files_limited(d.path(), 10).is_err());
        assert_eq!(files_limited(d.path(), 100).unwrap().len(), 20);
    }

    #[test]
    fn symlinked_files_are_listed() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("g/real.yaml"));
        symlink("real.yaml", d.path().join("g/link.yaml")).unwrap();
        assert_eq!(names(d.path()), ["g/link.yaml", "g/real.yaml"]);
    }

    /// Kubernetes ConfigMap layout: `g -> ..data/g`, `..data -> ..ts`, real
    /// files only in `..ts`. The group appears once, under its public name.
    #[test]
    fn configmap_mount_layout_lists_each_file_once() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("..2026_11_12/g/x.xml.yaml"));
        symlink("..2026_11_12", d.path().join("..data")).unwrap();
        symlink("..data/g", d.path().join("g")).unwrap();
        assert_eq!(names(d.path()), ["g/x.xml.yaml"]);
    }
}
