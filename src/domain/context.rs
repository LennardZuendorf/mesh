//! `build-context` and `graph`: BFS over `related`, plus the inbound index.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value as Json;

use crate::config::Config;
use crate::error::{MeshError, Result};
use crate::model::asset::ASSET_FIELDS;
use crate::model::memory::MEMORY_FIELDS;
use crate::model::note::NOTE_FIELDS;
use crate::model::task::TASK_FIELDS;
use crate::render;
use crate::spaces::Space;

/// The three edge directions `graph --direction` accepts.
pub const DIRECTIONS: [&str; 3] = ["out", "in", "both"];

/// The corpus the graph lenses read when no `--space` is given: `related` may now name an
/// `m-` id, and a dangling neighbour would otherwise silently vanish (final.md §5.9).
pub const DEFAULT_SPACES: [Space; 3] = [Space::Notes, Space::Tasks, Space::Memories];

/// A graph query result: the visited nodes, the link-direction edges, and the traversal tree.
#[derive(Clone, Debug, Default)]
pub struct GraphResult {
    pub seed: String,
    pub entries: Vec<serde_json::Value>,
    pub edges: Vec<(String, String)>,
    pub tree_edges: Vec<(String, String)>,
}

impl GraphResult {
    /// `{"seed", "nodes", "edges"}`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "seed": self.seed,
            "nodes": self.entries,
            "edges": self.edges.iter().map(|(a, b)| vec![a, b]).collect::<Vec<_>>(),
        })
    }

    /// Visited ids in BFS order, seed first.
    pub fn ids(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|e| e.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .collect()
    }

    /// The human tree: `"  " * depth + {id}\t{type}\t{title}`.
    ///
    /// A DFS pre-order over the **traversal** tree (who found whom), children in discovery
    /// order. Pure: no disk access, no second traversal.
    pub fn tree_lines(&self) -> Vec<String> {
        let mut index: HashMap<&str, &Json> = HashMap::new();
        for entry in &self.entries {
            if let Some(id) = entry.get("id").and_then(Json::as_str) {
                index.entry(id).or_insert(entry);
            }
        }
        let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
        for (parent, child) in &self.tree_edges {
            children
                .entry(parent.as_str())
                .or_default()
                .push(child.as_str());
        }
        let mut out: Vec<String> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack: Vec<(&str, usize)> = vec![(self.seed.as_str(), 0)];
        while let Some((id, hop)) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(entry) = index.get(id) else {
                continue;
            };
            out.push(tree_line(entry, hop));
            if let Some(kids) = children.get(id) {
                for kid in kids.iter().rev() {
                    stack.push((kid, hop + 1));
                }
            }
        }
        out
    }
}

fn text_field(entry: &Json, key: &str) -> String {
    entry
        .get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string()
}

fn tree_line(entry: &Json, hop: usize) -> String {
    format!(
        "{}{}\t{}\t{}",
        "  ".repeat(hop),
        text_field(entry, "id"),
        text_field(entry, "type"),
        text_field(entry, "title")
    )
}

/// The id of a node, as a string (`""` when absent or not a string).
fn node_id(entry: &Json) -> String {
    text_field(entry, "id")
}

/// Whether an id carries one of the space id prefixes.
fn is_mesh_id(id: &str) -> bool {
    Space::ALL
        .iter()
        .filter_map(|s| s.id_prefix())
        .any(|prefix| id.starts_with(prefix))
}

