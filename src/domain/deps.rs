//! The task dependency graph: derived readiness, edge mutation, cycles.
//!
//! `blocked_by` is authoritative; `blocks` is a best-effort mirror kept for readability and
//! forward traversal. Readiness is computed at read time from the union of both directions, so
//! no verb ever writes another task's file and a hand-edited one-sided edge still answers
//! correctly.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::config::Config;
use crate::domain::tasks;
use crate::domain::Filter;
use crate::error::{MeshError, Result};
use crate::fm::{read_doc, Row, Value, View};
use crate::model::common::{meta_str, meta_strings, ts_value};
use crate::model::task::{is_terminal, Task};
use crate::storage::{entity_lock, hold};
use crate::timefmt::now_utc;

/// How many candidates `task next --claim` will try before giving up on a conflict.
pub const NEXT_MAX_ATTEMPTS: usize = 3;

/// Whether a task is takeable, and why not when it is not.
#[derive(Clone, Debug, Default)]
pub struct Readiness {
    pub ready: bool,
    pub unsatisfied: Vec<String>,
    pub cycle: Option<Vec<String>>,
}

/// A best-effort mirror write that failed, reported on stderr rather than failing the verb.
#[derive(Clone, Debug)]
pub struct Warning(pub String);

/// One task's graph-relevant frontmatter.
#[derive(Clone, Debug, Default)]
struct Node {
    status: String,
    claimed: bool,
    blocks: Vec<String>,
    blocked_by: Vec<String>,
}

impl Node {
    fn satisfied(&self) -> bool {
        is_terminal(&self.status)
    }
}

/// Index a scan by id. Only `t-` ids participate; a missing id is a dangling reference.
fn index(rows: &[Row]) -> BTreeMap<String, Node> {
    let mut out: BTreeMap<String, Node> = BTreeMap::new();
    for row in rows {
        let Some(id) = meta_str(&row.meta, "id") else {
            continue;
        };
        if !id.starts_with("t-") {
            continue;
        }
        out.insert(
            id.to_string(),
            Node {
                status: meta_str(&row.meta, "status").unwrap_or("open").to_string(),
                claimed: meta_str(&row.meta, "claimed_by").is_some(),
                blocks: meta_strings(&row.meta, "blocks"),
                blocked_by: meta_strings(&row.meta, "blocked_by"),
            },
        );
    }
    out
}

/// `effective_blockers(T) = T.blocked_by ∪ { S : T ∈ S.blocks }`, sorted ascending.
fn blockers_of(nodes: &BTreeMap<String, Node>, id: &str) -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    if let Some(node) = nodes.get(id) {
        set.extend(node.blocked_by.iter().cloned());
    }
    for (other, node) in nodes {
        if other != id && node.blocks.iter().any(|t| t == id) {
            set.insert(other.clone());
        }
    }
    set.remove(id);
    set.into_iter().collect()
}

/// `dependents(X) = X.blocks ∪ { T : X ∈ T.blocked_by }`, sorted ascending.
fn dependents_of(nodes: &BTreeMap<String, Node>, id: &str) -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    if let Some(node) = nodes.get(id) {
        set.extend(node.blocks.iter().cloned());
    }
    for (other, node) in nodes {
        if other != id && node.blocked_by.iter().any(|b| b == id) {
            set.insert(other.clone());
        }
    }
    set.remove(id);
    set.into_iter().collect()
}

/// The union blocker adjacency of the whole graph, every node → its effective blockers.
fn adjacency(nodes: &BTreeMap<String, Node>) -> BTreeMap<String, Vec<String>> {
    let mut adj: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for id in nodes.keys() {
        adj.entry(id.clone()).or_default();
    }
    for (id, node) in nodes {
        for blocker in &node.blocked_by {
            if blocker != id {
                adj.entry(id.clone()).or_default().insert(blocker.clone());
                adj.entry(blocker.clone()).or_default();
            }
        }
        for blocked in &node.blocks {
            if blocked != id {
                adj.entry(blocked.clone()).or_default().insert(id.clone());
                adj.entry(id.clone()).or_default();
            }
        }
    }
    adj.into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect()
}

/// The union of the edges declared in both directions, sorted ascending, `id` itself removed.
pub fn effective_blockers(rows: &[Row], id: &str) -> Vec<String> {
    blockers_of(&index(rows), id)
}

