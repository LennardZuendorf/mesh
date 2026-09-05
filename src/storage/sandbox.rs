//! Path sandboxing across the multi-root space set.

use std::path::{Component, Path, PathBuf};

use crate::error::{MeshError, Result};
use crate::spaces::Spaces;

/// Canonicalise with realpath semantics, tolerating a non-existent tail.
///
/// The deepest existing ancestor is canonicalised and the remainder re-joined, so a path that
/// does not exist yet still resolves the way `os.path.realpath` does.
pub fn realpath(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    // `..` is resolved lexically first, so a non-existent tail cannot smuggle a component
    // past the sandbox check.
    let lexical = normalise(path);
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut head = lexical.clone();
    loop {
        if let Ok(resolved) = std::fs::canonicalize(&head) {
            let mut out = resolved;
            for component in tail.iter().rev() {
                out.push(component);
            }
            return out;
        }
        let (Some(name), Some(parent)) = (head.file_name(), head.parent()) else {
            return lexical;
        };
        tail.push(name.to_os_string());
        head = parent.to_path_buf();
    }
}

/// Lexically drop `.` and `..` components without touching the filesystem.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve `candidate` and accept it only when it sits inside an enabled space root.
///
/// A relative candidate resolves against the first sandbox root.
pub fn safe_resolve(spaces: &Spaces, candidate: &Path) -> Result<PathBuf> {
    let roots = spaces.sandbox();
    let base = roots
        .first()
        .map_or_else(|| spaces.vault().to_path_buf(), Clone::clone);
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base.join(candidate)
    };
    let resolved = realpath(&absolute);
    for root in roots {
        let root_real = realpath(root);
        if resolved == root_real || resolved.starts_with(&root_real) {
            return Ok(resolved);
        }
    }
    Err(MeshError::Validation(format!(
        "path escapes sandbox {}: {}",
        realpath(&base).display(),
        resolved.display()
    )))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::config::test_support::config_for;

    #[test]
    fn accepts_paths_inside_a_space() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notes")).unwrap();
        let cfg = config_for(dir.path());
        let inside = dir.path().join("notes/n-1.md");
        assert!(safe_resolve(&cfg.spaces, &inside).is_ok());
    }

    #[test]
    fn rejects_traversal_and_absolute_escapes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notes")).unwrap();
        let cfg = config_for(dir.path());
        let err = safe_resolve(&cfg.spaces, Path::new("/etc/passwd")).unwrap_err();
        assert_eq!(err.code(), 2);
        assert!(err.to_string().starts_with("path escapes sandbox "));
        assert!(safe_resolve(&cfg.spaces, Path::new("../../secret")).is_err());
        assert!(safe_resolve(&cfg.spaces, Path::new("notes/../../secret")).is_err());
    }

    #[test]
    fn a_missing_tail_still_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("notes/logs/n-9.md");
        let resolved = realpath(&target);
        assert!(resolved.ends_with("notes/logs/n-9.md"));
        assert!(resolved.is_absolute());
    }
}
