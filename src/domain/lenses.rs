//! `project`, `session-start` and the `status` payload.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use serde_json::{Map, Value as Json};

use crate::config::Config;
use crate::domain::select::{Filter, SortKey};
use crate::domain::tasks::Availability;
use crate::error::{MeshError, Result};
use crate::fm::View;
use crate::model::memory::{Memory, MEMORY_FIELDS};
use crate::model::note::{Note, NOTE_FIELDS};
use crate::model::task::{Task, TASK_FIELDS, TASK_STATUSES};
use crate::render;
use crate::spaces::Space;
use crate::timefmt::{parse_iso_lenient, parse_since, ts_instant};

/// The `--since` window `session-start` uses for mentions and activity.
pub const SESSION_SINCE: &str = "7d";

/// How many memories `session-start` offers.
pub const MEMORY_PICKS: usize = 5;

/// The statuses that make a task part of the live queue.
pub const OPEN_STATUSES: [&str; 2] = ["open", "claimed"];

/// How old a claim must be to count as stale in the `status` agent breakdown.
pub const STATUS_STALE_WINDOW: &str = "2d";

/// How many dangling links `status` lists before it just reports the count.
pub const DANGLING_CAP: usize = 50;

/// The corpus `session-start` and `project` read when no `--space` is given.
pub const DEFAULT_SPACES: [Space; 2] = [Space::Notes, Space::Tasks];

// --------------------------------------------------------------------------------------------
// project
// --------------------------------------------------------------------------------------------

/// `project` over the default corpus.
pub fn project_view(cfg: &Config, project_id: &str) -> Result<serde_json::Value> {
    project_view_in(cfg, project_id, &DEFAULT_SPACES)
}

/// A project note plus the tasks scoped to it: `{"project": <node>, "tasks": [<node>, …]}`.
///
/// The note is a point read (an `n-` id or a title slug). Tasks are filtered by **exact
/// string equality** against the raw `project` frontmatter value — the caller's string, not
/// the resolved id — with every status, unbounded, `updated` descending. A project with no
/// scoped tasks yields `"tasks": []`, never an error.
pub fn project_view_in(
    cfg: &Config,
    project_id: &str,
    spaces: &[Space],
) -> Result<serde_json::Value> {
    let note = if spaces.contains(&Space::Notes) {
        crate::domain::notes::get(cfg, project_id).ok()
    } else {
        None
    };
    let Some(view) = note else {
        return Err(MeshError::ProjectNotFound(project_id.to_string()));
    };
    let project = render::entry(
        &view.item.meta,
        NOTE_FIELDS.fields(),
        None,
        Some(&view.path),
    );
    let mut tasks: Vec<Json> = Vec::new();
    if spaces.contains(&Space::Tasks) {
        let filter = Filter {
            limit: None,
            sort: SortKey::Updated,
            ..Filter::default()
        }
        .with_extra("project", Some(project_id));
        for task in crate::domain::tasks::list(cfg, &filter, Availability::Any).unwrap_or_default()
        {
            tasks.push(render::entry(
                &task.item.meta,
                TASK_FIELDS.fields(),
                None,
                Some(&task.path),
            ));
        }
    }
    let mut out = Map::new();
    out.insert("project".to_string(), project);
    out.insert("tasks".to_string(), Json::Array(tasks));
    Ok(Json::Object(out))
}

// --------------------------------------------------------------------------------------------
// session-start
// --------------------------------------------------------------------------------------------

/// The one recency key: `updated` (ISO) if present, else `mtime`, else `0.0`.
fn updated_key(entry: &Json) -> f64 {
    if let Some(text) = entry.get("updated").and_then(Json::as_str) {
        if let Some(value) = parse_iso_lenient(text) {
            let at = ts_instant(&value);
            #[allow(clippy::cast_precision_loss)]
            let secs = at.timestamp() as f64;
            return secs + f64::from(at.timestamp_subsec_nanos()) / 1_000_000_000.0;
        }
    }
    entry.get("mtime").and_then(Json::as_f64).unwrap_or(0.0)
}

fn entry_id(entry: &Json) -> Option<String> {
    entry.get("id").and_then(Json::as_str).map(str::to_string)
}