/// Whether `id` is takeable, plus the blockers that stop it and any cycle it sits in.
///
/// `satisfied(B) ⇔ B.status ∈ {done, cancelled} ∨ B does not exist` — a dangling blocker fails
/// open, so a typo can never wedge a task permanently with no error surface.
pub fn readiness(rows: &[Row], id: &str) -> Readiness {
    let nodes = index(rows);
    let unsatisfied: Vec<String> = blockers_of(&nodes, id)
        .into_iter()
        .filter(|b| nodes.get(b).is_some_and(|n| !n.satisfied()))
        .collect();
    let ready = nodes
        .get(id)
        .is_some_and(|n| n.status == "open" && !n.claimed && unsatisfied.is_empty());
    // `ready()` never recurses: only the direct blockers' statuses are inspected, so a task
    // inside a cycle simply has a non-terminal blocker and is never ready.
    let cycle = cycles_in(&adjacency(&nodes))
        .into_iter()
        .find(|c| c.iter().any(|n| n == id));
    Readiness {
        ready,
        unsatisfied,
        cycle,
    }
}

/// The tasks that depend on `id`, in both directions, sorted ascending.
pub fn dependents(rows: &[Row], id: &str) -> Vec<String> {
    dependents_of(&index(rows), id)
}

/// The dependents of `finished` that are ready now — a report, never a write.
pub fn newly_ready(rows: &[Row], finished: &str) -> Vec<String> {
    let nodes = index(rows);
    dependents_of(&nodes, finished)
        .into_iter()
        .filter(|dep| {
            let unsatisfied = blockers_of(&nodes, dep)
                .into_iter()
                .any(|b| nodes.get(&b).is_some_and(|n| !n.satisfied()));
            !unsatisfied
                && nodes
                    .get(dep)
                    .is_some_and(|n| n.status == "open" && !n.claimed)
        })
        .collect()
}

/// Every cycle in the blocker graph, found by an iterative DFS with a colour map.
///
/// Cycles cannot be created through `block` / `new` / `update` (they are checked first), but
/// they can arrive by hand-editing or a merge, so `mesh status` reports them.
pub fn cycles(rows: &[Row]) -> Vec<Vec<String>> {
    cycles_in(&adjacency(&index(rows)))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Colour {
    White,
    Grey,
    Black,
}

/// Iterative DFS: no recursion, so a pathological graph cannot blow the stack.
fn cycles_in(adj: &BTreeMap<String, Vec<String>>) -> Vec<Vec<String>> {
    let mut colour: HashMap<&str, Colour> =
        adj.keys().map(|k| (k.as_str(), Colour::White)).collect();
    let mut found: Vec<Vec<String>> = Vec::new();
    let mut seen: HashSet<Vec<String>> = HashSet::new();

    for start in adj.keys() {
        if colour.get(start.as_str()) != Some(&Colour::White) {
            continue;
        }
        // (node, index of the next neighbour to visit)
        let mut stack: Vec<(&str, usize)> = vec![(start.as_str(), 0)];
        colour.insert(start.as_str(), Colour::Grey);
        while let Some(&mut (node, ref mut cursor)) = stack.last_mut() {
            let neighbours = adj.get(node).map(Vec::as_slice).unwrap_or_default();
            match neighbours.get(*cursor) {
                Some(next) => {
                    *cursor += 1;
                    match colour.get(next.as_str()).copied() {
                        Some(Colour::Grey) => {
                            // A back edge: the cycle is the grey suffix of the stack.
                            let at = stack.iter().position(|(n, _)| *n == next.as_str());
                            if let Some(at) = at {
                                let path: Vec<String> = stack
                                    .iter()
                                    .skip(at)
                                    .map(|(n, _)| (*n).to_string())
                                    .collect();
                                if seen.insert(canonical(&path)) {
                                    found.push(path);
                                }
                            }
                        }
                        Some(Colour::White) | None => {
                            colour.insert(next.as_str(), Colour::Grey);
                            stack.push((next.as_str(), 0));
                        }
                        Some(Colour::Black) => {}
                    }
                }
                None => {
                    colour.insert(node, Colour::Black);
                    stack.pop();
                }
            }
        }
    }
    found
}

/// A rotation-independent key for a cycle, so the same loop is reported once.
fn canonical(path: &[String]) -> Vec<String> {
    let Some(at) = path
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.cmp(b.1))
        .map(|(i, _)| i)
    else {
        return Vec::new();
    };
    let mut out: Vec<String> = path.iter().skip(at).cloned().collect();
    out.extend(path.iter().take(at).cloned());
    out
}

