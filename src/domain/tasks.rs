//! Task lifecycle verbs.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::domain::{deps, validate_owner, AppendOpts, Filter};
use crate::error::{MeshError, Result};
use crate::fm::{read_doc, read_meta_only, write_doc, Doc, Meta, Row, Value, View};
use crate::ids::generate_id;
use crate::model::common::{meta_str, meta_strings, optional_str, ts_value};
use crate::model::task::{is_terminal, Task, TASK_PRIORITIES, TASK_STATUSES};
use crate::spaces::Space;
use crate::storage::{create_lock, entity_lock, hold, iter_md, safe_resolve};
use crate::text::{
    append_to_end, append_under_section, edit_distance, format_block, format_stamp, slugify,
};
use crate::timefmt::{iso_seconds_z, iso_z, now_utc};

/// The two folders a task can live in, walked in this order and never recursively.
pub const TASK_SUBDIRS: [&str; 2] = ["open", "done"];

/// The heading `finish` writes.
pub const OUTCOME_HEADING: &str = "## Outcome";
/// The heading `cancel` writes.
pub const CANCELLED_HEADING: &str = "## Cancelled";

/// Which slice of the task list a caller wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Availability {
    #[default]
    Any,
    /// `status == open && claimed_by == null` — the Python meaning, dependency-blind.
    Available,
    /// Available and unblocked.
    Ready,
    /// Open or claimed with at least one unsatisfied blocker.
    Blocked,
}

/// The two terminal transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminal {
    Finish,
    Cancel,
}

impl Terminal {
    /// The section heading this transition appends under.
    pub fn heading(self) -> &'static str {
        match self {
            Terminal::Finish => OUTCOME_HEADING,
            Terminal::Cancel => CANCELLED_HEADING,
        }
    }

    /// The status this transition writes.
    pub fn status(self) -> &'static str {
        match self {
            Terminal::Finish => "done",
            Terminal::Cancel => "cancelled",
        }
    }

    /// The mutation verb the CLI reports.
    pub fn verb(self) -> &'static str {
        match self {
            Terminal::Finish => "finished",
            Terminal::Cancel => "cancelled",
        }
    }
}

/// What `task new` was asked to create.
#[derive(Clone, Debug, Default)]
pub struct NewTask {
    pub priority: Option<String>,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub body: String,
    pub project: Option<String>,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
}

/// What `task update` was asked to change.
#[derive(Clone, Debug, Default)]
pub struct UpdateTask {
    pub priority: Option<String>,
    pub tags: Option<String>,
    pub title: Option<String>,
    pub project: Option<String>,
    pub owner: Option<String>,
    pub blocks: Option<Vec<String>>,
    pub blocked_by: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------------------
// paths, scanning and validation
// ---------------------------------------------------------------------------------------

/// The tasks space root, or the disabled-space validation error.
pub(crate) fn root(cfg: &Config) -> Result<&Path> {
    cfg.root(Space::Tasks)
}

/// `tasks/open` or `tasks/done`.
pub(crate) fn folder(cfg: &Config, sub: &str) -> Result<PathBuf> {
    Ok(root(cfg)?.join(sub))
}

/// Every task file, `open/` then `done/`, non-recursively — the canonical scope.
pub(crate) fn task_files(cfg: &Config) -> Vec<PathBuf> {
    let Ok(base) = root(cfg) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for sub in TASK_SUBDIRS {
        out.extend(iter_md(&base.join(sub), false, &[]));
    }
    out
}

/// Id-only resolution: the file in `open/` or `done/` whose stem is exactly `id`.
///
/// A title slug never resolves a task. A corrupt sibling never blocks a different id, because
/// a non-matching file is never read.
pub(crate) fn resolve(cfg: &Config, id: &str) -> Result<PathBuf> {
    root(cfg)?;
    for path in task_files(cfg) {
        if path.file_stem().and_then(|s| s.to_str()) == Some(id) {
            return safe_resolve(&cfg.spaces, &path);
        }
    }
    Err(not_found(cfg, id))
}

/// `TaskNotFound` with up to five near-miss ids attached.
pub(crate) fn not_found(cfg: &Config, id: &str) -> MeshError {
    MeshError::TaskNotFound(id.to_string()).with_candidates(near_ids(cfg, id))
}

/// The five ids closest to `target` by edit distance over their slugs.
fn near_ids(cfg: &Config, target: &str) -> Vec<String> {
    let want = slugify(target);
    let mut scored: Vec<(usize, String)> = task_files(cfg)
        .into_iter()
        .filter_map(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| (edit_distance(&want, &slugify(s)), s.to_string()))
        })
        .collect();
    scored.sort_by(|a, b| a.1.cmp(&b.1));
    scored.sort_by_key(|(d, _)| *d);
    scored.into_iter().take(5).map(|(_, id)| id).collect()
}

/// Whether any task file already carries this id.
pub(crate) fn id_taken(cfg: &Config, candidate: &str) -> bool {
    task_files(cfg)
        .iter()
        .any(|p| p.file_stem().and_then(|s| s.to_str()) == Some(candidate))
}

/// A validated view, or not-found — the corrupt-entity rule, in one place.
pub(crate) fn validated(meta: &Meta, id: &str) -> Result<Task> {
    use crate::domain::FromMeta;
    Task::from_meta(meta).ok_or_else(|| MeshError::TaskNotFound(id.to_string()))
}

