//! The one vault walk. Nothing else enumerates Markdown files.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Files larger than this are skipped by every walk.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Every `*.md` under `root`, sorted, skipping dot components, `excl` subtrees and huge files.
///
/// `recursive == false` looks only at `root` itself. A missing root yields nothing.
pub fn iter_md(root: &Path, recursive: bool, excl: &[PathBuf]) -> std::vec::IntoIter<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if !root.is_dir() {
        return out.into_iter();
    }
    let depth = if recursive { usize::MAX } else { 1 };
    let walker = WalkDir::new(root)
        .follow_links(false)
        .max_depth(depth)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            if is_dot(e.file_name()) {
                return false;
            }
            !excl
                .iter()
                .any(|x| e.path() == x || e.path().starts_with(x))
        });
    for entry in walker.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
            continue;
        }
        out.push(path.to_path_buf());
    }
    out.sort();
    out.into_iter()
}

/// Whether [`iter_md`] over `root` would yield `path`: a `.md` file beneath `root`, no dot
/// component below it, within the size cap. A writer that targets a single file (adopt)
/// asks this so it never mints an entity the walk cannot find again.
pub fn walk_sees(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    path.extension().and_then(|e| e.to_str()) == Some("md")
        && !rel.components().any(|c| is_dot(c.as_os_str()))
        && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE_BYTES)
}

fn is_dot(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|s| s.starts_with('.'))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn names(root: &Path, recursive: bool, excl: &[PathBuf]) -> Vec<String> {
        iter_md(root, recursive, excl)
            .filter_map(|p| {
                p.strip_prefix(root)
                    .ok()
                    .map(|r| r.to_string_lossy().into_owned())
            })
            .collect()
    }

    #[test]
    fn skips_dot_components_and_non_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        std::fs::create_dir_all(root.join(".locks")).unwrap();
        std::fs::create_dir_all(root.join("logs")).unwrap();
        std::fs::write(root.join("a.md"), "x").unwrap();
        std::fs::write(root.join("b.txt"), "x").unwrap();
        std::fs::write(root.join(".obsidian/c.md"), "x").unwrap();
        std::fs::write(root.join(".locks/d.md"), "x").unwrap();
        std::fs::write(root.join("logs/e.md"), "x").unwrap();
        assert_eq!(names(root, true, &[]), ["a.md", "logs/e.md"]);
        assert_eq!(names(root, false, &[]), ["a.md"]);
    }

    #[test]
    fn walk_sees_agrees_with_the_walk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        std::fs::create_dir_all(root.join("logs")).unwrap();
        for name in ["a.md", "b.txt", ".obsidian/c.md", "logs/e.md"] {
            std::fs::write(root.join(name), "x").unwrap();
        }
        let big = root.join("big.md");
        std::fs::write(&big, vec![b'x'; (MAX_FILE_BYTES + 1) as usize]).unwrap();
        for name in [
            "a.md",
            "b.txt",
            ".obsidian/c.md",
            "logs/e.md",
            "big.md",
            "missing.md",
        ] {
            let seen = names(root, true, &[]).contains(&name.to_string());
            assert_eq!(walk_sees(root, &root.join(name)), seen, "{name}");
        }
        assert!(!walk_sees(&root.join("logs"), &root.join("a.md")));
    }

    #[test]
    fn honours_exclusions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("tasks/open")).unwrap();
        std::fs::write(root.join("a.md"), "x").unwrap();
        std::fs::write(root.join("tasks/open/t.md"), "x").unwrap();
        let excl = vec![root.join("tasks")];
        assert_eq!(names(root, true, &excl), ["a.md"]);
    }

    #[test]
    fn a_missing_root_yields_nothing() {
        assert_eq!(iter_md(Path::new("/nope/nothing"), true, &[]).count(), 0);
    }

    #[test]
    fn output_is_sorted() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["c.md", "a.md", "b.md"] {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }
        assert_eq!(names(dir.path(), true, &[]), ["a.md", "b.md", "c.md"]);
    }
}
