//! The search corpus: which files each space contributes, and how a path maps back to a space.
//!
//! Every walk goes through `storage::iter_md`, so the §3.3 skip set (dot components, nested
//! space roots, files over 4 MiB, non-`.md`) applies uniformly. Foreign Markdown is included
//! with an empty `Meta`; a corrupt file is skipped silently — never an error, never a panic.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::domain::select::{Filter, SortKey};
use crate::fm::{read_doc, read_meta_only, Meta, Row};
use crate::search::SearchFilter;
use crate::spaces::Space;
use crate::storage::{iter_md, realpath};

/// The two task folders, each walked **non-recursively** (final.md §4.3).
pub const TASK_DIRS: [&str; 2] = ["open", "done"];

/// The spaces a Python-era vault had. When the corpus is exactly these, hits keep the legacy
/// key set and carry no `space` key.
pub const LEGACY_SPACES: [Space; 2] = [Space::Notes, Space::Tasks];

/// One corpus file: its path, frontmatter, body, and the space it was walked from.
#[derive(Clone, Debug)]
pub struct CorpusDoc {
    pub path: PathBuf,
    pub meta: Meta,
    pub body: String,
    pub space: Space,
}

/// The files one space contributes, in walk order.
///
/// Notes, memories, scratch and assets are recursive; tasks are `open/` then `done/`, each
/// non-recursive. A disabled or missing space yields nothing.
pub fn space_files(cfg: &Config, space: Space) -> Vec<PathBuf> {
    let Ok(root) = cfg.root(space) else {
        return Vec::new();
    };
    let excl = cfg.spaces.exclusions_for(space);
    match space {
        Space::Tasks => TASK_DIRS
            .iter()
            .flat_map(|dir| iter_md(&root.join(dir), false, excl))
            .collect(),
        _ => iter_md(root, true, excl).collect(),
    }
}

/// Every corpus file with its body, walked in space order.
pub fn docs(cfg: &Config, spaces: &[Space]) -> Vec<CorpusDoc> {
    let mut out: Vec<CorpusDoc> = Vec::new();
    for space in dedupe(spaces) {
        for path in space_files(cfg, space) {
            let Some(doc) = read_doc(&path) else {
                continue;
            };
            out.push(CorpusDoc {
                path,
                meta: doc.meta,
                body: doc.body,
                space,
            });
        }
    }
    out
}

/// Every corpus file as a frontmatter-only `Row`, walked in space order.
///
/// This is the cheap half of `docs`: no body is read into a `Row`, which is what a tag pull
/// and every lens want.
pub fn rows(cfg: &Config, spaces: &[Space]) -> Vec<Row> {
    rows_with_space(cfg, spaces)
        .into_iter()
        .map(|(row, _)| row)
        .collect()
}

/// `rows`, each paired with the space it was walked from.
pub fn rows_with_space(cfg: &Config, spaces: &[Space]) -> Vec<(Row, Space)> {
    let mut out: Vec<(Row, Space)> = Vec::new();
    for space in dedupe(spaces) {
        for path in space_files(cfg, space) {
            let Some(meta) = read_meta_only(&path) else {
                continue;
            };
            out.push((Row { path, meta }, space));
        }
    }
    out
}

/// A `path -> space` map over the requested spaces, for attributing an external ranker's hits.
pub fn space_map(cfg: &Config, spaces: &[Space]) -> HashMap<PathBuf, Space> {
    let mut out: HashMap<PathBuf, Space> = HashMap::new();
    for space in dedupe(spaces) {
        for path in space_files(cfg, space) {
            out.entry(path).or_insert(space);
        }
    }
    out
}

/// Which space owns `path`: the longest enabled root that contains it.
///
/// Roots are compared after `realpath`, so a symlinked space still matches. `None` when the
/// path is outside every enabled root.
pub fn space_of(cfg: &Config, path: &Path) -> Option<Space> {
    let resolved = realpath(path);
    let mut best: Option<(usize, Space)> = None;
    for space in Space::ALL {
        let Ok(root) = cfg.root(space) else {
            continue;
        };
        let root = realpath(root);
        if !resolved.starts_with(&root) {
            continue;
        }
        let depth = root.components().count();
        if best.is_none_or(|(d, _)| depth > d) {
            best = Some((depth, space));
        }
    }
    best.map(|(_, space)| space)
}