/// Resolve one id — or, in the notes space, a title slug — into a full node: the validated
/// frontmatter in declaration order plus `path`.
///
/// Routing is by id prefix, restricted to the spaces this query reads. Every failure (not
/// found, ambiguous, corrupt, invalid, disabled space) is a `None`, never an error: only the
/// seed is fatal, and its caller turns that `None` into `seed not found`.
pub fn resolve_entry(cfg: &Config, key: &str, spaces: &[Space]) -> Option<Json> {
    let enabled = |space: Space| spaces.contains(&space);
    if key.starts_with("t-") {
        if !enabled(Space::Tasks) {
            return None;
        }
        let view = crate::domain::tasks::get(cfg, key).ok()?;
        return Some(render::entry(
            &view.item.meta,
            TASK_FIELDS.fields(),
            None,
            Some(&view.path),
        ));
    }
    if key.starts_with("m-") {
        if !enabled(Space::Memories) {
            return None;
        }
        let view = crate::domain::memories::get(cfg, key).ok()?;
        return Some(render::entry(
            &view.item.meta,
            MEMORY_FIELDS.fields(),
            None,
            Some(&view.path),
        ));
    }
    if key.starts_with("a-") {
        if !enabled(Space::Assets) {
            return None;
        }
        let view = crate::domain::assets::get(cfg, key).ok()?;
        return Some(render::entry(
            &view.item.meta,
            ASSET_FIELDS.fields(),
            None,
            Some(&view.path),
        ));
    }
    if !enabled(Space::Notes) {
        return None;
    }
    // Everything else — `n-` ids and bare title slugs alike — goes to the notes space.
    let view = crate::domain::notes::get(cfg, key).ok()?;
    Some(render::entry(
        &view.item.meta,
        NOTE_FIELDS.fields(),
        None,
        Some(&view.path),
    ))
}

/// The reverse-`related` index over the default corpus.
pub fn inbound_index(cfg: &Config) -> HashMap<String, Vec<String>> {
    inbound_index_in(cfg, &DEFAULT_SPACES)
}

/// The reverse-`related` index: target id → the ids naming it, sorted ascending.
///
/// One whole-corpus pass. A file with no mesh id is never a valid source; a non-list
/// `related` contributes nothing; targets are the raw `related` entries, dangling ids
/// included (the map is simply never queried for them).
pub fn inbound_index_in(cfg: &Config, spaces: &[Space]) -> HashMap<String, Vec<String>> {
    let rows: Vec<crate::fm::Row> = crate::search::corpus_rows(cfg, spaces)
        .into_iter()
        .filter(|row| crate::model::common::meta_str(&row.meta, "id").is_some_and(is_mesh_id))
        .collect();
    crate::domain::wikilinks::inbound_from_rows(&rows)
}

/// The ids naming `target` in their `related` list, sorted ascending.
pub fn inbound_ids(cfg: &Config, target: &str) -> Vec<String> {
    inbound_index(cfg).get(target).cloned().unwrap_or_default()
}

/// The forward neighbours of a node: its `related` list, in list order.
fn out_candidates(entry: &Json) -> Vec<(String, bool)> {
    let Some(items) = entry.get("related").and_then(Json::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|v| {
            let key = v.as_str().map_or_else(|| v.to_string(), str::to_string);
            (key, false)
        })
        .collect()
}

/// The inbound neighbours of a node: whoever names it, sorted ascending.
fn in_candidates(
    entry: &Json,
    inbound: Option<&HashMap<String, Vec<String>>>,
) -> Vec<(String, bool)> {
    let Some(map) = inbound else {
        return Vec::new();
    };
    map.get(&node_id(entry))
        .map(|sources| sources.iter().map(|s| (s.clone(), true)).collect())
        .unwrap_or_default()
}

/// Out-candidates first under `both`, so the `seen` set suppresses the inbound duplicate of a
/// mutual link.
fn neighbour_candidates(
    entry: &Json,
    direction: &str,
    inbound: Option<&HashMap<String, Vec<String>>>,
) -> Vec<(String, bool)> {
    match direction {
        "in" => in_candidates(entry, inbound),
        "both" => {
            let mut out = out_candidates(entry);
            out.extend(in_candidates(entry, inbound));
            out
        }
        _ => out_candidates(entry),
    }
}