/// Sort newest first, ties by id ascending — the fix for Python's hash-ordered tie.
fn sort_recent_then_id(entries: &mut [Json]) {
    entries.sort_by_key(entry_id);
    entries.sort_by(|a, b| {
        updated_key(b)
            .partial_cmp(&updated_key(a))
            .unwrap_or(Ordering::Equal)
    });
}

/// Mentions of me over the default corpus.
pub fn session_mentions(
    cfg: &Config,
    tasks: &[View<Task>],
    notes: &[View<Note>],
    me: Option<&str>,
    since: &str,
) -> Vec<serde_json::Value> {
    session_mentions_in(cfg, tasks, notes, me, since, &DEFAULT_SPACES)
}

/// The entities that name one of my tasks or notes in their `related` list.
///
/// Target set = every task I own or have claimed (any status) plus every note I own. One
/// inbound pass, shared by every target. Exclusions, in order: an unresolvable mentioner, a
/// mentioner I authored myself, and anything older than the window. Entries are frontmatter
/// nodes — they never carry a body.
pub fn session_mentions_in(
    cfg: &Config,
    tasks: &[View<Task>],
    notes: &[View<Note>],
    me: Option<&str>,
    since: &str,
    spaces: &[Space],
) -> Vec<serde_json::Value> {
    let mut targets: Vec<String> = tasks.iter().map(|v| v.item.id.clone()).collect();
    targets.extend(notes.iter().map(|v| v.item.id.clone()));
    if targets.is_empty() {
        return Vec::new();
    }
    let Ok(cutoff) = parse_since(since) else {
        return Vec::new();
    };
    #[allow(clippy::cast_precision_loss)]
    let floor = cutoff.timestamp() as f64 + f64::from(cutoff.timestamp_subsec_nanos()) / 1e9;

    let inbound = crate::domain::context::inbound_index_in(cfg, spaces);
    let mut mentioners: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for target in &targets {
        for source in inbound.get(target).into_iter().flatten() {
            if seen.insert(source.clone()) {
                mentioners.push(source.clone());
            }
        }
    }

    let mut entries: Vec<Json> = Vec::new();
    for mentioner in mentioners {
        let Some(entry) = crate::domain::context::resolve_entry(cfg, &mentioner, spaces) else {
            continue;
        };
        if me.is_some() && entry.get("owner").and_then(Json::as_str) == me {
            continue;
        }
        if updated_key(&entry) < floor {
            continue;
        }
        entries.push(entry);
    }
    sort_recent_then_id(&mut entries);
    entries
}

/// The serialised size of one entry, in characters — what `--budget` counts.
fn entry_cost(entry: &Json) -> usize {
    serde_json::to_string(entry).map(|s| s.len()).unwrap_or(0)
}

fn total_cost(entries: &[Json]) -> usize {
    entries.iter().map(entry_cost).sum()
}

/// Trim to `budget`: bodies first (last entry first), then whole entries off the end.
/// Returns how many entries were dropped.
fn apply_budget(entries: &mut Vec<Json>, budget: usize) -> usize {
    if budget == 0 {
        return 0;
    }
    let mut total = total_cost(entries);
    let mut index = entries.len();
    while total > budget && index > 0 {
        index -= 1;
        let trimmed = entries
            .get_mut(index)
            .and_then(Json::as_object_mut)
            .is_some_and(|obj| obj.remove("body").is_some());
        if trimmed {
            total = total_cost(entries);
        }
    }
    let mut dropped = 0;
    while total > budget && !entries.is_empty() {
        entries.pop();
        dropped += 1;
        total = total_cost(entries);
    }
    dropped
}

/// The synthetic entry a truncated payload ends with.
fn truncated_entry(dropped: usize) -> Json {
    let mut out = Map::new();
    out.insert("id".to_string(), Json::Null);
    out.insert("type".to_string(), Json::String("meta".to_string()));
    out.insert("reason".to_string(), Json::String("truncated".to_string()));
    out.insert("dropped".to_string(), Json::from(dropped));
    Json::Object(out)
}

fn with_reason(entry: &Json, reason: &str) -> Json {
    let mut out = entry.clone();
    if let Some(obj) = out.as_object_mut() {
        obj.insert("reason".to_string(), Json::String(reason.to_string()));
    }
    out
}