/// Whether hits should carry the `space` key.
///
/// True when `--space` was given explicitly, or when the corpus reaches past the legacy
/// notes+tasks pair — i.e. a requested non-legacy space exists on disk. A Python-era vault
/// with no `memories/` or `assets/` folder therefore emits the legacy key set.
pub fn emit_space_key(cfg: &Config, spaces: &[Space], explicit: bool) -> bool {
    if explicit {
        return true;
    }
    spaces.iter().any(|space| {
        !LEGACY_SPACES.contains(space) && cfg.root(*space).is_ok_and(|root| root.is_dir())
    })
}

/// The `domain::select` filter a `SearchFilter` denotes.
///
/// One translation, used by the tag pull, the built-in engine and the `indexed` post-filter,
/// so the three cannot drift.
pub fn base_filter(f: &SearchFilter) -> Filter {
    let filter = Filter {
        tags: if f.tags.is_empty() {
            None
        } else {
            Some(f.tags.clone())
        },
        any_tag: false,
        owner: f.owner.clone(),
        mine: false,
        me: None,
        cutoff: None,
        stale_cutoff: None,
        sort: SortKey::Updated,
        limit: Some(f.limit),
        extra: Vec::new(),
    };
    filter
        .with_extra("type", f.type_filter.as_deref())
        .with_extra("status", f.status.as_deref())
        .with_extra("kind", f.kind.as_deref())
}