/// `invalid priority: 'x' (use high, normal, low)` — checked before any lock or write.
pub fn validate_priority(priority: Option<&str>) -> Result<()> {
    match priority {
        None => Ok(()),
        Some(p) if TASK_PRIORITIES.contains(&p) => Ok(()),
        Some(p) => Err(MeshError::Validation(format!(
            "invalid priority: '{p}' (use {})",
            TASK_PRIORITIES.join(", ")
        ))),
    }
}

/// `unknown status: x (use open, claimed, done, cancelled)` — trims, drops empties,
/// order-preserving dedupe, and names every offender.
pub fn parse_status_csv(value: &str) -> Result<Option<Vec<String>>> {
    let statuses = crate::domain::select::parse_csv(value);
    if statuses.is_empty() {
        return Ok(None);
    }
    let unknown: Vec<&str> = statuses
        .iter()
        .filter(|s| !TASK_STATUSES.contains(&s.as_str()))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return Err(MeshError::Validation(format!(
            "unknown status: {} (use {})",
            unknown.join(", "),
            TASK_STATUSES.join(", ")
        )));
    }
    Ok(Some(statuses))
}

/// Write a document to `path` under the sandbox.
pub(crate) fn persist(cfg: &Config, path: &Path, doc: &Doc) -> Result<()> {
    write_doc(&cfg.spaces, path, doc)
}

