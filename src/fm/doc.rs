//! The one reader and the one writer for a mesh Markdown document.

use std::path::Path;

use crate::error::Result;
use crate::fm::emit::emit_meta;
use crate::fm::load::{parse_meta, split_frontmatter};
use crate::fm::value::Meta;
use crate::spaces::Spaces;
use crate::storage::{atomic_write, safe_resolve};

/// A parsed document: ordered frontmatter plus the trailing-trimmed body.
#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub meta: Meta,
    pub body: String,
}

impl Doc {
    /// A document with the given meta and body.
    pub fn new(meta: Meta, body: impl Into<String>) -> Self {
        Doc {
            meta,
            body: body.into(),
        }
    }
}

/// A validated typed view. `body` is `""` for every list result; only `get` populates it.
#[derive(Clone, Debug)]
pub struct View<T> {
    pub item: T,
    pub body: String,
    pub path: std::path::PathBuf,
}

/// `(path, frontmatter)` — the shared scan unit. Bodies are never carried in a `Row`.
#[derive(Clone, Debug)]
pub struct Row {
    pub path: std::path::PathBuf,
    pub meta: Meta,
}

/// The single safe reader. An io error, malformed YAML or non-UTF-8 content yields `None`.
pub fn read_doc(path: &Path) -> Option<Doc> {
    let text = std::fs::read_to_string(path).ok()?;
    let (yaml, body) = split_frontmatter(&text);
    let meta = match yaml {
        Some(block) => parse_meta(&block)?,
        None => Meta::new(),
    };
    Some(Doc { meta, body })
}

/// Read only the frontmatter, stopping at the closing `---`.
pub fn read_meta_only(path: &Path) -> Option<Meta> {
    let text = std::fs::read_to_string(path).ok()?;
    let (yaml, _) = split_frontmatter(&text);
    match yaml {
        Some(block) => parse_meta(&block),
        None => Some(Meta::new()),
    }
}

/// Read a body, or `""` when the file is unreadable.
pub fn read_body(path: &Path) -> String {
    read_doc(path).map(|d| d.body).unwrap_or_default()
}

/// Serialise a document: `---\n<yaml>---\n\n<body>` with exactly one trailing newline.
pub fn dump_doc(doc: &Doc) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&emit_meta(&doc.meta));
    out.push_str("---\n\n");
    out.push_str(doc.body.trim_end());
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out
}

/// Sandbox-check the destination, then write the document atomically.
pub fn write_doc(spaces: &Spaces, path: &Path, doc: &Doc) -> Result<()> {
    let resolved = safe_resolve(spaces, path)?;
    atomic_write(&resolved, &dump_doc(doc))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::fm::value::Value;

    #[test]
    fn dump_ends_with_exactly_one_newline() {
        let mut meta = Meta::new();
        meta.insert("id".into(), Value::str("n-1"));
        let doc = Doc::new(meta, "body\n\n\n");
        let text = dump_doc(&doc);
        assert_eq!(text, "---\nid: n-1\n---\n\nbody\n");
    }

    #[test]
    fn empty_body_still_terminates() {
        let mut meta = Meta::new();
        meta.insert("id".into(), Value::str("n-1"));
        assert_eq!(dump_doc(&Doc::new(meta, "")), "---\nid: n-1\n---\n");
    }

    #[test]
    fn read_round_trips_a_written_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("n-1.md");
        let mut meta = Meta::new();
        meta.insert("id".into(), Value::str("n-1"));
        meta.insert("tags".into(), Value::strings(["a"]));
        let doc = Doc::new(meta.clone(), "hello");
        std::fs::write(&path, dump_doc(&doc)).unwrap();
        let back = read_doc(&path).unwrap();
        assert_eq!(back.meta, meta);
        assert_eq!(back.body, "hello");
        assert_eq!(read_meta_only(&path).unwrap(), meta);
        assert_eq!(read_body(&path), "hello");
    }

    #[test]
    fn unreadable_paths_are_none() {
        let missing = Path::new("/nonexistent/does-not-exist.md");
        assert!(read_doc(missing).is_none());
        assert!(read_meta_only(missing).is_none());
        assert_eq!(read_body(missing), "");
    }
}