/// Blocker ids referenced by some task's `blocked_by` that resolve to no file.
///
/// They are *satisfied* for readiness (fail open) and counted here so the operator can see them.
pub fn dangling_blockers(rows: &[Row]) -> Vec<String> {
    let nodes = index(rows);
    let mut out: BTreeSet<String> = BTreeSet::new();
    for node in nodes.values() {
        for blocker in &node.blocked_by {
            if !nodes.contains_key(blocker) {
                out.insert(blocker.clone());
            }
        }
    }
    out.into_iter().collect()
}

/// Refuse a set of proposed `(blocked, blocker)` edges that would close a cycle.
///
/// `rows` is the **post-edit** graph, so a removal is never refused; only cycles that run
/// through one of the `add` edges are reported, so a pre-existing hand-made cycle elsewhere
/// never blocks an unrelated write.
pub fn check_acyclic(rows: &[Row], add: &[(String, String)]) -> Result<()> {
    if add.is_empty() {
        return Ok(());
    }
    let adj = adjacency(&index(rows));
    for (blocked, blocker) in add {
        if blocked == blocker {
            return Err(MeshError::Validation(format!(
                "a task cannot block itself: {blocked}"
            )));
        }
        // A cycle exists exactly when the blocker is itself (transitively) blocked by `blocked`.
        if let Some(path) = path_between(&adj, blocker, blocked) {
            // `path` runs blocker → … → blocked, so the loop is blocked → blocker → … → blocked.
            let mut chain: Vec<String> = vec![blocked.clone()];
            chain.extend(path);
            return Err(MeshError::Validation(format!(
                "dependency cycle: {}",
                chain.join(" -> ")
            )));
        }
    }
    Ok(())
}