/// Move a terminal file into `done/` when it is not already there, creating the folder.
///
/// `finish`/`cancel` write in place and then move, so a crash between the two strands a
/// terminal file in `open/`; the idempotent branch calls this and heals it.
fn move_if_needed(src: &Path, dest: &Path) -> Result<()> {
    if src == dest {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(src, dest)?;
    Ok(())
}

// ---------------------------------------------------------------------------------------
// verbs
// ---------------------------------------------------------------------------------------

pub fn create(cfg: &Config, title: &str, o: NewTask) -> Result<Task> {
    // Both validations run before any lock or write.
    validate_owner(cfg, o.owner.as_deref())?;
    validate_priority(o.priority.as_deref())?;

    let existing = rows(cfg);
    let now = now_utc();
    let related = crate::domain::resolve_wikilinks(cfg, &o.body);
    let open_dir = folder(cfg, "open")?;

    let task = {
        // The allocator lock spans id allocation AND the write.
        let _guard = hold(&create_lock(root(cfg)?))?;
        let id = generate_id("t-", &iso_z(&now), title, &|c| id_taken(cfg, c));

        // A new node plus its proposed edges must not close a cycle.
        deps::check_acyclic(
            &with_lists(&existing, &id, &o.blocks, &o.blocked_by),
            &proposed_edges(&id, &o.blocks, &o.blocked_by),
        )?;

        let mut meta = Meta::new();
        meta.insert("id".into(), Value::str(&id));
        meta.insert("type".into(), Value::str("task"));
        meta.insert("title".into(), Value::str(title));
        meta.insert("tags".into(), Value::strings(o.tags.clone()));
        meta.insert(
            "owner".into(),
            optional_str(crate::domain::effective_owner(cfg, o.owner.as_deref()).as_deref()),
        );
        meta.insert("created".into(), ts_value(&now));
        meta.insert("updated".into(), ts_value(&now));
        meta.insert("related".into(), Value::strings(related));
        meta.insert("status".into(), Value::str("open"));
        meta.insert("priority".into(), optional_str(o.priority.as_deref()));
        meta.insert("claimed_by".into(), Value::Null);
        meta.insert("project".into(), optional_str(o.project.as_deref()));
        meta.insert("blocks".into(), Value::strings(o.blocks.clone()));
        meta.insert("blocked_by".into(), Value::strings(o.blocked_by.clone()));

        let task = validated(&meta, &id)?;
        // A new task always lands in open/, regardless of its blockers.
        let path = safe_resolve(&cfg.spaces, &open_dir.join(format!("{id}.md")))?;
        persist(cfg, &path, &Doc::new(meta, o.body.clone()))?;
        task
    };

    // Best-effort mirrors: the authoritative side is already durable.
    deps::apply_mirrors(
        cfg,
        deps::mirror_edits(&task.id, &[], &o.blocks, &[], &o.blocked_by),
    );
    Ok(task)
}

pub fn update(cfg: &Config, id: &str, o: UpdateTask) -> Result<Task> {
    validate_owner(cfg, o.owner.as_deref())?;
    validate_priority(o.priority.as_deref())?;

    let before = rows(cfg);
    let (old_blocks, old_blocked_by) = node_lists(&before, id);
    let new_blocks = o.blocks.clone().unwrap_or_else(|| old_blocks.clone());
    let new_blocked_by = o
        .blocked_by
        .clone()
        .unwrap_or_else(|| old_blocked_by.clone());
    let edges_change = o.blocks.is_some() || o.blocked_by.is_some();
    if edges_change {
        deps::check_acyclic(
            &with_lists(&before, id, &new_blocks, &new_blocked_by),
            &proposed_edges(id, &new_blocks, &new_blocked_by),
        )?;
    }

    let task = {
        let _guard = hold(&entity_lock(root(cfg)?, id))?;
        // Resolution happens again inside the lock (the TOCTOU rule).
        let path = resolve(cfg, id)?;
        let mut doc = read_doc(&path).ok_or_else(|| MeshError::TaskNotFound(id.to_string()))?;
        // The map is mutated in place, so unknown keys and absent optionals round-trip.
        if let Some(p) = &o.priority {
            doc.meta.insert("priority".into(), Value::str(p));
        }
        if let Some(t) = &o.title {
            doc.meta.insert("title".into(), Value::str(t));
        }
        if let Some(p) = &o.project {
            doc.meta.insert("project".into(), Value::str(p));
        }
        if let Some(owner) = &o.owner {
            doc.meta.insert("owner".into(), Value::str(owner));
        }
        if let Some(spec) = &o.tags {
            let existing = meta_strings(&doc.meta, "tags");
            let applied = crate::domain::apply_tag_spec(&existing, spec)?;
            doc.meta.insert("tags".into(), Value::strings(applied));
        }
        if let Some(list) = &o.blocks {
            doc.meta
                .insert("blocks".into(), Value::strings(list.clone()));
        }
        if let Some(list) = &o.blocked_by {
            doc.meta
                .insert("blocked_by".into(), Value::strings(list.clone()));
        }
        let now = now_utc();
        doc.meta.insert("updated".into(), ts_value(&now));
        let task = validated(&doc.meta, id)?;
        persist(cfg, &path, &doc)?;
        task
    };

    if edges_change {
        // A replace also retracts the mirrors it dropped, or the union rule would resurrect them.
        deps::apply_mirrors(
            cfg,
            deps::mirror_edits(
                id,
                &old_blocks,
                &new_blocks,
                &old_blocked_by,
                &new_blocked_by,
            ),
        );
    }
    Ok(task)
}

pub fn append(cfg: &Config, id: &str, text: &str, o: AppendOpts) -> Result<Task> {
    let actor = o
        .actor
        .clone()
        .or_else(|| cfg.agent().map(str::to_string))
        .filter(|s| !s.is_empty());
    let block = format_block(text, o.timestamp, actor.as_deref());

    let _guard = hold(&entity_lock(root(cfg)?, id))?;
    let path = resolve(cfg, id)?;
    let mut doc = read_doc(&path).ok_or_else(|| MeshError::TaskNotFound(id.to_string()))?;
    doc.body = match &o.section {
        Some(section) => append_under_section(&doc.body, &block, section),
        None => append_to_end(&doc.body, &block),
    };
    // Never a lifecycle transition: status and folder are untouched.
    let related = crate::domain::resolve_wikilinks(cfg, &doc.body);
    doc.meta.insert("related".into(), Value::strings(related));
    let now = now_utc();
    doc.meta.insert("updated".into(), ts_value(&now));
    let task = validated(&doc.meta, id)?;
    persist(cfg, &path, &doc)?;
    Ok(task)
}

/// Returns the task and its unsatisfied blockers (empty when it was ready).
pub fn claim(cfg: &Config, id: &str, claimer: &str, strict: bool) -> Result<(Task, Vec<String>)> {
    // Resolve first, so a missing task is not-found rather than "unblocked".
    resolve(cfg, id)?;
    let unsatisfied = deps::readiness(&rows(cfg), id).unsatisfied;
    if strict && !unsatisfied.is_empty() {
        return Err(MeshError::Blocked {
            task_id: id.to_string(),
            blockers: unsatisfied.join(", "),
        });
    }

    let _guard = hold(&entity_lock(root(cfg)?, id))?;
    let path = resolve(cfg, id)?;
    let mut doc = read_doc(&path).ok_or_else(|| MeshError::TaskNotFound(id.to_string()))?;
    let status = meta_str(&doc.meta, "status").unwrap_or("open").to_string();
    // 1. Terminal is an idempotent no-op — checked BEFORE claimed_by, so claiming an
    //    already-finished task is a no-op rather than a conflict.
    if is_terminal(&status) {
        return Ok((validated(&doc.meta, id)?, unsatisfied));
    }
    let existing = meta_str(&doc.meta, "claimed_by").map(str::to_string);
    match existing {
        // 2. Same-owner reclaim: no write, `updated` untouched.
        Some(ref who) if who == claimer => Ok((validated(&doc.meta, id)?, unsatisfied)),
        // 3. Someone else holds it.
        Some(who) => Err(MeshError::ClaimConflict {
            task_id: id.to_string(),
            existing_owner: who,
        }),
        // 4. Take it. `open` and `claimed` share a folder, so a claim never moves the file.
        None => {
            doc.meta.insert("claimed_by".into(), Value::str(claimer));
            doc.meta.insert("status".into(), Value::str("claimed"));
            let now = now_utc();
            doc.meta.insert("updated".into(), ts_value(&now));
            let task = validated(&doc.meta, id)?;
            persist(cfg, &path, &doc)?;
            Ok((task, unsatisfied))
        }
    }
}

pub fn release(cfg: &Config, id: &str, releaser: &str, force: bool) -> Result<Task> {
    let _guard = hold(&entity_lock(root(cfg)?, id))?;
    let path = resolve(cfg, id)?;
    let mut doc = read_doc(&path).ok_or_else(|| MeshError::TaskNotFound(id.to_string()))?;
    let status = meta_str(&doc.meta, "status").unwrap_or("open").to_string();
    if is_terminal(&status) {
        return validated(&doc.meta, id);
    }
    let Some(holder) = meta_str(&doc.meta, "claimed_by").map(str::to_string) else {
        // Releasing an unclaimed task is an idempotent no-op.
        return validated(&doc.meta, id);
    };
    // `force` is a cooperation override and an audit affordance, never an auth check.
    if holder != releaser && !force {
        return Err(MeshError::ClaimConflict {
            task_id: id.to_string(),
            existing_owner: holder,
        });
    }
    doc.meta.insert("claimed_by".into(), Value::Null);
    doc.meta.insert("status".into(), Value::str("open"));
    let now = now_utc();
    doc.meta.insert("updated".into(), ts_value(&now));
    let task = validated(&doc.meta, id)?;
    persist(cfg, &path, &doc)?;
    Ok(task)
}

/// Returns the task and the ids that became ready — a report; nothing is written to them.
pub fn terminate(
    cfg: &Config,
    id: &str,
    kind: Terminal,
    text: Option<&str>,
    actor: Option<&str>,
) -> Result<(Task, Vec<String>)> {
    let done_path = safe_resolve(&cfg.spaces, &folder(cfg, "done")?.join(format!("{id}.md")))?;
    let task = {
        let _guard = hold(&entity_lock(root(cfg)?, id))?;
        let path = resolve(cfg, id)?;
        let mut doc = read_doc(&path).ok_or_else(|| MeshError::TaskNotFound(id.to_string()))?;
        let status = meta_str(&doc.meta, "status").unwrap_or("open").to_string();
        if is_terminal(&status) {
            // Idempotent: no second section, no `updated` bump — but a crash-stranded file
            // still gets reconciled into done/.
            let task = validated(&doc.meta, id)?;
            move_if_needed(&path, &done_path)?;
            task
        } else {
            let now = now_utc();
            let actor = actor
                .map(str::to_string)
                .or_else(|| cfg.agent().map(str::to_string))
                .filter(|s| !s.is_empty());
            let stamp = format_stamp(&iso_seconds_z(&now), actor.as_deref());
            let block = match text.filter(|t| !t.is_empty()) {
                Some(t) => format!("{stamp}\n{t}"),
                None => stamp,
            };
            let section = format!("{}\n\n{block}", kind.heading());
            doc.body = append_to_end(&doc.body, &section);
            let related = crate::domain::resolve_wikilinks(cfg, &doc.body);
            doc.meta.insert("related".into(), Value::strings(related));
            doc.meta.insert("status".into(), Value::str(kind.status()));
            doc.meta.insert("updated".into(), ts_value(&now));
            let task = validated(&doc.meta, id)?;
            // Write-then-move: a crash between the two is repaired by the branch above.
            persist(cfg, &path, &doc)?;
            move_if_needed(&path, &done_path)?;
            task
        }
    };
    // The cascade is a report, computed from a fresh scan; no other file is ever written.
    Ok((task, deps::newly_ready(&rows(cfg), id)))
}

pub fn get(cfg: &Config, id: &str) -> Result<View<Task>> {
    let path = resolve(cfg, id)?;
    let doc = read_doc(&path).ok_or_else(|| MeshError::TaskNotFound(id.to_string()))?;
    let item = validated(&doc.meta, id)?;
    Ok(View {
        item,
        body: doc.body,
        path,
    })
}

pub fn list(cfg: &Config, f: &Filter, av: Availability) -> Result<Vec<View<Task>>> {
    let all = rows(cfg);
    // Availability is applied before `select` so it is not defeated by the limit.
    let admitted: Vec<Row> = match av {
        Availability::Any => all,
        Availability::Available => all.into_iter().filter(|r| is_available(&r.meta)).collect(),
        Availability::Ready => {
            let snapshot = all.clone();
            all.into_iter()
                .filter(|r| {
                    is_available(&r.meta)
                        && meta_str(&r.meta, "id")
                            .is_some_and(|id| deps::readiness(&snapshot, id).ready)
                })
                .collect()
        }
        Availability::Blocked => {
            let snapshot = all.clone();
            all.into_iter()
                .filter(|r| {
                    !is_terminal(meta_str(&r.meta, "status").unwrap_or("open"))
                        && meta_str(&r.meta, "id").is_some_and(|id| {
                            !deps::readiness(&snapshot, id).unsatisfied.is_empty()
                        })
                })
                .collect()
        }
    };
    Ok(crate::domain::select(admitted, f))
}

/// `status == open && claimed_by == null` — excludes a hand-edited open task that still
/// carries a stale `claimed_by`, and never filters on owner.
fn is_available(meta: &Meta) -> bool {
    meta_str(meta, "status").unwrap_or("open") == "open" && meta_str(meta, "claimed_by").is_none()
}

pub fn delete(cfg: &Config, id: &str) -> Result<String> {
    let _guard = hold(&entity_lock(root(cfg)?, id))?;
    // No read: a corrupt task is still deletable — that is the repair path.
    let path = resolve(cfg, id)?;
    std::fs::remove_file(&path)?;
    Ok(id.to_string())
}

pub fn rows(cfg: &Config) -> Vec<Row> {
    task_files(cfg)
        .into_iter()
        .filter_map(|path| read_meta_only(&path).map(|meta| Row { path, meta }))
        .collect()
}

pub fn find_duplicate_title(cfg: &Config, title: &str) -> Option<String> {
    let want = slugify(title);
    rows(cfg).into_iter().find_map(|row| {
        let id = meta_str(&row.meta, "id")?;
        if !id.starts_with("t-") {
            return None;
        }
        let other = meta_str(&row.meta, "title")?;
        (slugify(other) == want).then(|| id.to_string())
    })
}

/// The ids of a view list, in order — a convenience for the CLI and for tests.
pub fn view_ids(views: &[View<Task>]) -> Vec<String> {
    views.iter().map(|v| v.item.id.clone()).collect()
}

// ---------------------------------------------------------------------------------------
// graph helpers shared with `deps`
// ---------------------------------------------------------------------------------------

/// `(blocks, blocked_by)` for one id in a scan; two empty lists when it is absent.
pub(crate) fn node_lists(rows: &[Row], id: &str) -> (Vec<String>, Vec<String>) {
    for row in rows {
        if meta_str(&row.meta, "id") == Some(id) {
            return (
                meta_strings(&row.meta, "blocks"),
                meta_strings(&row.meta, "blocked_by"),
            );
        }
    }
    (Vec::new(), Vec::new())
}

/// A copy of `rows` where `id` carries the given lists, adding a synthetic row when absent.
fn with_lists(rows: &[Row], id: &str, blocks: &[String], blocked_by: &[String]) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::with_capacity(rows.len() + 1);
    let mut seen = false;
    for row in rows {
        if meta_str(&row.meta, "id") == Some(id) {
            seen = true;
            let mut meta = row.meta.clone();
            meta.insert("blocks".into(), Value::strings(blocks.to_vec()));
            meta.insert("blocked_by".into(), Value::strings(blocked_by.to_vec()));
            out.push(Row {
                path: row.path.clone(),
                meta,
            });
        } else {
            out.push(row.clone());
        }
    }
    if !seen {
        let mut meta = Meta::new();
        meta.insert("id".into(), Value::str(id));
        meta.insert("type".into(), Value::str("task"));
        meta.insert("status".into(), Value::str("open"));
        meta.insert("blocks".into(), Value::strings(blocks.to_vec()));
        meta.insert("blocked_by".into(), Value::strings(blocked_by.to_vec()));
        out.push(Row {
            path: PathBuf::from(format!("{id}.md")),
            meta,
        });
    }
    out
}