/// The shared traversal. `direction` is validated **before** the seed is resolved, so a bad
/// direction on a missing seed is exit 2, not exit 3.
fn bfs(
    cfg: &Config,
    seed_id: &str,
    depth: i64,
    direction: &str,
    spaces: &[Space],
) -> Result<GraphResult> {
    if !DIRECTIONS.contains(&direction) {
        return Err(MeshError::Validation(format!(
            "invalid direction: '{direction}' (use {})",
            DIRECTIONS.join(", ")
        )));
    }
    let seed = resolve_entry(cfg, seed_id, spaces)
        .ok_or_else(|| MeshError::SeedNotFound(seed_id.to_string()))?;

    // One pass, at most once per query, and never at depth 0.
    let inbound = if (direction == "in" || direction == "both") && depth > 0 {
        Some(inbound_index_in(cfg, spaces))
    } else {
        None
    };

    let resolved_seed = node_id(&seed);
    let mut result = GraphResult {
        seed: resolved_seed.clone(),
        ..GraphResult::default()
    };
    // Seeded with both the resolved id and the caller's string, so a slug seed cannot be
    // revisited through its own id.
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(resolved_seed);
    seen.insert(seed_id.to_string());

    let mut queue: VecDeque<(Json, i64)> = VecDeque::new();
    queue.push_back((seed, 0));
    while let Some((entry, hop)) = queue.pop_front() {
        let entry_id = node_id(&entry);
        if hop >= depth {
            result.entries.push(entry);
            continue;
        }
        let candidates = neighbour_candidates(&entry, direction, inbound.as_ref());
        result.entries.push(entry);
        for (key, reversed) in candidates {
            // Marked seen BEFORE resolution, so a dangling id is attempted exactly once.
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some(neighbour) = resolve_entry(cfg, &key, spaces) else {
                continue;
            };
            let neighbour_id = node_id(&neighbour);
            seen.insert(neighbour_id.clone());
            // `edges` are link-direction; `tree_edges` are traversal-direction.
            if reversed {
                result.edges.push((neighbour_id.clone(), entry_id.clone()));
            } else {
                result.edges.push((entry_id.clone(), neighbour_id.clone()));
            }
            result.tree_edges.push((entry_id.clone(), neighbour_id));
            queue.push_back((neighbour, hop + 1));
        }
    }
    Ok(result)
}

/// `build-context` over the default corpus.
pub fn build_context(cfg: &Config, seed: &str, depth: i64) -> Result<Vec<serde_json::Value>> {
    build_context_in(cfg, seed, depth, &DEFAULT_SPACES)
}

/// Forward-only BFS from `seed`: the visited nodes, seed first, deduped by id.
pub fn build_context_in(
    cfg: &Config,
    seed: &str,
    depth: i64,
    spaces: &[Space],
) -> Result<Vec<serde_json::Value>> {
    Ok(bfs(cfg, seed, depth, "out", spaces)?.entries)
}

/// `graph` over the default corpus.
pub fn graph_query(cfg: &Config, seed: &str, depth: i64, dir: &str) -> Result<GraphResult> {
    graph_query_in(cfg, seed, depth, dir, &DEFAULT_SPACES)
}