/// The blocker-direction path from `from` to `to`, inclusive of both, or `None`.
fn path_between(adj: &BTreeMap<String, Vec<String>>, from: &str, to: &str) -> Option<Vec<String>> {
    let mut previous: HashMap<&str, &str> = HashMap::new();
    let mut visited: HashSet<&str> = HashSet::new();
    let mut queue: std::collections::VecDeque<&str> = std::collections::VecDeque::new();
    queue.push_back(from);
    visited.insert(from);
    while let Some(node) = queue.pop_front() {
        if node == to {
            let mut chain: Vec<String> = vec![node.to_string()];
            let mut cursor = node;
            while let Some(prev) = previous.get(cursor) {
                chain.push((*prev).to_string());
                cursor = prev;
            }
            chain.reverse();
            return Some(chain);
        }
        for next in adj.get(node).map(Vec::as_slice).unwrap_or_default() {
            if visited.insert(next.as_str()) {
                previous.insert(next.as_str(), node);
                queue.push_back(next.as_str());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------------------
// mirrors
// ---------------------------------------------------------------------------------------

/// One additive or subtractive change to another task's mirror list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MirrorEdit {
    /// The task whose file is edited.
    pub other: String,
    /// `blocks` or `blocked_by`.
    pub key: &'static str,
    /// The id added to or removed from that list.
    pub value: String,
    pub add: bool,
}

/// The mirror edits that make `id`'s new `blocks` / `blocked_by` lists consistent.
///
/// A replace retracts what it dropped as well as adding what it gained; the union readiness
/// rule would otherwise resurrect a removed edge from the stale mirror.
pub(crate) fn mirror_edits(
    id: &str,
    old_blocks: &[String],
    new_blocks: &[String],
    old_blocked_by: &[String],
    new_blocked_by: &[String],
) -> Vec<MirrorEdit> {
    let mut out: Vec<MirrorEdit> = Vec::new();
    let mut push = |other: &String, key: &'static str, add: bool| {
        if other == id {
            return;
        }
        let edit = MirrorEdit {
            other: other.clone(),
            key,
            value: id.to_string(),
            add,
        };
        if !out.contains(&edit) {
            out.push(edit);
        }
    };
    // T blocks B  ⇒  B.blocked_by gains T.
    for b in new_blocks.iter().filter(|b| !old_blocks.contains(b)) {
        push(b, "blocked_by", true);
    }
    for b in old_blocks.iter().filter(|b| !new_blocks.contains(b)) {
        push(b, "blocked_by", false);
    }
    // T blocked_by B  ⇒  B.blocks gains T.
    for b in new_blocked_by
        .iter()
        .filter(|b| !old_blocked_by.contains(b))
    {
        push(b, "blocks", true);
    }
    for b in old_blocked_by
        .iter()
        .filter(|b| !new_blocked_by.contains(b))
    {
        push(b, "blocks", false);
    }
    out
}

/// Apply mirror edits, one task at a time, under its own lock, in ascending id order.
///
/// Locks are never held simultaneously, so two concurrent `task block` calls cannot deadlock.
/// A failure is a warning, not an error: the authoritative side is already written and
/// readiness is computed from the union either way.
pub(crate) fn apply_mirrors(cfg: &Config, edits: Vec<MirrorEdit>) -> Vec<Warning> {
    let mut grouped: BTreeMap<String, Vec<MirrorEdit>> = BTreeMap::new();
    for edit in edits {
        grouped.entry(edit.other.clone()).or_default().push(edit);
    }
    let mut warnings: Vec<Warning> = Vec::new();
    for (other, edits) in grouped {
        if let Err(reason) = mirror_one(cfg, &other, &edits) {
            warnings.push(Warning(format!(
                "task block: could not mirror onto {other} ({reason})"
            )));
        }
    }
    warnings
}

fn mirror_one(cfg: &Config, other: &str, edits: &[MirrorEdit]) -> std::result::Result<(), String> {
    let root = tasks::root(cfg).map_err(|e| e.to_string())?;
    let _guard = hold(&entity_lock(root, other)).map_err(|_| "locked".to_string())?;
    let path = tasks::resolve(cfg, other).map_err(|_| "missing".to_string())?;
    let mut doc = read_doc(&path).ok_or_else(|| "unreadable".to_string())?;
    let mut changed = false;
    for edit in edits {
        let mut list = meta_strings(&doc.meta, edit.key);
        if edit.add {
            if !list.iter().any(|x| x == &edit.value) {
                list.push(edit.value.clone());
                changed = true;
            }
        } else if list.iter().any(|x| x == &edit.value) {
            list.retain(|x| x != &edit.value);
            changed = true;
        }
        if changed {
            doc.meta.insert(edit.key.to_string(), Value::strings(list));
        }
    }
    if !changed {
        return Ok(());
    }
    let now = now_utc();
    doc.meta.insert("updated".into(), ts_value(&now));
    tasks::validated(&doc.meta, other).map_err(|_| "corrupt".to_string())?;
    tasks::persist(cfg, &path, &doc).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------
// edge mutation
// ---------------------------------------------------------------------------------------

/// Validate a blocker id list against the task it is being attached to.
fn validate_targets(id: &str, on: &[String]) -> Result<()> {
    for target in on {
        if target == id {
            return Err(MeshError::Validation(format!(
                "a task cannot block itself: {id}"
            )));
        }
        if !target.starts_with("t-") || target.len() <= 2 {
            return Err(MeshError::Validation(format!(
                "invalid task id: '{target}'"
            )));
        }
    }
    Ok(())
}

/// Add blocking edges: additive, order-preserving dedupe, cycle-checked before any write.
pub fn block(cfg: &Config, id: &str, on: &[String]) -> Result<(Task, Vec<Warning>)> {
    validate_targets(id, on)?;
    if on.is_empty() {
        return Err(MeshError::Validation(
            "--on requires at least one task id".to_string(),
        ));
    }
    tasks::resolve(cfg, id)?;
    let before = tasks::rows(cfg);
    let (blocks, blocked_by) = tasks::node_lists(&before, id);
    let mut next = blocked_by.clone();
    for target in on {
        if !next.iter().any(|x| x == target) {
            next.push(target.clone());
        }
    }
    if next == blocked_by {
        // Adding an existing edge writes nothing and exits 0.
        return Ok((tasks::get(cfg, id)?.item, Vec::new()));
    }
    let added: Vec<(String, String)> = on
        .iter()
        .filter(|t| !blocked_by.contains(t))
        .map(|t| (id.to_string(), t.clone()))
        .collect();
    check_acyclic(&overlay(&before, id, &blocks, &next), &added)?;

    let task = write_blocked_by(cfg, id, &next)?;
    let warnings = apply_mirrors(cfg, mirror_edits(id, &[], &[], &blocked_by, &next));
    Ok((task, warnings))
}

/// Remove blocking edges. Idempotent; never cycle-checked, so breaking a cycle never fails.
pub fn unblock(cfg: &Config, id: &str, on: &[String], all: bool) -> Result<(Task, Vec<Warning>)> {
    if !all {
        validate_targets(id, on)?;
        if on.is_empty() {
            return Err(MeshError::Validation(
                "pass --on with task ids, or --all".to_string(),
            ));
        }
    }
    tasks::resolve(cfg, id)?;
    let before = tasks::rows(cfg);
    let (_, blocked_by) = tasks::node_lists(&before, id);
    // `--all` reaches every mirror, including one-sided edges declared only on the other side.
    let targets: Vec<String> = if all {
        effective_blockers(&before, id)
    } else {
        on.to_vec()
    };
    let next: Vec<String> = blocked_by
        .iter()
        .filter(|b| !targets.contains(b))
        .cloned()
        .collect();

    let task = if next == blocked_by {
        tasks::get(cfg, id)?.item
    } else {
        write_blocked_by(cfg, id, &next)?
    };
    let warnings = apply_mirrors(cfg, mirror_edits(id, &[], &[], &targets, &[]));
    Ok((task, warnings))
}

/// Write the authoritative `blocked_by` list under the task's own lock.
fn write_blocked_by(cfg: &Config, id: &str, list: &[String]) -> Result<Task> {
    let _guard = hold(&entity_lock(tasks::root(cfg)?, id))?;
    let path = tasks::resolve(cfg, id)?;
    let mut doc = read_doc(&path).ok_or_else(|| MeshError::TaskNotFound(id.to_string()))?;
    doc.meta
        .insert("blocked_by".into(), Value::strings(list.to_vec()));
    let now = now_utc();
    doc.meta.insert("updated".into(), ts_value(&now));
    let task = tasks::validated(&doc.meta, id)?;
    tasks::persist(cfg, &path, &doc)?;
    Ok(task)
}

/// A copy of `rows` in which `id` carries the given lists.
fn overlay(rows: &[Row], id: &str, blocks: &[String], blocked_by: &[String]) -> Vec<Row> {
    rows.iter()
        .map(|row| {
            if meta_str(&row.meta, "id") == Some(id) {
                let mut meta = row.meta.clone();
                meta.insert("blocks".into(), Value::strings(blocks.to_vec()));
                meta.insert("blocked_by".into(), Value::strings(blocked_by.to_vec()));
                Row {
                    path: row.path.clone(),
                    meta,
                }
            } else {
                row.clone()
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// task next
// ---------------------------------------------------------------------------------------

/// Pick the next ready task, optionally claiming it in the same invocation.
///
/// With `claim`, an exit-4 conflict re-selects across up to three candidates, so two agents
/// racing one queue both get work instead of one getting a conflict.
pub fn next(
    cfg: &Config,
    f: &Filter,
    claim: bool,
    strict: bool,
    claimer: Option<&str>,
) -> Result<Option<View<Task>>> {
    if !claim {
        return Ok(candidates(cfg, f)?.into_iter().next().map(hydrate(cfg)));
    }
    let Some(claimer) = claimer.filter(|c| !c.is_empty()) else {
        return Err(MeshError::Validation(
            "no agent identity: set [core].agent or pass --owner".to_string(),
        ));
    };
    let mut tried: Vec<String> = Vec::new();
    let mut last: Option<MeshError> = None;
    for _ in 0..NEXT_MAX_ATTEMPTS {
        let pool = candidates(cfg, f)?;
        let Some(view) = pool.into_iter().find(|v| !tried.contains(&v.item.id)) else {
            break;
        };
        tried.push(view.item.id.clone());
        match tasks::claim(cfg, &view.item.id, claimer, strict) {
            Ok((task, _)) => {
                return Ok(Some(View {
                    item: task,
                    body: crate::fm::read_body(&view.path),
                    path: view.path,
                }))
            }
            // A conflict means someone beat us to it: try the next candidate.
            Err(e) if e.code() == 4 => last = Some(e),
            // `--strict` on a candidate that turned blocked under us is exit 5, not a skip.
            Err(e) => return Err(e),
        }
    }
    match last {
        Some(e) => Err(e),
        None => Ok(None),
    }
}

/// The ready tasks matching `f`, in selection order.
fn candidates(cfg: &Config, f: &Filter) -> Result<Vec<View<Task>>> {
    tasks::list(cfg, f, tasks::Availability::Ready)
}

/// Fill in the body of a chosen view.
fn hydrate(cfg: &Config) -> impl Fn(View<Task>) -> View<Task> + '_ {
    move |view| {
        let _ = cfg;
        let body = crate::fm::read_body(&view.path);
        View { body, ..view }
    }
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
    use crate::domain::tasks::{NewTask, Terminal};
    use crate::domain::SortKey;
    use crate::fm::parse_meta;
    use std::path::PathBuf;

    fn row(id: &str, status: &str, blocks: &[&str], blocked_by: &[&str]) -> Row {
        let list = |v: &[&str]| {
            if v.is_empty() {
                "[]".to_string()
            } else {
                format!(
                    "\n{}",
                    v.iter()
                        .map(|x| format!("  - {x}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            }
        };
        Row {
            path: PathBuf::from(format!("/v/tasks/open/{id}.md")),
            meta: parse_meta(&format!(
                "id: {id}\ntype: task\ntitle: {id}\ncreated: 2026-01-01T00:00:00Z\n\
                 updated: 2026-01-01T00:00:00Z\nstatus: {status}\nblocks: {}\nblocked_by: {}\n",
                list(blocks),
                list(blocked_by)
            ))
            .unwrap(),
        }
    }

    fn vault() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        (dir, cfg)
    }

    #[test]
    fn blockers_are_the_union_of_both_directions() {
        let rows = vec![
            row("t-a", "open", &["t-b"], &[]),
            row("t-b", "open", &[], &["t-c"]),
            row("t-c", "open", &[], &[]),
        ];
        assert_eq!(effective_blockers(&rows, "t-b"), ["t-a", "t-c"]);
        assert!(effective_blockers(&rows, "t-a").is_empty());
    }

    #[test]
    fn a_terminal_blocker_is_satisfied_and_a_missing_one_fails_open() {
        let rows = vec![
            row("t-a", "done", &[], &[]),
            row("t-b", "cancelled", &[], &[]),
            row("t-x", "open", &[], &["t-a", "t-b", "t-gone"]),
        ];
        let r = readiness(&rows, "t-x");
        assert!(r.ready, "{r:?}");
        assert!(r.unsatisfied.is_empty());
        assert_eq!(dangling_blockers(&rows), ["t-gone"]);
    }

    #[test]
    fn readiness_needs_open_and_unclaimed() {
        let mut rows = vec![row("t-x", "claimed", &[], &[])];
        assert!(!readiness(&rows, "t-x").ready);
        rows = vec![row("t-x", "done", &[], &[])];
        assert!(!readiness(&rows, "t-x").ready);
        rows = vec![row("t-x", "open", &[], &[])];
        assert!(readiness(&rows, "t-x").ready);
        // An absent task is never ready.
        assert!(!readiness(&rows, "t-nope").ready);
    }

    #[test]
    fn an_open_task_carrying_a_stale_claim_is_not_ready() {
        let mut r = row("t-x", "open", &[], &[]);
        r.meta.insert("claimed_by".into(), Value::str("ghost"));
        assert!(!readiness(&[r], "t-x").ready);
    }

    #[test]
    fn unsatisfied_blockers_are_sorted_ascending() {
        let rows = vec![
            row("t-c", "open", &[], &[]),
            row("t-a", "open", &[], &[]),
            row("t-x", "open", &[], &["t-c", "t-a"]),
        ];
        assert_eq!(readiness(&rows, "t-x").unsatisfied, ["t-a", "t-c"]);
    }

    #[test]
    fn dependents_are_the_union_of_both_directions() {
        let rows = vec![
            row("t-a", "open", &["t-b"], &[]),
            row("t-b", "open", &[], &[]),
            row("t-c", "open", &[], &["t-a"]),
        ];
        assert_eq!(dependents(&rows, "t-a"), ["t-b", "t-c"]);
    }

    #[test]
    fn newly_ready_reports_only_the_dependents_that_became_takeable() {
        let rows = vec![
            row("t-a", "done", &[], &[]),
            row("t-b", "open", &[], &["t-a"]),
            row("t-c", "open", &[], &["t-a", "t-d"]),
            row("t-d", "open", &[], &[]),
            row("t-e", "claimed", &[], &["t-a"]),
        ];
        assert_eq!(newly_ready(&rows, "t-a"), ["t-b"]);
    }

    #[test]
    fn a_hand_made_cycle_is_reported_and_never_hangs() {
        let rows = vec![
            row("t-a", "open", &[], &["t-b"]),
            row("t-b", "open", &[], &["t-c"]),
            row("t-c", "open", &[], &["t-a"]),
        ];
        let found = cycles(&rows);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].len(), 3);
        // The read path is cycle-safe: no member is ready, nothing recurses.
        for id in ["t-a", "t-b", "t-c"] {
            let r = readiness(&rows, id);
            assert!(!r.ready);
            assert!(r.cycle.is_some());
        }
    }

    #[test]
    fn a_self_loop_is_reported_as_a_cycle_and_never_as_a_blocker() {
        let rows = vec![row("t-a", "open", &[], &["t-a"])];
        assert!(effective_blockers(&rows, "t-a").is_empty());
        assert!(readiness(&rows, "t-a").ready);
    }

    #[test]
    fn check_acyclic_refuses_a_closing_edge_with_the_back_edge_path() {
        let rows = vec![
            row("t-a", "open", &[], &["t-b"]),
            row("t-b", "open", &[], &["t-c"]),
            row("t-c", "open", &[], &[]),
        ];
        // Adding "t-c blocked_by t-a" closes a -> b -> c -> a.
        let post = vec![
            row("t-a", "open", &[], &["t-b"]),
            row("t-b", "open", &[], &["t-c"]),
            row("t-c", "open", &[], &["t-a"]),
        ];
        let err = check_acyclic(&post, &[("t-c".into(), "t-a".into())]).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            "dependency cycle: t-c -> t-a -> t-b -> t-c"
        );
        // Nothing added is always fine, even on a graph that already has a cycle.
        assert!(check_acyclic(&rows, &[]).is_ok());
    }

    #[test]
    fn check_acyclic_rejects_a_self_edge() {
        let err = check_acyclic(&[], &[("t-a".into(), "t-a".into())]).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "a task cannot block itself: t-a");
    }

    #[test]
    fn mirror_edits_add_what_is_gained_and_retract_what_is_dropped() {
        let edits = mirror_edits(
            "t-x",
            &["t-a".into()],
            &["t-b".into()],
            &[],
            &["t-c".into()],
        );
        assert_eq!(edits.len(), 3);
        assert!(edits.contains(&MirrorEdit {
            other: "t-b".into(),
            key: "blocked_by",
            value: "t-x".into(),
            add: true
        }));
        assert!(edits.contains(&MirrorEdit {
            other: "t-a".into(),
            key: "blocked_by",
            value: "t-x".into(),
            add: false
        }));
        assert!(edits.contains(&MirrorEdit {
            other: "t-c".into(),
            key: "blocks",
            value: "t-x".into(),
            add: true
        }));
        // A self-edge never becomes a mirror.
        assert!(mirror_edits("t-x", &[], &["t-x".into()], &[], &[]).is_empty());
    }

    #[test]
    fn block_is_additive_idempotent_and_mirrors() {
        let (_d, cfg) = vault();
        let a = crate::domain::tasks::create(&cfg, "A", NewTask::default()).unwrap();
        let b = crate::domain::tasks::create(&cfg, "B", NewTask::default()).unwrap();
        let (task, warnings) = block(&cfg, &b.id, std::slice::from_ref(&a.id)).unwrap();
        assert_eq!(task.blocked_by, [a.id.clone()][..]);
        assert!(warnings.is_empty());
        assert_eq!(
            crate::domain::tasks::get(&cfg, &a.id).unwrap().item.blocks,
            [b.id.clone()][..]
        );

        // Adding an existing edge writes nothing.
        let path = crate::domain::tasks::resolve(&cfg, &b.id).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        block(&cfg, &b.id, std::slice::from_ref(&a.id)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(!readiness(&crate::domain::tasks::rows(&cfg), &b.id).ready);
    }

    #[test]
    fn block_warns_rather_than_failing_when_a_mirror_is_missing() {
        let (_d, cfg) = vault();
        let b = crate::domain::tasks::create(&cfg, "B", NewTask::default()).unwrap();
        let (task, warnings) = block(&cfg, &b.id, &["t-GONE".into()]).unwrap();
        assert_eq!(task.blocked_by, ["t-GONE"]);
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            warnings[0].0,
            "task block: could not mirror onto t-GONE (missing)"
        );
        // A dangling blocker fails open.
        assert!(readiness(&crate::domain::tasks::rows(&cfg), &b.id).ready);
    }

    #[test]
    fn unblock_is_idempotent_and_all_clears_both_sides() {
        let (_d, cfg) = vault();
        let a = crate::domain::tasks::create(&cfg, "A", NewTask::default()).unwrap();
        let b = crate::domain::tasks::create(&cfg, "B", NewTask::default()).unwrap();
        block(&cfg, &b.id, std::slice::from_ref(&a.id)).unwrap();

        let path = crate::domain::tasks::resolve(&cfg, &b.id).unwrap();
        let (task, _) = unblock(&cfg, &b.id, &["t-NOPE".into()], false).unwrap();
        assert_eq!(task.blocked_by, [a.id.clone()][..]);
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(before.contains(&a.id));

        let (task, _) = unblock(&cfg, &b.id, &[], true).unwrap();
        assert!(task.blocked_by.is_empty());
        assert!(crate::domain::tasks::get(&cfg, &a.id)
            .unwrap()
            .item
            .blocks
            .is_empty());
        assert!(readiness(&crate::domain::tasks::rows(&cfg), &b.id).ready);
    }

    #[test]
    fn unblock_breaks_a_hand_made_cycle_without_being_refused() {
        let (_d, cfg) = vault();
        let a = crate::domain::tasks::create(&cfg, "A", NewTask::default()).unwrap();
        let b = crate::domain::tasks::create(&cfg, "B", NewTask::default()).unwrap();
        // Forge the cycle behind the verb's back.
        for (one, two) in [(&a, &b), (&b, &a)] {
            let path = crate::domain::tasks::resolve(&cfg, &one.id).unwrap();
            let mut doc = read_doc(&path).unwrap();
            doc.meta
                .insert("blocked_by".into(), Value::strings([two.id.clone()]));
            crate::domain::tasks::persist(&cfg, &path, &doc).unwrap();
        }
        assert_eq!(cycles(&crate::domain::tasks::rows(&cfg)).len(), 1);
        assert!(unblock(&cfg, &a.id, std::slice::from_ref(&b.id), false).is_ok());
        assert!(cycles(&crate::domain::tasks::rows(&cfg)).is_empty());
    }

    #[test]
    fn block_rejects_a_self_edge_and_a_malformed_id() {
        let (_d, cfg) = vault();
        let a = crate::domain::tasks::create(&cfg, "A", NewTask::default()).unwrap();
        let err = block(&cfg, &a.id, std::slice::from_ref(&a.id)).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            format!("a task cannot block itself: {}", a.id)
        );
        let err = block(&cfg, &a.id, &["nope".into()]).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "invalid task id: 'nope'");
    }

    #[test]
    fn block_refuses_a_cycle_before_any_write() {
        let (_d, cfg) = vault();
        let a = crate::domain::tasks::create(&cfg, "A", NewTask::default()).unwrap();
        let b = crate::domain::tasks::create(&cfg, "B", NewTask::default()).unwrap();
        block(&cfg, &b.id, std::slice::from_ref(&a.id)).unwrap();
        let path = crate::domain::tasks::resolve(&cfg, &a.id).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let err = block(&cfg, &a.id, std::slice::from_ref(&b.id)).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            format!("dependency cycle: {} -> {} -> {}", a.id, b.id, a.id)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn next_picks_by_priority_then_fifo_and_can_claim() {
        let (_d, cfg) = vault();
        let low = crate::domain::tasks::create(
            &cfg,
            "low",
            NewTask {
                priority: Some("low".into()),
                ..NewTask::default()
            },
        )
        .unwrap();
        let high = crate::domain::tasks::create(
            &cfg,
            "high",
            NewTask {
                priority: Some("high".into()),
                ..NewTask::default()
            },
        )
        .unwrap();
        let f = Filter {
            sort: SortKey::Priority,
            ..Filter::unbounded()
        };
        let picked = next(&cfg, &f, false, false, None).unwrap().unwrap();
        assert_eq!(picked.item.id, high.id);

        let claimed = next(&cfg, &f, true, false, Some("alice")).unwrap().unwrap();
        assert_eq!(claimed.item.id, high.id);
        assert_eq!(claimed.item.status, "claimed");
        // The next call falls through to the low-priority task.
        let claimed = next(&cfg, &f, true, false, Some("alice")).unwrap().unwrap();
        assert_eq!(claimed.item.id, low.id);
        // Nothing left.
        assert!(next(&cfg, &f, true, false, Some("alice"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn next_skips_blocked_and_claimed_tasks() {
        let (_d, cfg) = vault();
        let a = crate::domain::tasks::create(&cfg, "A", NewTask::default()).unwrap();
        let b = crate::domain::tasks::create(
            &cfg,
            "B",
            NewTask {
                blocked_by: vec![a.id.clone()],
                ..NewTask::default()
            },
        )
        .unwrap();
        crate::domain::tasks::claim(&cfg, &a.id, "someone", false).unwrap();
        let f = Filter {
            sort: SortKey::Priority,
            ..Filter::unbounded()
        };
        assert!(next(&cfg, &f, false, false, None).unwrap().is_none());
        crate::domain::tasks::terminate(&cfg, &a.id, Terminal::Finish, None, None).unwrap();
        assert_eq!(
            next(&cfg, &f, false, false, None).unwrap().unwrap().item.id,
            b.id
        );
    }

    #[test]
    fn next_with_claim_requires_an_identity() {
        let (_d, cfg) = vault();
        crate::domain::tasks::create(&cfg, "A", NewTask::default()).unwrap();
        let f = Filter::unbounded();
        let err = next(&cfg, &f, true, false, None).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            "no agent identity: set [core].agent or pass --owner"
        );
    }
}