/// Merge the four sections into the warm-start payload.
///
/// Sections in order — tasks → mentions → memories → activity — deduped by id with the
/// earlier section winning. `reason` lands last on every entry (after `path` on a task, which
/// then carries its `body`). Only live (`open`/`claimed`) tasks contribute, only they carry a
/// body, and only without `--meta-only`. The activity remainder is re-sorted newest first.
pub fn session_start_entries(
    cfg: &Config,
    tasks: &[View<Task>],
    mentions: Vec<serde_json::Value>,
    memories: Vec<View<Memory>>,
    activity: Vec<serde_json::Value>,
    meta_only: bool,
    budget: usize,
) -> Vec<serde_json::Value> {
    let _ = cfg;
    let mut out: Vec<Json> = Vec::new();
    // The dedup set is seeded from the LIVE tasks only, so a task of mine that is already
    // done can legitimately reappear in the activity remainder.
    let mut seen: HashSet<String> = HashSet::new();

    for view in tasks {
        if !OPEN_STATUSES.contains(&view.item.status.as_str()) {
            continue;
        }
        seen.insert(view.item.id.clone());
        let mut entry = render::entry(
            &view.item.meta,
            TASK_FIELDS.fields(),
            None,
            Some(&view.path),
        );
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("reason".to_string(), Json::String("task".to_string()));
            if !meta_only {
                obj.insert(
                    "body".to_string(),
                    Json::String(crate::fm::read_body(&view.path)),
                );
            }
        }
        out.push(entry);
    }

    for entry in &mentions {
        if entry_id(entry).is_some_and(|id| !seen.insert(id)) {
            continue;
        }
        out.push(with_reason(entry, "mention"));
    }

    for view in &memories {
        if !seen.insert(view.item.id.clone()) {
            continue;
        }
        let entry = render::entry(
            &view.item.meta,
            MEMORY_FIELDS.fields(),
            None,
            Some(&view.path),
        );
        out.push(with_reason(&entry, "memory"));
    }

    let mut remaining: Vec<Json> = Vec::new();
    for entry in &activity {
        if entry_id(entry).is_some_and(|id| !seen.insert(id)) {
            continue;
        }
        remaining.push(with_reason(entry, "activity"));
    }
    sort_recent_then_id(&mut remaining);
    out.extend(remaining);

    let dropped = apply_budget(&mut out, budget);
    if dropped > 0 {
        out.push(truncated_entry(dropped));
    }
    out
}

// --------------------------------------------------------------------------------------------
// status
// --------------------------------------------------------------------------------------------

/// Every `<space>/.locks/*.lock` the canonical staleness rule reports as stale.
///
/// Spaces in declaration order, locks sorted within each. Strictly read-only: nothing is
/// touched, cleared or reclaimed.
pub fn scan_stale_locks(cfg: &Config) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for space in Space::ALL {
        let Ok(root) = cfg.root(space) else {
            continue;
        };
        let locks = root.join(".locks");
        let Ok(entries) = std::fs::read_dir(&locks) else {
            continue;
        };
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("lock"))
            .collect();
        candidates.sort();
        for path in candidates {
            if crate::storage::is_stale(&path) {
                out.push(path);
            }
        }
    }
    out
}

fn now_epoch() -> f64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn counts_object(entries: &[(&str, u64)]) -> Json {
    let mut out = Map::new();
    for (key, value) in entries {
        out.insert((*key).to_string(), Json::from(*value));
    }
    Json::Object(out)
}

fn liveness(pid: Option<u32>) -> Json {
    let mut out = Map::new();
    out.insert("running".to_string(), Json::Bool(pid.is_some()));
    out.insert(
        "pid".to_string(),
        pid.map_or(Json::Null, |p| Json::from(u64::from(p))),
    );
    Json::Object(out)
}

/// One agent's row in the `status` breakdown.
#[derive(Clone, Debug, Default)]
struct AgentCounts {
    owns_open: u64,
    claimed: u64,
    stale_claims: u64,
}