/// The `(blocked, blocker)` pairs a `--blocks` / `--blocked-by` pair proposes.
fn proposed_edges(id: &str, blocks: &[String], blocked_by: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for b in blocked_by {
        out.push((id.to_string(), b.clone()));
    }
    for b in blocks {
        out.push((b.clone(), id.to_string()));
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
    use crate::domain::SortKey;

    fn vault() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        (dir, cfg)
    }

    fn new_task(cfg: &Config, title: &str, o: NewTask) -> Task {
        create(cfg, title, o).unwrap()
    }

    fn open_dir(cfg: &Config) -> PathBuf {
        let dir = folder(cfg, "open").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn raw(cfg: &Config, id: &str, extra: &str) {
        let dir = open_dir(cfg);
        std::fs::write(
            dir.join(format!("{id}.md")),
            format!(
                "---\nid: {id}\ntype: task\ntitle: {id}\ncreated: 2026-01-01T00:00:00Z\n\
                 updated: 2026-01-01T00:00:00Z\nstatus: open\n{extra}---\n\nx\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn create_writes_the_canonical_key_set_into_open() {
        let (_d, cfg) = vault();
        let task = new_task(&cfg, "Ship it", NewTask::default());
        assert!(task.id.starts_with("t-"));
        assert_eq!(task.status, "open");
        let path = resolve(&cfg, &task.id).unwrap();
        assert!(path.to_string_lossy().contains("/tasks/open/"));
        let keys: Vec<&str> = task.meta.keys().map(String::as_str).collect();
        assert_eq!(keys, crate::model::task::TASK_FIELDS.fields());
        // priority / claimed_by / project are written as null, never omitted.
        assert!(task.meta["priority"].is_null());
        assert!(task.meta["claimed_by"].is_null());
        assert!(task.meta["project"].is_null());
        assert_eq!(task.owner.as_deref(), Some("test-agent"));
    }

    #[test]
    fn priority_is_validated_before_any_write() {
        let (_d, cfg) = vault();
        let o = NewTask {
            priority: Some("urgent".into()),
            ..NewTask::default()
        };
        let err = create(&cfg, "T", o).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            "invalid priority: 'urgent' (use high, normal, low)"
        );
        assert!(rows(&cfg).is_empty(), "nothing was created");
    }

    #[test]
    fn an_unknown_owner_is_rejected_before_any_write() {
        let (_d, mut cfg) = vault();
        cfg.tasks.collections = vec!["alice".into()];
        let o = NewTask {
            owner: Some("ghost".into()),
            ..NewTask::default()
        };
        let err = create(&cfg, "T", o).unwrap_err();
        assert_eq!(err.to_string(), "unknown owner: 'ghost'");
        assert!(rows(&cfg).is_empty());
    }

    #[test]
    fn resolution_is_id_only_and_never_a_slug() {
        let (_d, cfg) = vault();
        let task = new_task(&cfg, "Ship it", NewTask::default());
        assert!(resolve(&cfg, &task.id).is_ok());
        assert_eq!(resolve(&cfg, "ship-it").unwrap_err().code(), 3);
        assert_eq!(get(&cfg, "ship-it").unwrap_err().code(), 3);
    }

    #[test]
    fn claim_is_a_four_branch_test_and_set() {
        let (_d, cfg) = vault();
        let t = new_task(&cfg, "T", NewTask::default());
        let (task, _) = claim(&cfg, &t.id, "alice", false).unwrap();
        assert_eq!(task.status, "claimed");
        assert_eq!(task.claimed_by.as_deref(), Some("alice"));

        // Same-owner reclaim: `updated` untouched, no rewrite.
        let path = resolve(&cfg, &t.id).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let (again, _) = claim(&cfg, &t.id, "alice", false).unwrap();
        assert_eq!(again.updated, task.updated);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        let err = claim(&cfg, &t.id, "bob", false).unwrap_err();
        assert_eq!(err.code(), 4);
        assert_eq!(
            err.to_string(),
            format!("task {} already claimed by alice", t.id)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn claiming_a_terminal_task_is_a_no_op_not_a_conflict() {
        let (_d, cfg) = vault();
        let t = new_task(&cfg, "T", NewTask::default());
        claim(&cfg, &t.id, "alice", false).unwrap();
        terminate(&cfg, &t.id, Terminal::Finish, None, Some("alice")).unwrap();
        let path = resolve(&cfg, &t.id).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let (task, _) = claim(&cfg, &t.id, "bob", false).unwrap();
        assert_eq!(task.status, "done");
        // finish never clears claimed_by.
        assert_eq!(task.claimed_by.as_deref(), Some("alice"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn release_is_idempotent_and_force_breaks_a_foreign_claim() {
        let (_d, cfg) = vault();
        let t = new_task(&cfg, "T", NewTask::default());
        let path = resolve(&cfg, &t.id).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let task = release(&cfg, &t.id, "alice", false).unwrap();
        assert_eq!(task.status, "open");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        claim(&cfg, &t.id, "alice", false).unwrap();
        let err = release(&cfg, &t.id, "bob", false).unwrap_err();
        assert_eq!(err.code(), 4);
        let task = release(&cfg, &t.id, "bob", true).unwrap();
        assert_eq!(task.status, "open");
        assert_eq!(task.claimed_by, None);
    }

    #[test]
    fn releasing_a_terminal_task_is_a_no_op() {
        let (_d, cfg) = vault();
        let t = new_task(&cfg, "T", NewTask::default());
        claim(&cfg, &t.id, "alice", false).unwrap();
        terminate(&cfg, &t.id, Terminal::Cancel, None, None).unwrap();
        let path = resolve(&cfg, &t.id).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let task = release(&cfg, &t.id, "bob", false).unwrap();
        assert_eq!(task.status, "cancelled");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn terminate_appends_a_stamped_section_and_moves_the_file() {
        let (_d, cfg) = vault();
        let o = NewTask {
            body: "Original body.".into(),
            ..NewTask::default()
        };
        let t = new_task(&cfg, "T", o);
        let (task, unblocked) = terminate(
            &cfg,
            &t.id,
            Terminal::Finish,
            Some("Shipped."),
            Some("alice"),
        )
        .unwrap();
        assert_eq!(task.status, "done");
        assert!(unblocked.is_empty());
        let path = resolve(&cfg, &t.id).unwrap();
        assert!(path.to_string_lossy().contains("/tasks/done/"));
        let body = crate::fm::read_body(&path);
        assert!(
            body.starts_with("Original body.\n\n## Outcome\n\n"),
            "{body}"
        );
        assert!(body.ends_with(" — alice\nShipped."), "{body}");

        // A second finish is a pure no-op.
        let before = std::fs::read_to_string(&path).unwrap();
        terminate(&cfg, &t.id, Terminal::Finish, Some("again"), Some("alice")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        // A cross-transition is refused as a no-op too.
        let (task, _) = terminate(&cfg, &t.id, Terminal::Cancel, None, None).unwrap();
        assert_eq!(task.status, "done");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn terminate_with_no_text_is_heading_plus_stamp_only() {
        let (_d, cfg) = vault();
        let t = new_task(&cfg, "T", NewTask::default());
        terminate(&cfg, &t.id, Terminal::Cancel, None, Some("alice")).unwrap();
        let body = crate::fm::read_body(&resolve(&cfg, &t.id).unwrap());
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines[0], "## Cancelled");
        assert_eq!(lines[1], "");
        assert!(lines[2].ends_with(" — alice"));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn a_crash_stranded_terminal_file_is_reconciled_into_done() {
        let (_d, cfg) = vault();
        let t = new_task(&cfg, "T", NewTask::default());
        // Simulate a crash between the write and the rename.
        let path = resolve(&cfg, &t.id).unwrap();
        let mut doc = read_doc(&path).unwrap();
        doc.meta.insert("status".into(), Value::str("done"));
        persist(&cfg, &path, &doc).unwrap();
        assert!(path.to_string_lossy().contains("/tasks/open/"));

        terminate(&cfg, &t.id, Terminal::Finish, None, None).unwrap();
        let healed = resolve(&cfg, &t.id).unwrap();
        assert!(healed.to_string_lossy().contains("/tasks/done/"));
        assert!(!path.exists());
    }

    #[test]
    fn append_never_transitions_a_lifecycle() {
        let (_d, cfg) = vault();
        let o = NewTask {
            body: "start".into(),
            ..NewTask::default()
        };
        let t = new_task(&cfg, "T", o);
        terminate(&cfg, &t.id, Terminal::Finish, None, None).unwrap();
        let task = append(&cfg, &t.id, "more", AppendOpts::default()).unwrap();
        assert_eq!(task.status, "done");
        let path = resolve(&cfg, &t.id).unwrap();
        assert!(path.to_string_lossy().contains("/tasks/done/"));
        let body = crate::fm::read_body(&path);
        assert_eq!(body.matches("## Outcome").count(), 1);
        assert!(body.ends_with("more"));
    }

    #[test]
    fn append_under_a_section_and_with_a_stamp() {
        let (_d, cfg) = vault();
        let o = NewTask {
            body: "Intro.\n\n## A\n\nitem1\n\n## B\n\nitem2".into(),
            ..NewTask::default()
        };
        let t = new_task(&cfg, "T", o);
        append(
            &cfg,
            &t.id,
            "NEW",
            AppendOpts {
                section: Some("A".into()),
                ..AppendOpts::default()
            },
        )
        .unwrap();
        let body = crate::fm::read_body(&resolve(&cfg, &t.id).unwrap());
        assert_eq!(body, "Intro.\n\n## A\n\nitem1\n\nNEW\n\n## B\n\nitem2");

        append(
            &cfg,
            &t.id,
            "stamped",
            AppendOpts {
                section: None,
                timestamp: true,
                actor: Some("carol".into()),
            },
        )
        .unwrap();
        let body = crate::fm::read_body(&resolve(&cfg, &t.id).unwrap());
        assert!(body.contains(" — carol\nstamped"), "{body}");
    }

    #[test]
    fn update_touches_only_the_supplied_fields() {
        let (_d, cfg) = vault();
        let t = new_task(&cfg, "T", NewTask::default());
        claim(&cfg, &t.id, "alice", false).unwrap();
        let o = UpdateTask {
            owner: Some("bob".into()),
            title: Some("Renamed".into()),
            ..UpdateTask::default()
        };
        let task = update(&cfg, &t.id, o).unwrap();
        assert_eq!(task.title, "Renamed");
        assert_eq!(task.owner.as_deref(), Some("bob"));
        // Reassignment never touches the claim.
        assert_eq!(task.claimed_by.as_deref(), Some("alice"));
        assert_eq!(task.status, "claimed");
    }

    #[test]
    fn update_round_trips_unknown_keys_and_absent_optionals() {
        let (_d, cfg) = vault();
        let dir = open_dir(&cfg);
        std::fs::write(
            dir.join("t-LEG.md"),
            "---\nid: t-LEG\ntype: task\ntitle: Legacy\ncreated: 2026-01-01T00:00:00Z\n\
             updated: 2026-01-01T00:00:00Z\nstatus: open\ncustom: keep\n---\n\nbody\n",
        )
        .unwrap();
        let o = UpdateTask {
            priority: Some("low".into()),
            ..UpdateTask::default()
        };
        update(&cfg, "t-LEG", o).unwrap();
        let meta = read_meta_only(&dir.join("t-LEG.md")).unwrap();
        assert_eq!(meta_str(&meta, "custom"), Some("keep"));
        assert_eq!(meta_str(&meta, "priority"), Some("low"));
        // An update without --project injects none.
        assert!(!meta.contains_key("project"));
    }

    #[test]
    fn update_tags_go_through_the_shared_spec_grammar() {
        let (_d, cfg) = vault();
        let t = new_task(
            &cfg,
            "T",
            NewTask {
                tags: vec!["a".into(), "b".into()],
                ..NewTask::default()
            },
        );
        let task = update(
            &cfg,
            &t.id,
            UpdateTask {
                tags: Some("+c,-a".into()),
                ..UpdateTask::default()
            },
        )
        .unwrap();
        assert_eq!(task.tags, ["b", "c"]);
        let err = update(
            &cfg,
            &t.id,
            UpdateTask {
                tags: Some("+c,d".into()),
                ..UpdateTask::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), 2);
    }

    #[test]
    fn delete_removes_a_task_in_any_state_including_a_corrupt_one() {
        let (_d, cfg) = vault();
        let t = new_task(&cfg, "T", NewTask::default());
        assert_eq!(delete(&cfg, &t.id).unwrap(), t.id);
        assert_eq!(delete(&cfg, &t.id).unwrap_err().code(), 3);

        let dir = open_dir(&cfg);
        std::fs::write(dir.join("t-BAD.md"), "---\n: : :\n---\nbroken\n").unwrap();
        assert_eq!(get(&cfg, "t-BAD").unwrap_err().code(), 3);
        assert_eq!(delete(&cfg, "t-BAD").unwrap(), "t-BAD");
    }

    #[test]
    fn status_csv_parsing_trims_dedupes_and_names_every_offender() {
        assert_eq!(parse_status_csv(" , ").unwrap(), None);
        assert_eq!(
            parse_status_csv(" open , done , open ").unwrap(),
            Some(vec!["open".into(), "done".into()])
        );
        let err = parse_status_csv("open,wat,nope").unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            "unknown status: wat, nope (use open, claimed, done, cancelled)"
        );
    }

    #[test]
    fn list_availability_slices() {
        let (_d, cfg) = vault();
        let a = new_task(&cfg, "A", NewTask::default());
        let b = new_task(&cfg, "B", NewTask::default());
        claim(&cfg, &b.id, "alice", false).unwrap();
        let c = new_task(
            &cfg,
            "C",
            NewTask {
                blocked_by: vec![a.id.clone()],
                ..NewTask::default()
            },
        );
        let f = Filter::unbounded();
        assert_eq!(list(&cfg, &f, Availability::Any).unwrap().len(), 3);
        let available = view_ids(&list(&cfg, &f, Availability::Available).unwrap());
        assert!(available.contains(&a.id) && available.contains(&c.id));
        assert!(!available.contains(&b.id));
        assert_eq!(
            view_ids(&list(&cfg, &f, Availability::Ready).unwrap()),
            [a.id.clone()][..]
        );
        assert_eq!(
            view_ids(&list(&cfg, &f, Availability::Blocked).unwrap()),
            [c.id.clone()][..]
        );

        // Finishing the blocker flips C to ready.
        terminate(&cfg, &a.id, Terminal::Finish, None, None).unwrap();
        assert_eq!(
            view_ids(&list(&cfg, &f, Availability::Ready).unwrap()),
            [c.id]
        );
        assert!(list(&cfg, &f, Availability::Blocked).unwrap().is_empty());
    }

    #[test]
    fn available_excludes_a_hand_edited_open_task_carrying_a_claim() {
        let (_d, cfg) = vault();
        raw(&cfg, "t-STALE", "claimed_by: ghost\n");
        let f = Filter::unbounded();
        assert_eq!(list(&cfg, &f, Availability::Any).unwrap().len(), 1);
        assert!(list(&cfg, &f, Availability::Available).unwrap().is_empty());
    }

    #[test]
    fn a_legacy_priority_sorts_last_and_is_never_dropped() {
        let (_d, cfg) = vault();
        let dir = open_dir(&cfg);
        for (id, priority, created) in [
            ("t-AAA", "urgent", "2026-01-01T00:00:00Z"),
            ("t-BBB", "high", "2026-01-02T00:00:00Z"),
            ("t-CCC", "low", "2026-01-03T00:00:00Z"),
        ] {
            std::fs::write(
                dir.join(format!("{id}.md")),
                format!(
                    "---\nid: {id}\ntype: task\ntitle: {id}\ncreated: {created}\n\
                     updated: {created}\nstatus: open\npriority: {priority}\n---\n\nx\n"
                ),
            )
            .unwrap();
        }
        let f = Filter {
            sort: SortKey::Priority,
            ..Filter::unbounded()
        };
        assert_eq!(
            view_ids(&list(&cfg, &f, Availability::Any).unwrap()),
            ["t-BBB", "t-CCC", "t-AAA"]
        );
    }

    #[test]
    fn duplicate_titles_are_slug_normalised_and_task_only() {
        let (_d, cfg) = vault();
        let t = new_task(&cfg, "Japan Visa", NewTask::default());
        assert_eq!(
            find_duplicate_title(&cfg, "japan  visa").as_deref(),
            Some(t.id.as_str())
        );
        assert_eq!(find_duplicate_title(&cfg, "Something else"), None);
        // The scan covers done/ too.
        terminate(&cfg, &t.id, Terminal::Finish, None, None).unwrap();
        assert_eq!(
            find_duplicate_title(&cfg, "Japan Visa").as_deref(),
            Some(t.id.as_str())
        );
    }

    #[test]
    fn a_corrupt_sibling_never_blocks_a_different_id() {
        let (_d, cfg) = vault();
        let t = new_task(&cfg, "T", NewTask::default());
        let dir = open_dir(&cfg);
        std::fs::write(dir.join("t-BAD.md"), "---\n: : :\n---\nbroken\n").unwrap();
        assert!(claim(&cfg, &t.id, "alice", false).is_ok());
        assert!(terminate(&cfg, &t.id, Terminal::Finish, None, None).is_ok());
        assert_eq!(
            list(&cfg, &Filter::unbounded(), Availability::Any)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn not_found_carries_near_miss_candidates() {
        let (_d, cfg) = vault();
        let t = new_task(&cfg, "T", NewTask::default());
        let err = get(&cfg, "t-ZZZZ").unwrap_err();
        assert_eq!(err.code(), 3);
        assert!(err.candidates().contains(&t.id), "{:?}", err.candidates());
    }

    #[test]
    fn strict_claim_on_a_blocked_task_is_exit_five_and_writes_nothing() {
        let (_d, cfg) = vault();
        let a = new_task(&cfg, "A", NewTask::default());
        let b = new_task(
            &cfg,
            "B",
            NewTask {
                blocked_by: vec![a.id.clone()],
                ..NewTask::default()
            },
        );
        let path = resolve(&cfg, &b.id).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let err = claim(&cfg, &b.id, "alice", true).unwrap_err();
        assert_eq!(err.code(), 5);
        assert_eq!(
            err.to_string(),
            format!("task {} is blocked by {}", b.id, a.id)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        let (task, unsatisfied) = claim(&cfg, &b.id, "alice", false).unwrap();
        assert_eq!(task.status, "claimed");
        assert_eq!(unsatisfied, [a.id]);
    }

    #[test]
    fn finishing_a_blocker_reports_the_newly_ready_dependents() {
        let (_d, cfg) = vault();
        let a = new_task(&cfg, "A", NewTask::default());
        let b = new_task(
            &cfg,
            "B",
            NewTask {
                blocked_by: vec![a.id.clone()],
                ..NewTask::default()
            },
        );
        let (_, unblocked) = terminate(&cfg, &a.id, Terminal::Finish, None, None).unwrap();
        assert_eq!(unblocked, [b.id]);
    }

    #[test]
    fn a_cycle_is_refused_before_any_write() {
        let (_d, cfg) = vault();
        let a = new_task(&cfg, "A", NewTask::default());
        let b = new_task(
            &cfg,
            "B",
            NewTask {
                blocked_by: vec![a.id.clone()],
                ..NewTask::default()
            },
        );
        let path = resolve(&cfg, &a.id).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let o = UpdateTask {
            blocked_by: Some(vec![b.id]),
            ..UpdateTask::default()
        };
        let err = update(&cfg, &a.id, o).unwrap_err();
        assert_eq!(err.code(), 2);
        assert!(err.to_string().starts_with("dependency cycle: "), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn create_and_update_mirror_the_edges_they_write() {
        let (_d, cfg) = vault();
        let a = new_task(&cfg, "A", NewTask::default());
        let b = new_task(
            &cfg,
            "B",
            NewTask {
                blocked_by: vec![a.id.clone()],
                ..NewTask::default()
            },
        );
        assert_eq!(get(&cfg, &a.id).unwrap().item.blocks, [b.id.clone()][..]);

        // A replace retracts the mirror it dropped.
        let o = UpdateTask {
            blocked_by: Some(vec![]),
            ..UpdateTask::default()
        };
        update(&cfg, &b.id, o).unwrap();
        assert!(get(&cfg, &a.id).unwrap().item.blocks.is_empty());
        assert!(deps::readiness(&rows(&cfg), &b.id).ready);
    }

    #[test]
    fn a_disabled_tasks_space_is_a_validation_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.spaces = crate::spaces::Spaces::resolve(
            dir.path(),
            &[(Space::Tasks, crate::spaces::SpaceSetting::Disabled)],
        )
        .unwrap();
        let err = create(&cfg, "T", NewTask::default()).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "space 'tasks' is disabled in [spaces]");
    }
}