/// Requested spaces with duplicates removed, order preserved.
fn dedupe(spaces: &[Space]) -> Vec<Space> {
    let mut out: Vec<Space> = Vec::new();
    for space in spaces {
        if !out.contains(space) {
            out.push(*space);
        }
    }
    out
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
    use crate::config::test_support::config_for;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn seed(root: &Path) {
        write(
            root,
            "notes/n-1.md",
            "---\nid: n-1\ntitle: One\n---\n\nbody one\n",
        );
        write(
            root,
            "notes/logs/n-2.md",
            "---\nid: n-2\ntitle: Two\n---\n\nbody two\n",
        );
        write(root, "notes/foreign.md", "no frontmatter here\n");
        write(root, "notes/n-bad.md", "---\ntitle: [unclosed\n---\n\nx\n");
        write(
            root,
            "tasks/open/t-1.md",
            "---\nid: t-1\ntitle: Task\n---\n\nt\n",
        );
        write(
            root,
            "tasks/done/t-2.md",
            "---\nid: t-2\ntitle: Done\n---\n\nd\n",
        );
        write(
            root,
            "tasks/open/nested/t-3.md",
            "---\nid: t-3\ntitle: Nested\n---\n\nn\n",
        );
        write(
            root,
            "memories/m-1.md",
            "---\nid: m-1\ntype: memory\ntitle: Mem\n---\n\nm\n",
        );
        write(
            root,
            "assets/a-1.md",
            "---\nid: a-1\ntype: asset\ntitle: Asset\n---\n\na\n",
        );
        write(root, "scratch/agent/x.md", "---\ntype: scratch\n---\n\nx\n");
        write(root, "notes/.obsidian/skip.md", "---\nid: n-9\n---\n\ns\n");
    }

    fn names(docs: &[CorpusDoc], root: &Path) -> Vec<String> {
        docs.iter()
            .map(|d| {
                d.path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn notes_are_recursive_and_tasks_are_not() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let cfg = config_for(dir.path());
        let docs = docs(&cfg, &[Space::Notes, Space::Tasks]);
        let listed = names(&docs, &crate::storage::realpath(dir.path()));
        assert!(listed.contains(&"notes/logs/n-2.md".to_string()));
        assert!(listed.contains(&"tasks/open/t-1.md".to_string()));
        assert!(listed.contains(&"tasks/done/t-2.md".to_string()));
        assert!(!listed.contains(&"tasks/open/nested/t-3.md".to_string()));
    }

    #[test]
    fn foreign_files_are_included_and_corrupt_files_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let cfg = config_for(dir.path());
        let docs = docs(&cfg, &[Space::Notes]);
        let listed = names(&docs, &crate::storage::realpath(dir.path()));
        assert!(listed.contains(&"notes/foreign.md".to_string()));
        assert!(!listed.contains(&"notes/n-bad.md".to_string()));
        let foreign = docs
            .iter()
            .find(|d| d.path.ends_with("foreign.md"))
            .unwrap();
        assert!(foreign.meta.is_empty());
        assert!(foreign.body.contains("no frontmatter"));
    }

    #[test]
    fn dot_directories_never_reach_the_corpus() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let cfg = config_for(dir.path());
        let listed = names(
            &docs(&cfg, &[Space::Notes]),
            &crate::storage::realpath(dir.path()),
        );
        assert!(!listed.iter().any(|p| p.contains(".obsidian")));
    }

    #[test]
    fn spaces_are_walked_in_the_order_requested() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let cfg = config_for(dir.path());
        let docs = docs(&cfg, &[Space::Memories, Space::Notes]);
        assert_eq!(docs.first().map(|d| d.space), Some(Space::Memories));
        assert!(docs.iter().any(|d| d.space == Space::Notes));
    }

    #[test]
    fn scratch_is_absent_unless_requested() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let cfg = config_for(dir.path());
        let default = docs(
            &cfg,
            &[Space::Notes, Space::Tasks, Space::Memories, Space::Assets],
        );
        assert!(!default.iter().any(|d| d.space == Space::Scratch));
        let opted_in = docs(&cfg, &[Space::Scratch]);
        assert_eq!(opted_in.len(), 1);
    }

    #[test]
    fn duplicate_spaces_are_walked_once() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let cfg = config_for(dir.path());
        let once = docs(&cfg, &[Space::Notes]).len();
        let twice = docs(&cfg, &[Space::Notes, Space::Notes]).len();
        assert_eq!(once, twice);
    }

    #[test]
    fn rows_carry_frontmatter_without_bodies() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let cfg = config_for(dir.path());
        let rows = rows(&cfg, &[Space::Notes]);
        assert!(rows.iter().any(|r| r.meta.get("id").is_some()));
        assert_eq!(rows.len(), docs(&cfg, &[Space::Notes]).len());
    }

    #[test]
    fn space_of_picks_the_longest_matching_root() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let cfg = config_for(dir.path());
        let root = crate::storage::realpath(dir.path());
        assert_eq!(
            space_of(&cfg, &root.join("notes/n-1.md")),
            Some(Space::Notes)
        );
        assert_eq!(
            space_of(&cfg, &root.join("tasks/open/t-1.md")),
            Some(Space::Tasks)
        );
        assert_eq!(space_of(&cfg, Path::new("/definitely/not/here.md")), None);
    }

    #[test]
    fn emit_space_key_is_off_for_a_legacy_vault() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "notes/n-1.md", "---\nid: n-1\n---\n\nx\n");
        write(dir.path(), "tasks/open/t-1.md", "---\nid: t-1\n---\n\nx\n");
        let cfg = config_for(dir.path());
        let all = [Space::Notes, Space::Tasks, Space::Memories, Space::Assets];
        assert!(!emit_space_key(&cfg, &all, false));
        assert!(emit_space_key(&cfg, &all, true));
    }

    #[test]
    fn emit_space_key_is_on_once_a_non_legacy_space_exists() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let cfg = config_for(dir.path());
        let all = [Space::Notes, Space::Tasks, Space::Memories, Space::Assets];
        assert!(emit_space_key(&cfg, &all, false));
    }

    #[test]
    fn base_filter_maps_every_conjunctive_predicate() {
        let f = SearchFilter {
            tags: vec!["a".into()],
            type_filter: Some("log".into()),
            status: Some("open".into()),
            kind: Some("fact".into()),
            owner: Some("me".into()),
            limit: 3,
            ..SearchFilter::default()
        };
        let filter = base_filter(&f);
        assert_eq!(filter.tags.as_deref(), Some(["a".to_string()].as_slice()));
        assert!(!filter.any_tag);
        assert_eq!(filter.owner.as_deref(), Some("me"));
        assert_eq!(filter.limit, Some(3));
        let keys: Vec<&str> = filter.extra.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["type", "status", "kind"]);
    }

    #[test]
    fn base_filter_drops_an_empty_tag_list() {
        let filter = base_filter(&SearchFilter::default());
        assert!(filter.tags.is_none());
        assert!(filter.extra.is_empty());
    }

    #[test]
    fn a_disabled_space_contributes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let mut cfg = config_for(dir.path());
        cfg.spaces = crate::spaces::Spaces::resolve(
            dir.path(),
            &[(Space::Memories, crate::spaces::SpaceSetting::Disabled)],
        )
        .unwrap();
        assert!(space_files(&cfg, Space::Memories).is_empty());
        assert!(docs(&cfg, &[Space::Memories]).is_empty());
    }
}