/// The complete `mesh status` payload, in the pinned key order (surface.md §8.2).
///
/// Strictly read-only: no mtime, no byte and no lock is touched. Every group degrades to a
/// zero/empty shape rather than failing, so a missing vault is still exit 0.
pub fn status_report(cfg: &Config) -> serde_json::Value {
    let mut out = Map::new();

    // notes: schema-valid notes only.
    let notes = crate::domain::notes::list(cfg, &Filter::unbounded(), false)
        .map(|views| views.len())
        .unwrap_or(0);
    out.insert("notes".to_string(), Json::from(notes));

    // tasks: zero-filled in TASK_STATUSES order, an unknown status appended.
    let task_views = crate::domain::tasks::list(cfg, &Filter::unbounded(), Availability::Any)
        .unwrap_or_default();
    let mut status_counts: Vec<(String, u64)> = TASK_STATUSES
        .iter()
        .map(|s| ((*s).to_string(), 0))
        .collect();
    for view in &task_views {
        let status = view.item.status.clone();
        match status_counts.iter_mut().find(|(name, _)| *name == status) {
            Some((_, count)) => *count += 1,
            None => status_counts.push((status, 1)),
        }
    }
    let mut tasks = Map::new();
    for (name, count) in &status_counts {
        tasks.insert(name.clone(), Json::from(*count));
    }
    out.insert("tasks".to_string(), Json::Object(tasks));
    out.insert("tasks_total".to_string(), Json::from(task_views.len()));

    // freshness: the newest notes-or-tasks file, age clamped at zero.
    let newest =
        crate::domain::activity::scan_recent(cfg, 1, &crate::domain::activity::DEFAULT_SPACES);
    let mut freshness = Map::new();
    match newest.first().map(crate::domain::activity::row_mtime) {
        Some(mtime) => {
            freshness.insert(
                "mtime".to_string(),
                serde_json::Number::from_f64(mtime).map_or(Json::Null, Json::Number),
            );
            let age = (now_epoch() - mtime).max(0.0);
            freshness.insert(
                "age_seconds".to_string(),
                serde_json::Number::from_f64(age).map_or(Json::Null, Json::Number),
            );
        }
        None => {
            freshness.insert("mtime".to_string(), Json::Null);
            freshness.insert("age_seconds".to_string(), Json::Null);
        }
    }
    out.insert("freshness".to_string(), Json::Object(freshness));

    // dangling links: first-seen order, capped, with the real total reported separately.
    let dangling = crate::domain::wikilinks::find_dangling(cfg);
    let listed: Vec<Json> = dangling
        .iter()
        .take(DANGLING_CAP)
        .map(|t| Json::String(t.clone()))
        .collect();
    out.insert("dangling_links".to_string(), Json::Array(listed));

    let locks: Vec<Json> = scan_stale_locks(cfg)
        .iter()
        .map(|p| Json::String(p.display().to_string()))
        .collect();
    out.insert("stale_locks".to_string(), Json::Array(locks));

    let vault = cfg.vault().to_path_buf();
    let mut vault_obj = Map::new();
    vault_obj.insert(
        "path".to_string(),
        Json::String(vault.display().to_string()),
    );
    vault_obj.insert("exists".to_string(), Json::Bool(vault.is_dir()));
    out.insert("vault".to_string(), Json::Object(vault_obj));

    // `daemon` mirrors the watcher, so an existing parser still sees something true.
    let watcher = crate::cli::watch::watcher_pid(cfg);
    out.insert("daemon".to_string(), liveness(watcher));

    // agents: registered on sight as owner or claimer, sorted by identity.
    let stale_cutoff = parse_since(STATUS_STALE_WINDOW).ok();
    let mut agents: Vec<(String, AgentCounts)> = Vec::new();
    let register = |agents: &mut Vec<(String, AgentCounts)>, name: &str| -> usize {
        match agents.iter().position(|(id, _)| id == name) {
            Some(index) => index,
            None => {
                agents.push((name.to_string(), AgentCounts::default()));
                agents.len().saturating_sub(1)
            }
        }
    };
    for view in &task_views {
        let task = &view.item;
        if let Some(owner) = task.owner.as_deref().filter(|o| !o.is_empty()) {
            let index = register(&mut agents, owner);
            if task.status == "open" {
                if let Some((_, counts)) = agents.get_mut(index) {
                    counts.owns_open += 1;
                }
            }
        }
        if let Some(claimer) = task.claimed_by.as_deref().filter(|c| !c.is_empty()) {
            let index = register(&mut agents, claimer);
            if task.status == "claimed" {
                if let Some((_, counts)) = agents.get_mut(index) {
                    counts.claimed += 1;
                    let stale = matches!((task.updated, stale_cutoff), (Some(u), Some(c)) if u < c);
                    if stale {
                        counts.stale_claims += 1;
                    }
                }
            }
        }
    }
    agents.sort_by(|a, b| a.0.cmp(&b.0));
    let mut agents_obj = Map::new();
    for (name, counts) in &agents {
        agents_obj.insert(
            name.clone(),
            counts_object(&[
                ("owns_open", counts.owns_open),
                ("claimed", counts.claimed),
                ("stale_claims", counts.stale_claims),
            ]),
        );
    }
    out.insert("agents".to_string(), Json::Object(agents_obj));

    out.insert(
        "dangling_links_total".to_string(),
        Json::from(dangling.len()),
    );

    let memories = crate::domain::memories::summary(cfg);
    out.insert(
        "memories".to_string(),
        counts_object(&[
            ("total", memories.total),
            ("expired", memories.expired),
            ("superseded", memories.superseded),
        ]),
    );

    let scratch = crate::domain::scratch::summary(cfg);
    out.insert(
        "scratch".to_string(),
        counts_object(&[("files", scratch.files), ("agents", scratch.agents)]),
    );

    let assets = crate::domain::assets::summary(cfg);
    out.insert(
        "assets".to_string(),
        counts_object(&[
            ("count", assets.count),
            ("bytes", assets.bytes),
            ("orphan_blobs", assets.orphan_blobs),
        ]),
    );

    // deps: readiness is derived, so every number here is computed from one scan.
    let task_rows = crate::domain::tasks::rows(cfg);
    let blocked = crate::domain::tasks::list(cfg, &Filter::unbounded(), Availability::Blocked)
        .map(|v| v.len())
        .unwrap_or(0);
    let ready = crate::domain::tasks::list(cfg, &Filter::unbounded(), Availability::Ready)
        .map(|v| v.len())
        .unwrap_or(0);
    let cycles: Vec<Json> = crate::domain::deps::cycles(&task_rows)
        .into_iter()
        .map(|cycle| Json::Array(cycle.into_iter().map(Json::String).collect()))
        .collect();
    let mut deps = Map::new();
    deps.insert("blocked".to_string(), Json::from(blocked));
    deps.insert("ready".to_string(), Json::from(ready));
    deps.insert("cycles".to_string(), Json::Array(cycles));
    deps.insert(
        "dangling_blockers".to_string(),
        Json::from(crate::domain::deps::dangling_blockers(&task_rows).len()),
    );
    out.insert("deps".to_string(), Json::Object(deps));

    let mut spaces = Map::new();
    for space in Space::ALL {
        let Ok(root) = cfg.root(space) else {
            continue;
        };
        let mut entry = Map::new();
        entry.insert("path".to_string(), Json::String(root.display().to_string()));
        entry.insert("exists".to_string(), Json::Bool(root.is_dir()));
        spaces.insert(space.name().to_string(), Json::Object(entry));
    }
    out.insert("spaces".to_string(), Json::Object(spaces));

    out.insert("watcher".to_string(), liveness(watcher));

    Json::Object(out)
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
    use crate::domain::select::FromMeta;
    use crate::fm::parse_meta;
    use std::fs;
    use std::path::Path;

    fn note(dir: &Path, id: &str, title: &str, owner: &str, body: &str) {
        let text = format!(
            "---\nid: {id}\ntype: note\ntitle: {title}\ntags: []\nowner: {owner}\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated: []\n---\n\n{body}\n"
        );
        let path = dir.join("notes").join(format!("{id}.md"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn task_file(dir: &Path, id: &str, status: &str, owner: &str, claimed: &str, project: &str) {
        let sub = if status == "done" || status == "cancelled" {
            "done"
        } else {
            "open"
        };
        let text = format!(
            "---\nid: {id}\ntype: task\ntitle: {id}\ntags: []\nowner: {owner}\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated: []\n\
             status: {status}\npriority: null\nclaimed_by: {claimed}\nproject: {project}\n\
             blocks: []\nblocked_by: []\n---\n\n{id} body\n"
        );
        let path = dir.join("tasks").join(sub).join(format!("{id}.md"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn task_views(cfg: &Config) -> Vec<View<Task>> {
        crate::domain::tasks::list(cfg, &Filter::unbounded(), Availability::Any).unwrap()
    }

    fn ids(entries: &[Json]) -> Vec<String> {
        entries
            .iter()
            .map(|e| {
                e.get("id")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    fn reasons(entries: &[Json]) -> Vec<String> {
        entries
            .iter()
            .map(|e| {
                e.get("reason")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn project_view_returns_the_note_and_its_scoped_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-p", "Proj", "test-agent", "the project");
        task_file(dir.path(), "t-a", "open", "test-agent", "null", "n-p");
        task_file(dir.path(), "t-b", "done", "test-agent", "null", "n-p");
        task_file(dir.path(), "t-c", "open", "test-agent", "null", "null");
        let payload = project_view(&cfg, "n-p").unwrap();
        let keys: Vec<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["project", "tasks"]);
        assert_eq!(payload["project"]["id"], Json::String("n-p".into()));
        let task_ids = ids(payload["tasks"].as_array().unwrap());
        assert_eq!(task_ids.len(), 2);
        assert!(task_ids.contains(&"t-a".to_string()));
        assert!(task_ids.contains(&"t-b".to_string()));
    }

    #[test]
    fn project_view_matches_the_raw_id_and_reports_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-p", "Proj", "test-agent", "x");
        task_file(dir.path(), "t-a", "open", "test-agent", "null", "n-p");
        // A slug seed resolves the note but matches no task: the raw string is the filter.
        let payload = project_view(&cfg, "proj").unwrap();
        assert_eq!(payload["project"]["id"], Json::String("n-p".into()));
        assert!(payload["tasks"].as_array().unwrap().is_empty());

        let err = project_view(&cfg, "n-nope").unwrap_err();
        assert_eq!(err.code(), 3);
        assert_eq!(err.to_string(), "project not found: n-nope");
    }

    #[test]
    fn session_start_orders_sections_and_dedupes_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        task_file(dir.path(), "t-live", "open", "test-agent", "null", "null");
        task_file(dir.path(), "t-done", "done", "test-agent", "null", "null");
        let tasks = task_views(&cfg);
        let mention = serde_json::json!({"id": "n-m", "type": "note", "title": "M",
                                         "updated": "2026-01-02T00:00:00Z", "path": "/x/n-m.md"});
        let activity = vec![
            serde_json::json!({"id": "t-live", "type": "task", "title": "L", "path": "/x",
                               "mtime": 5.0, "owner": null, "claimed_by": null}),
            serde_json::json!({"id": "t-done", "type": "task", "title": "D", "path": "/x",
                               "mtime": 9.0, "owner": null, "claimed_by": null}),
            serde_json::json!({"id": "n-old", "type": "note", "title": "O", "path": "/x",
                               "mtime": 1.0, "owner": null, "claimed_by": null}),
        ];
        let entries =
            session_start_entries(&cfg, &tasks, vec![mention], Vec::new(), activity, true, 0);
        assert_eq!(ids(&entries), ["t-live", "n-m", "t-done", "n-old"]);
        assert_eq!(
            reasons(&entries),
            ["task", "mention", "activity", "activity"]
        );
    }

    #[test]
    fn only_live_tasks_carry_a_body_and_meta_only_omits_it() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        task_file(
            dir.path(),
            "t-live",
            "claimed",
            "test-agent",
            "test-agent",
            "null",
        );
        let tasks = task_views(&cfg);
        let mention = serde_json::json!({"id": "n-m", "type": "note", "title": "M",
                                         "updated": "2026-01-02T00:00:00Z"});
        let entries = session_start_entries(
            &cfg,
            &tasks,
            vec![mention.clone()],
            Vec::new(),
            Vec::new(),
            false,
            0,
        );
        assert_eq!(entries[0]["body"], Json::String("t-live body".into()));
        // `reason` sits before `body` on a task entry, after `path`.
        let keys: Vec<&str> = entries[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(&keys[keys.len() - 3..], ["path", "reason", "body"]);
        assert!(entries[1].get("body").is_none());

        let meta_only =
            session_start_entries(&cfg, &tasks, vec![mention], Vec::new(), Vec::new(), true, 0);
        assert!(meta_only[0].get("body").is_none());
    }

    #[test]
    fn memories_sit_between_mentions_and_activity_and_never_carry_a_body() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let meta = parse_meta(
            "id: m-a\ntype: memory\ntitle: Fact\ntags: []\nowner: null\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated: []\n\
             kind: fact\nscope: shared\nimportance: 4\nsource: null\nexpires: null\n\
             superseded_by: null\n",
        )
        .unwrap();
        let memory = View {
            item: Memory::from_meta(&meta).unwrap(),
            body: String::new(),
            path: dir.path().join("memories/m-a.md"),
        };
        let mention = serde_json::json!({"id": "n-m", "type": "note", "title": "M",
                                         "updated": "2026-01-02T00:00:00Z"});
        let activity = vec![
            serde_json::json!({"id": "n-x", "type": "note", "title": "X",
                                               "mtime": 3.0}),
        ];
        let entries =
            session_start_entries(&cfg, &[], vec![mention], vec![memory], activity, false, 0);
        assert_eq!(ids(&entries), ["n-m", "m-a", "n-x"]);
        assert_eq!(reasons(&entries), ["mention", "memory", "activity"]);
        assert!(entries[1].get("body").is_none());
        assert_eq!(entries[1]["importance"], Json::from(4));
    }

    #[test]
    fn the_budget_trims_bodies_then_entries_and_reports_the_drop() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        task_file(dir.path(), "t-a", "open", "test-agent", "null", "null");
        task_file(dir.path(), "t-b", "open", "test-agent", "null", "null");
        let tasks = task_views(&cfg);
        let full =
            session_start_entries(&cfg, &tasks, Vec::new(), Vec::new(), Vec::new(), false, 0);
        assert_eq!(full.len(), 2);
        assert!(full.iter().all(|e| e.get("body").is_some()));

        // A budget just under the full size trims bodies but keeps both entries.
        let bodies: usize = full.iter().map(entry_cost).sum();
        let trimmed = session_start_entries(
            &cfg,
            &tasks,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            false,
            bodies - 5,
        );
        assert_eq!(trimmed.len(), 2);
        assert!(trimmed.iter().any(|e| e.get("body").is_none()));

        // A tiny budget drops entries and appends the synthetic marker.
        let tiny =
            session_start_entries(&cfg, &tasks, Vec::new(), Vec::new(), Vec::new(), false, 10);
        let last = tiny.last().unwrap();
        assert_eq!(last["type"], Json::String("meta".into()));
        assert_eq!(last["reason"], Json::String("truncated".into()));
        assert!(last["id"].is_null());
        assert!(last["dropped"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn mentions_exclude_self_authored_and_stale_sources() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-mine", "Mine", "test-agent", "target");
        // A peer's note that names mine.
        fs::write(
            dir.path().join("notes/n-peer.md"),
            "---\nid: n-peer\ntype: note\ntitle: Peer\ntags: []\nowner: peer\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2100-01-01T00:00:00Z\nrelated:\n  - n-mine\n---\n\nx\n",
        )
        .unwrap();
        // My own note that names mine: excluded as self-authored.
        fs::write(
            dir.path().join("notes/n-self.md"),
            "---\nid: n-self\ntype: note\ntitle: Self\ntags: []\nowner: test-agent\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2100-01-01T00:00:00Z\nrelated:\n  - n-mine\n---\n\nx\n",
        )
        .unwrap();
        // An old peer note: outside the window.
        fs::write(
            dir.path().join("notes/n-old.md"),
            "---\nid: n-old\ntype: note\ntitle: Old\ntags: []\nowner: peer\n\
             created: 2000-01-01T00:00:00Z\nupdated: 2000-01-01T00:00:00Z\nrelated:\n  - n-mine\n---\n\nx\n",
        )
        .unwrap();
        let notes = crate::domain::notes::list(
            &cfg,
            &Filter {
                owner: Some("test-agent".into()),
                limit: None,
                ..Filter::default()
            },
            false,
        )
        .unwrap();
        let mentions = session_mentions(&cfg, &[], &notes, Some("test-agent"), SESSION_SINCE);
        assert_eq!(ids(&mentions), ["n-peer"]);
        assert!(mentions[0].get("body").is_none());
    }

    #[test]
    fn no_targets_means_no_mentions() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", "someone", "x");
        assert!(session_mentions(&cfg, &[], &[], Some("test-agent"), SESSION_SINCE).is_empty());
    }

    #[test]
    fn stale_locks_are_scanned_per_space_and_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let locks = dir.path().join("tasks/.locks");
        fs::create_dir_all(&locks).unwrap();
        fs::write(locks.join("t-b.lock"), "999999").unwrap();
        fs::write(locks.join("t-a.lock"), "999999").unwrap();
        fs::write(locks.join("live.lock"), std::process::id().to_string()).unwrap();
        fs::write(locks.join("not-a-lock.txt"), "999999").unwrap();
        let found = scan_stale_locks(&cfg);
        let names: Vec<String> = found
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert_eq!(names, ["t-a.lock", "t-b.lock"]);
    }

    #[test]
    fn the_status_payload_has_the_pinned_key_order() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let report = status_report(&cfg);
        let keys: Vec<&str> = report
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "notes",
                "tasks",
                "tasks_total",
                "freshness",
                "dangling_links",
                "stale_locks",
                "vault",
                "daemon",
                "agents",
                "dangling_links_total",
                "memories",
                "scratch",
                "assets",
                "deps",
                "spaces",
                "watcher",
            ]
        );
        assert_eq!(report["notes"], Json::from(0));
        assert_eq!(report["tasks"]["open"], Json::from(0));
        assert_eq!(report["tasks_total"], Json::from(0));
        assert!(report["freshness"]["mtime"].is_null());
        assert!(report["freshness"]["age_seconds"].is_null());
        assert_eq!(report["agents"], serde_json::json!({}));
        assert_eq!(report["daemon"]["running"], Json::Bool(false));
        assert_eq!(report["watcher"]["pid"], Json::Null);
        let space_keys: Vec<&str> = report["spaces"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            space_keys,
            ["notes", "tasks", "memories", "scratch", "assets"]
        );
    }

    #[test]
    fn the_status_counts_come_from_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", "A", "test-agent", "body [[Ghost]] link");
        note(dir.path(), "n-b", "B", "test-agent", "plain");
        task_file(dir.path(), "t-open", "open", "alice", "null", "null");
        task_file(dir.path(), "t-claimed", "claimed", "alice", "bob", "null");
        task_file(dir.path(), "t-done", "done", "alice", "null", "null");
        let report = status_report(&cfg);
        assert_eq!(report["notes"], Json::from(2));
        assert_eq!(report["tasks"]["open"], Json::from(1));
        assert_eq!(report["tasks"]["claimed"], Json::from(1));
        assert_eq!(report["tasks"]["done"], Json::from(1));
        assert_eq!(report["tasks_total"], Json::from(3));
        assert_eq!(report["dangling_links"], serde_json::json!(["Ghost"]));
        assert_eq!(report["dangling_links_total"], Json::from(1));
        assert!(report["freshness"]["age_seconds"].as_f64().unwrap() >= 0.0);
        assert_eq!(report["agents"]["alice"]["owns_open"], Json::from(1));
        assert_eq!(report["agents"]["bob"]["claimed"], Json::from(1));
        assert_eq!(report["agents"]["bob"]["owns_open"], Json::from(0));
        assert_eq!(report["deps"]["ready"], Json::from(1));
        assert_eq!(report["deps"]["blocked"], Json::from(0));
        assert_eq!(report["deps"]["cycles"], serde_json::json!([]));
        assert_eq!(report["vault"]["exists"], Json::Bool(true));
    }

    #[test]
    fn a_missing_vault_still_reports() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(&dir.path().join("gone"));
        let report = status_report(&cfg);
        assert_eq!(report["vault"]["exists"], Json::Bool(false));
        assert_eq!(report["notes"], Json::from(0));
        assert_eq!(report["stale_locks"], serde_json::json!([]));
    }

    #[test]
    fn blocked_and_dangling_blockers_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        task_file(dir.path(), "t-a", "open", "test-agent", "null", "null");
        let path = dir.path().join("tasks/open/t-b.md");
        fs::write(
            &path,
            "---\nid: t-b\ntype: task\ntitle: B\ntags: []\nowner: test-agent\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated: []\n\
             status: open\npriority: null\nclaimed_by: null\nproject: null\nblocks: []\n\
             blocked_by:\n  - t-a\n  - t-ghost\n---\n\nb\n",
        )
        .unwrap();
        let report = status_report(&cfg);
        assert_eq!(report["deps"]["blocked"], Json::from(1));
        assert_eq!(report["deps"]["ready"], Json::from(1));
        assert_eq!(report["deps"]["dangling_blockers"], Json::from(1));
    }
}
