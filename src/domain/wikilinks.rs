//! Wikilink derivation: `[[target]]` in a body becomes `related` ids. Bodies are never rewritten.

use std::collections::HashMap;

use crate::config::Config;
use crate::fm::read_meta_only;
use crate::model::common::{meta_str, meta_strings};
use crate::spaces::Space;
use crate::storage::iter_md;
use crate::text::{is_id_form, link_targets};

/// The exact-title index over the notes space: `title -> n-id`, first sorted file wins.
///
/// Titles resolve to notes by contract; memory and asset titles are sentences and would
/// collide constantly, so they are never title targets.
pub fn title_index(cfg: &Config) -> HashMap<String, String> {
    let mut index: HashMap<String, String> = HashMap::new();
    let Ok(root) = cfg.root(Space::Notes) else {
        return index;
    };
    for path in iter_md(root, true, cfg.spaces.exclusions_for(Space::Notes)) {
        let Some(meta) = read_meta_only(&path) else {
            continue;
        };
        let (Some(id), Some(title)) = (meta_str(&meta, "id"), meta_str(&meta, "title")) else {
            continue;
        };
        if !id.starts_with("n-") {
            continue;
        }
        index
            .entry(title.to_string())
            .or_insert_with(|| id.to_string());
    }
    index
}

/// Derive `related` from a body. The body is returned to the caller untouched.
///
/// Id-form targets pass through with no file lookup; title-form targets resolve against the
/// notes index, which is built lazily and only when a title-form target is present.
pub fn resolve_wikilinks(cfg: &Config, body: &str) -> Vec<String> {
    let targets = link_targets(body);
    if targets.is_empty() {
        return Vec::new();
    }
    let mut index: Option<HashMap<String, String>> = None;
    let mut related: Vec<String> = Vec::new();
    for target in targets {
        let resolved = if is_id_form(&target) {
            target
        } else {
            let idx = index.get_or_insert_with(|| title_index(cfg));
            match idx.get(&target) {
                Some(id) => id.clone(),
                None => continue,
            }
        };
        if !related.contains(&resolved) {
            related.push(resolved);
        }
    }
    related
}

/// Every link target in the vault that resolves to nothing, first-seen order, deduped.
///
/// Scans notes (recursive) then `tasks/open` and `tasks/done` (non-recursive), id-bearing
/// files only.
pub fn find_dangling(cfg: &Config) -> Vec<String> {
    let index = title_index(cfg);
    let mut out: Vec<String> = Vec::new();
    for path in scan_paths(cfg) {
        let Some(doc) = crate::fm::read_doc(&path) else {
            continue;
        };
        let Some(id) = meta_str(&doc.meta, "id") else {
            continue;
        };
        if !(id.starts_with("n-") || id.starts_with("t-")) {
            continue;
        }
        for target in link_targets(&doc.body) {
            if is_id_form(&target) || index.contains_key(&target) || out.contains(&target) {
                continue;
            }
            out.push(target);
        }
    }
    out
}

/// The ids of notes whose body contains a title-form link to `title`.
///
/// Used by `note update --title` to warn that a rename dangles existing links.
pub fn backlinks_by_title(cfg: &Config, title: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Ok(root) = cfg.root(Space::Notes) else {
        return out;
    };
    for path in iter_md(root, true, cfg.spaces.exclusions_for(Space::Notes)) {
        let Some(doc) = crate::fm::read_doc(&path) else {
            continue;
        };
        let Some(id) = meta_str(&doc.meta, "id") else {
            continue;
        };
        if !id.starts_with("n-") {
            continue;
        }
        if link_targets(&doc.body).iter().any(|t| t == title) && !out.contains(&id.to_string()) {
            out.push(id.to_string());
        }
    }
    out
}

/// The ids named by another entity's `related` list, keyed by target id (the inbound index).
pub fn inbound_from_rows(rows: &[crate::fm::Row]) -> HashMap<String, Vec<String>> {
    let mut index: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let Some(id) = meta_str(&row.meta, "id") else {
            continue;
        };
        for target in meta_strings(&row.meta, "related") {
            index.entry(target).or_default().push(id.to_string());
        }
    }
    for sources in index.values_mut() {
        sources.sort();
        sources.dedup();
    }
    index
}

fn scan_paths(cfg: &Config) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(notes) = cfg.root(Space::Notes) {
        paths.extend(iter_md(
            notes,
            true,
            cfg.spaces.exclusions_for(Space::Notes),
        ));
    }
    if let Ok(tasks) = cfg.root(Space::Tasks) {
        for sub in ["open", "done"] {
            paths.extend(iter_md(&tasks.join(sub), false, &[]));
        }
    }
    paths
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::config::test_support::config_for;
    use crate::config::Config;

    fn vault_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        for (rel, contents) in files {
            let path = dir.path().join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, contents).unwrap();
        }
        let cfg = config_for(dir.path());
        (dir, cfg)
    }

    fn note(id: &str, title: &str, body: &str) -> String {
        format!("---\nid: {id}\ntype: note\ntitle: {title}\n---\n\n{body}\n")
    }

    #[test]
    fn title_index_is_exact_and_first_sorted_file_wins() {
        let (_d, cfg) = vault_with(&[
            ("notes/n-A.md", &note("n-A", "Alpha", "")),
            ("notes/n-B.md", &note("n-B", "Alpha", "")),
            ("notes/foreign.md", "# Alpha\n"),
        ]);
        let index = title_index(&cfg);
        assert_eq!(index.get("Alpha").map(String::as_str), Some("n-A"));
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn id_form_passes_through_without_a_lookup() {
        let (_d, cfg) = vault_with(&[]);
        assert_eq!(
            resolve_wikilinks(&cfg, "see [[t-99]] and [[m-1]]"),
            ["t-99", "m-1"]
        );
    }

    #[test]
    fn title_form_resolves_and_dangling_is_omitted() {
        let (_d, cfg) = vault_with(&[("notes/n-A.md", &note("n-A", "Alpha", ""))]);
        assert_eq!(
            resolve_wikilinks(&cfg, "[[Alpha|x]] and [[Ghost]] and [[Alpha#h]]"),
            ["n-A"]
        );
    }

    #[test]
    fn find_dangling_reports_normalised_targets_once() {
        let (_d, cfg) = vault_with(&[
            (
                "notes/n-A.md",
                &note("n-A", "Alpha", "[[Ghost#Sec|display]] [[Alpha]] [[Ghost]]"),
            ),
            ("notes/foreign.md", "[[Invisible]]"),
        ]);
        assert_eq!(find_dangling(&cfg), ["Ghost"]);
    }

    #[test]
    fn backlinks_by_title_finds_the_linkers() {
        let (_d, cfg) = vault_with(&[
            ("notes/n-A.md", &note("n-A", "Alpha", "")),
            ("notes/n-B.md", &note("n-B", "Beta", "link to [[Alpha]]")),
        ]);
        assert_eq!(backlinks_by_title(&cfg, "Alpha"), ["n-B"]);
        assert!(backlinks_by_title(&cfg, "Nobody").is_empty());
    }
}