/// `graph` over an explicit space set.
pub fn graph_query_in(
    cfg: &Config,
    seed: &str,
    depth: i64,
    dir: &str,
    spaces: &[Space],
) -> Result<GraphResult> {
    bfs(cfg, seed, depth, dir, spaces)
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
    use std::fs;
    use std::path::Path;

    fn yaml_list(items: &[&str]) -> String {
        if items.is_empty() {
            " []".to_string()
        } else {
            format!(
                "\n{}",
                items
                    .iter()
                    .map(|r| format!("  - {r}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    }

    fn note(dir: &Path, id: &str, title: &str, related: &[&str]) {
        let text = format!(
            "---\nid: {id}\ntype: note\ntitle: {title}\ntags: []\nowner: null\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated:{}\n---\n\nbody\n",
            yaml_list(related)
        );
        let path = dir.join("notes").join(format!("{id}.md"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn task(dir: &Path, id: &str, related: &[&str]) {
        let text = format!(
            "---\nid: {id}\ntype: task\ntitle: {id}\ntags: []\nowner: null\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated:{}\n\
             status: open\npriority: null\nclaimed_by: null\nproject: null\n\
             blocks: []\nblocked_by: []\n---\n\nbody\n",
            yaml_list(related)
        );
        let path = dir.join("tasks/open").join(format!("{id}.md"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn ids(entries: &[Json]) -> Vec<String> {
        entries.iter().map(node_id).collect()
    }

    #[test]
    fn depth_zero_is_the_seed_alone() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["n-b"]);
        note(dir.path(), "n-b", "B", &[]);
        assert_eq!(ids(&build_context(&cfg, "n-a", 0).unwrap()), ["n-a"]);
        // A negative depth behaves like 0.
        assert_eq!(ids(&build_context(&cfg, "n-a", -1).unwrap()), ["n-a"]);
    }

    #[test]
    fn depth_one_takes_direct_neighbours_only() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["n-b", "n-b2"]);
        note(dir.path(), "n-b", "B", &[]);
        note(dir.path(), "n-b2", "B2", &["n-c"]);
        note(dir.path(), "n-c", "C", &[]);
        assert_eq!(
            ids(&build_context(&cfg, "n-a", 1).unwrap()),
            ["n-a", "n-b", "n-b2"]
        );
        assert_eq!(
            ids(&build_context(&cfg, "n-a", 2).unwrap()),
            ["n-a", "n-b", "n-b2", "n-c"]
        );
    }

    #[test]
    fn cycles_and_self_references_terminate() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["n-b"]);
        note(dir.path(), "n-b", "B", &["n-a"]);
        assert_eq!(ids(&build_context(&cfg, "n-a", 5).unwrap()), ["n-a", "n-b"]);
        let graph = graph_query(&cfg, "n-a", 5, "out").unwrap();
        assert_eq!(graph.edges, vec![("n-a".to_string(), "n-b".to_string())]);

        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-s", "S", &["n-s"]);
        assert_eq!(ids(&build_context(&cfg, "n-s", 3).unwrap()), ["n-s"]);
        assert!(graph_query(&cfg, "n-s", 3, "both")
            .unwrap()
            .edges
            .is_empty());
    }

    #[test]
    fn a_diamond_parents_the_shared_node_by_its_first_discoverer() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["n-b", "n-c"]);
        note(dir.path(), "n-b", "B", &["n-d"]);
        note(dir.path(), "n-c", "C", &["n-d"]);
        note(dir.path(), "n-d", "D", &[]);
        let graph = graph_query(&cfg, "n-a", 2, "out").unwrap();
        assert_eq!(graph.ids(), ["n-a", "n-b", "n-c", "n-d"]);
        assert_eq!(
            graph.edges,
            vec![
                ("n-a".to_string(), "n-b".to_string()),
                ("n-a".to_string(), "n-c".to_string()),
                ("n-b".to_string(), "n-d".to_string()),
            ]
        );
    }

    #[test]
    fn a_dangling_neighbour_is_skipped_and_only_the_seed_is_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["n-ghost", "n-ghost"]);
        assert_eq!(ids(&build_context(&cfg, "n-a", 2).unwrap()), ["n-a"]);
        let err = build_context(&cfg, "n-nope", 1).unwrap_err();
        assert_eq!(err.code(), 3);
        assert_eq!(err.to_string(), "seed not found: n-nope");
    }

    #[test]
    fn direction_is_validated_before_the_seed() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let err = graph_query(&cfg, "n-nope", 1, "sideways").unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            "invalid direction: 'sideways' (use out, in, both)"
        );
        assert_eq!(graph_query(&cfg, "n-nope", 1, "in").unwrap_err().code(), 3);
    }

    #[test]
    fn inbound_edges_keep_link_direction() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["n-b"]);
        note(dir.path(), "n-b", "B", &[]);
        let graph = graph_query(&cfg, "n-b", 1, "in").unwrap();
        assert_eq!(graph.ids(), ["n-b", "n-a"]);
        assert_eq!(graph.edges, vec![("n-a".to_string(), "n-b".to_string())]);
        assert_eq!(
            graph.tree_edges,
            vec![("n-b".to_string(), "n-a".to_string())]
        );
        assert_eq!(
            graph.tree_lines(),
            ["n-b\tnote\tB".to_string(), "  n-a\tnote\tA".to_string()]
        );
    }

    #[test]
    fn both_enumerates_out_first_and_a_mutual_link_yields_one_edge() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["n-b"]);
        note(dir.path(), "n-b", "B", &["n-a"]);
        let graph = graph_query(&cfg, "n-a", 1, "both").unwrap();
        assert_eq!(graph.ids(), ["n-a", "n-b"]);
        assert_eq!(graph.edges, vec![("n-a".to_string(), "n-b".to_string())]);
    }

    #[test]
    fn both_walks_out_and_in_neighbours() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["n-b"]);
        note(dir.path(), "n-b", "B", &[]);
        note(dir.path(), "n-c", "C", &["n-a"]);
        let graph = graph_query(&cfg, "n-a", 1, "both").unwrap();
        assert_eq!(graph.ids(), ["n-a", "n-b", "n-c"]);
        assert_eq!(
            graph.edges,
            vec![
                ("n-a".to_string(), "n-b".to_string()),
                ("n-c".to_string(), "n-a".to_string()),
            ]
        );
    }

    #[test]
    fn tasks_and_notes_resolve_together() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["t-x"]);
        task(dir.path(), "t-x", &["n-a"]);
        let entries = build_context(&cfg, "n-a", 1).unwrap();
        assert_eq!(ids(&entries), ["n-a", "t-x"]);
        assert_eq!(entries[1]["status"], Json::String("open".into()));
        assert!(entries[1]["path"].as_str().unwrap().ends_with("t-x.md"));
        assert_eq!(ids(&build_context(&cfg, "t-x", 1).unwrap()), ["t-x", "n-a"]);
    }

    #[test]
    fn a_title_slug_seed_reports_the_resolved_id() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "Alpha Note", &[]);
        let graph = graph_query(&cfg, "alpha-note", 1, "out").unwrap();
        assert_eq!(graph.seed, "n-a");
        assert_eq!(graph.ids(), ["n-a"]);
    }

    #[test]
    fn the_tree_is_dfs_pre_order_with_two_space_indents() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["n-b", "n-c"]);
        note(dir.path(), "n-b", "B", &["n-d"]);
        note(dir.path(), "n-c", "C", &[]);
        note(dir.path(), "n-d", "D", &[]);
        let lines = graph_query(&cfg, "n-a", 2, "out").unwrap().tree_lines();
        assert_eq!(
            lines,
            [
                "n-a\tnote\tA",
                "  n-b\tnote\tB",
                "    n-d\tnote\tD",
                "  n-c\tnote\tC",
            ]
        );
    }

    #[test]
    fn the_inbound_index_sorts_sources_and_ignores_id_less_files() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-z", "Z", &["n-t"]);
        note(dir.path(), "n-a", "A", &["n-t"]);
        note(dir.path(), "n-t", "T", &[]);
        fs::write(dir.path().join("notes/foreign.md"), "# no id\n").unwrap();
        let index = inbound_index(&cfg);
        assert_eq!(
            index.get("n-t").unwrap(),
            &["n-a".to_string(), "n-z".to_string()]
        );
        assert!(!index.contains_key("n-a"));
        assert_eq!(inbound_ids(&cfg, "n-t"), ["n-a", "n-z"]);
        assert!(inbound_ids(&cfg, "n-missing").is_empty());
    }

    #[test]
    fn to_json_carries_seed_nodes_and_edge_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["n-b"]);
        note(dir.path(), "n-b", "B", &[]);
        let payload = graph_query(&cfg, "n-a", 1, "out").unwrap().to_json();
        let keys: Vec<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["seed", "nodes", "edges"]);
        assert_eq!(payload["edges"], serde_json::json!([["n-a", "n-b"]]));
        assert_eq!(payload["nodes"][0]["title"], Json::String("A".into()));
    }

    #[test]
    fn a_space_set_gates_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", &["t-x"]);
        task(dir.path(), "t-x", &[]);
        let entries = build_context_in(&cfg, "n-a", 1, &[Space::Notes]).unwrap();
        assert_eq!(ids(&entries), ["n-a"]);
    }
}
