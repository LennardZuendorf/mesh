//! `mesh watch` — the foreground watcher: singleton lock, debounce, reconcile, index update.
//!
//! The daemon is gone (final.md §8). What survives is one blocking process per vault that
//! keeps `indexed` fresh and heals mis-foldered files. Every failure inside the loop is
//! swallowed on purpose: one escaping error would freeze freshness for the whole lifetime of
//! the watch.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use serde_json::{Map, Value as Json};

use crate::cli::out;
use crate::cli::WatchArgs;
use crate::config::Config;
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};
use crate::model::common::meta_str;
use crate::model::note::NOTE_TYPES;
use crate::spaces::Space;

/// The notice printed once when `indexed` cannot be reached.
pub const INDEXED_NOTICE: &str = "watch: indexed unavailable — watching for reconcile only";
/// How long the event loop blocks before re-checking the stop flag and refreshing the lock.
const TICK: Duration = Duration::from_millis(200);
/// How often the watcher touches its own lock so the TTL sweeper cannot reclaim it.
const LOCK_REFRESH: Duration = Duration::from_secs(60);

/// The roots `mesh watch` walks: `--space` when given, every enabled space otherwise.
fn watched_roots(cfg: &Config, csv: Option<&str>) -> Result<Vec<PathBuf>> {
    let names: Vec<String> = csv
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let mut roots: Vec<PathBuf> = Vec::new();
    if names.is_empty() {
        for space in Space::ALL {
            if let Ok(root) = cfg.root(space) {
                let root = root.to_path_buf();
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        return Ok(roots);
    }
    for name in names {
        let Some(space) = Space::from_name(&name) else {
            return Err(MeshError::Validation(format!(
                "unknown space: '{name}' (use notes, tasks, memories, scratch, assets)"
            )));
        };
        let root = cfg.root(space)?.to_path_buf();
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    Ok(roots)
}

/// One NDJSON line on stdout, under `--json` only.
fn event(ctx: &Ctx, kind: &str, fields: &[(&str, Json)]) {
    if !ctx.g.json {
        return;
    }
    let mut payload = Map::new();
    payload.insert("event".to_string(), Json::String(kind.to_string()));
    for (key, value) in fields {
        payload.insert((*key).to_string(), value.clone());
    }
    payload.insert(
        "ts".to_string(),
        Json::String(crate::timefmt::iso_z(&crate::timefmt::now_utc())),
    );
    out::line(out::json_line(&Json::Object(payload)).trim_end());
}

fn path_json(path: &Path) -> Json {
    Json::String(path.display().to_string())
}

/// Whether a raw watcher event is one this loop should act on.
///
/// Two filters, both load-bearing. `notify`'s inotify mask includes `IN_OPEN`, so **reading a
/// file's frontmatter emits an event for that same file** — acting on `Access` would spin the
/// loop forever on one healed note. And the `.locks/` directory this watcher writes into is
/// skipped for the same reason, along with every other dot path the vault walk skips.
fn interesting(kind: &notify::EventKind, path: &Path, roots: &[PathBuf]) -> bool {
    use notify::EventKind;
    if !matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return false;
    }
    if path.is_dir() {
        return false;
    }
    let relative = roots
        .iter()
        .filter_map(|root| path.strip_prefix(root).ok())
        .min_by_key(|rest| rest.components().count())
        .unwrap_or_else(|| Path::new(path.file_name().unwrap_or(path.as_os_str())));
    !relative
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
}

/// Reconcile then index one changed path. Every error is swallowed.
fn handle_path(ctx: &Ctx, cfg: &Config, path: &Path, args: &WatchArgs, indexed: bool) {
    let landed = if args.no_reconcile {
        path.to_path_buf()
    } else {
        let moved = reconcile_path(cfg, path);
        if moved != path {
            event(
                ctx,
                "reconcile",
                &[("path", path_json(path)), ("to", path_json(&moved))],
            );
        }
        moved
    };
    event(ctx, "change", &[("path", path_json(&landed))]);
    if indexed && crate::search::index_update(cfg, &landed).is_ok() {
        event(ctx, "index", &[("path", path_json(&landed))]);
    }
}

/// A full reconcile sweep over every watched root. Returns how many files moved.
fn sweep(ctx: &Ctx, cfg: &Config, roots: &[PathBuf]) -> usize {
    let mut moved = 0;
    for root in roots {
        let exclusions = space_of(cfg, root).map_or(&[][..], |s| cfg.spaces.exclusions_for(s));
        for path in crate::storage::iter_md(root, true, exclusions) {
            let dest = reconcile_path(cfg, &path);
            if dest != path {
                moved += 1;
                event(
                    ctx,
                    "reconcile",
                    &[("path", path_json(&path)), ("to", path_json(&dest))],
                );
            }
        }
    }
    moved
}

/// Run the watcher.
pub fn run(ctx: &mut Ctx, args: WatchArgs) -> Result<()> {
    ctx.coalesce(args.json, false, None);
    let cfg = ctx.cfg()?.clone();
    let roots = watched_roots(&cfg, args.space.as_deref())?;

    // Singleton: a second watcher on the same vault exits 4.
    let lock_path = watch_lock_path(&cfg);
    let guard = crate::storage::acquire(&lock_path).map_err(|e| match e {
        MeshError::Lock(_) => {
            let pid = read_pid(&lock_path).unwrap_or(0);
            MeshError::Lock(format!("watch: already running (pid {pid})"))
        }
        other => other,
    })?;

    let indexed = !args.no_index && crate::search::indexed_available(&cfg);
    if !args.no_index && !indexed {
        out::notice(ctx, INDEXED_NOTICE);
    }

    if args.once {
        let moved = if args.no_reconcile {
            0
        } else {
            sweep(ctx, &cfg, &roots)
        };
        // The sweep always attempts the rebuild; `search::reindex` is the one that knows
        // whether a collection is configured and whether the binary answered.
        let rebuilt = !args.no_index && crate::search::reindex(&cfg, &roots).is_ok();
        event(
            ctx,
            "sweep",
            &[
                ("reconciled", Json::from(moved)),
                ("indexed", Json::Bool(rebuilt)),
            ],
        );
        drop(guard);
        return Ok(());
    }

    let stop = AtomicBool::new(false);
    let outcome = watch_loop(ctx, &cfg, &roots, &args, indexed, &lock_path, &stop);
    drop(guard);
    outcome
}

/// The blocking debounce loop. `stop` ends it deterministically; a plain SIGINT also works,
/// because nothing in the loop owns state that needs unwinding.
#[allow(clippy::too_many_arguments)]
fn watch_loop(
    ctx: &Ctx,
    cfg: &Config,
    roots: &[PathBuf],
    args: &WatchArgs,
    indexed: bool,
    lock_path: &Path,
    stop: &AtomicBool,
) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer =
        notify_debouncer_full::new_debouncer(Duration::from_millis(args.debounce), None, tx)
            .map_err(notify_error)?;
    for root in roots {
        // A space folder is created lazily by the first write; the watcher needs it now.
        let _ = std::fs::create_dir_all(root);
        debouncer
            .watch(root, notify::RecursiveMode::Recursive)
            .map_err(notify_error)?;
    }
    event(
        ctx,
        "start",
        &[
            ("vault", path_json(cfg.vault())),
            (
                "roots",
                Json::Array(roots.iter().map(|r| path_json(r)).collect()),
            ),
            ("pid", Json::from(std::process::id())),
            ("debounce", Json::from(args.debounce)),
        ],
    );

    let mut refreshed = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(TICK) {
            Ok(Ok(events)) => {
                let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
                for debounced in events {
                    for path in &debounced.event.paths {
                        if interesting(&debounced.event.kind, path, roots)
                            && seen.insert(path.clone())
                        {
                            handle_path(ctx, cfg, path, args, indexed);
                        }
                    }
                }
            }
            // Watcher errors are reported by notify and are never fatal here.
            Ok(Err(_)) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if refreshed.elapsed() >= LOCK_REFRESH {
            touch(lock_path);
            refreshed = Instant::now();
        }
    }
    event(ctx, "stop", &[]);
    Ok(())
}

/// Keep the singleton lock's mtime fresh so its TTL never marks a live watcher stale.
fn touch(lock_path: &Path) {
    if let Ok(file) = std::fs::OpenOptions::new().append(true).open(lock_path) {
        let _ =
            file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()));
    }
}

fn notify_error(e: notify::Error) -> MeshError {
    MeshError::Io(std::io::Error::other(e.to_string()))
}

/// `$XDG_RUNTIME_DIR/mesh-<hash12>.watch.lock`, else `~/.mesh/run/`.
pub fn watch_lock_path(cfg: &Config) -> PathBuf {
    let digest = crate::ids::sha256_hex(cfg.vault().to_string_lossy().as_bytes());
    let short = digest.get(..12).unwrap_or("mesh").to_string();
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".mesh")
                .join("run")
        });
    dir.join(format!("mesh-{short}.watch.lock"))
}

fn read_pid(lock_path: &Path) -> Option<u32> {
    std::fs::read_to_string(lock_path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// The pid of a live watcher for this vault, if there is one.
///
/// A stale lock — a dead pid, or one aged past `storage::lock::LOCK_TTL` — reads as "not
/// running", which is exactly what `acquire` would do with it.
pub fn watcher_pid(cfg: &Config) -> Option<u32> {
    let path = watch_lock_path(cfg);
    if !path.is_file() || crate::storage::is_stale(&path) {
        return None;
    }
    read_pid(&path)
}

// --------------------------------------------------------------------------------------------
// reconcile (map/search.md §12)
// --------------------------------------------------------------------------------------------

/// The subfolder a note type lives in, relative to the notes root. `None` for an unknown type.
fn note_subdir(note_type: &str) -> Option<&'static str> {
    match note_type {
        "note" => Some(""),
        "log" => Some("logs"),
        "decision" => Some("decisions"),
        "reference" => Some("references"),
        "project" => Some("projects"),
        _ => None,
    }
}

/// The subfolder a task status lives in, relative to the tasks root.
fn task_subdir(status: &str) -> Option<&'static str> {
    match status {
        "open" | "claimed" => Some("open"),
        "done" | "cancelled" => Some("done"),
        _ => None,
    }
}

/// The enabled space a path belongs to: the longest matching root wins, so a nested space
/// beats an Obsidian-style `notes = "."`.
fn space_of(cfg: &Config, path: &Path) -> Option<Space> {
    let mut best: Option<(usize, Space)> = None;
    for space in Space::ALL {
        let Ok(root) = cfg.root(space) else { continue };
        if path == root || path.starts_with(root) {
            let depth = root.components().count();
            if best.is_none_or(|(seen, _)| depth > seen) {
                best = Some((depth, space));
            }
        }
    }
    best.map(|(_, space)| space)
}

/// Where a file belongs given its frontmatter, moving it there when it is misfiled.
///
/// Every guard from `map/search.md` §12.3 is reproduced in order, and the no-move branch hands
/// back **the caller's own path**, never a realpath — returning the canonical path there moves
/// every edited row into a second path space.
pub fn reconcile_path(cfg: &Config, path: &Path) -> PathBuf {
    let own = path.to_path_buf();
    // 1. Not Markdown: a sidecar carrying mesh-shaped frontmatter stays where its owner put it.
    if path.extension().and_then(|e| e.to_str()) != Some("md") {
        return own;
    }
    // Only the note and task spaces have a folder layout to heal.
    let space = match space_of(cfg, path) {
        Some(space @ (Space::Notes | Space::Tasks)) => space,
        _ => return own,
    };
    // 2. Vanished, unreadable or malformed: leave it alone.
    let Some(meta) = crate::fm::read_meta_only(path) else {
        return own;
    };
    // 3. Not a mesh id: foreign files are never moved.
    let Some(id) = meta_str(&meta, "id") else {
        return own;
    };
    if !crate::text::is_id_form(id) {
        return own;
    }
    let id = id.to_string();
    // 4. Unknown type or status: leave it alone.
    let Some(folder) = correct_folder(cfg, &meta) else {
        return own;
    };
    // 5. Sandbox escape on either side: leave it alone.
    let Ok(src) = crate::storage::safe_resolve(&cfg.spaces, path) else {
        return own;
    };
    let Some(name) = path.file_name() else {
        return own;
    };
    let Ok(dest) = crate::storage::safe_resolve(&cfg.spaces, &folder.join(name)) else {
        return own;
    };
    // 6. Already correct: the caller's own path space, never the realpath.
    if src == dest {
        return own;
    }
    // 7. A real move, under the same non-blocking per-entity lock every writer holds.
    let Ok(space_root) = cfg.root(space) else {
        return own;
    };
    if let Some(parent) = dest.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return own;
        }
    }
    let Ok(_guard) = crate::storage::acquire(&crate::storage::entity_lock(space_root, &id)) else {
        // A writer holds this entity — heal it on a later event.
        return own;
    };
    if !src.exists() {
        return own;
    }
    if std::fs::rename(&src, &dest).is_err() {
        return own;
    }
    dest
}

/// The folder a file's frontmatter says it belongs in; `None` leaves it in place.
fn correct_folder(cfg: &Config, meta: &crate::fm::Meta) -> Option<PathBuf> {
    let kind = meta_str(meta, "type").unwrap_or("note");
    if kind == "task" {
        let sub = task_subdir(meta_str(meta, "status")?)?;
        return Some(cfg.root(Space::Tasks).ok()?.join(sub));
    }
    if !NOTE_TYPES.contains(&kind) {
        return None;
    }
    let sub = note_subdir(kind)?;
    let root = cfg.root(Space::Notes).ok()?;
    Some(if sub.is_empty() {
        root.to_path_buf()
    } else {
        root.join(sub)
    })
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
    use crate::model::task::TASK_STATUSES;

    fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    const TASK_DONE: &str = "---\nid: t-AAAA\ntype: task\ntitle: Ship\nstatus: done\n---\n\nBody\n";
    const NOTE_DECISION: &str =
        "---\nid: n-BBBB\ntype: decision\ntitle: Pick\nreviewer: alice\n---\n\nBody\n";

    #[test]
    fn the_subdir_tables_match_the_python_maps() {
        assert_eq!(note_subdir("note"), Some(""));
        assert_eq!(note_subdir("log"), Some("logs"));
        assert_eq!(note_subdir("decision"), Some("decisions"));
        assert_eq!(note_subdir("reference"), Some("references"));
        assert_eq!(note_subdir("project"), Some("projects"));
        assert_eq!(note_subdir("task"), None);
        assert_eq!(task_subdir("open"), Some("open"));
        assert_eq!(task_subdir("claimed"), Some("open"));
        assert_eq!(task_subdir("done"), Some("done"));
        assert_eq!(task_subdir("cancelled"), Some("done"));
        assert_eq!(task_subdir("None"), None);
        for status in TASK_STATUSES {
            assert!(task_subdir(status).is_some());
        }
        for kind in NOTE_TYPES {
            assert!(note_subdir(kind).is_some());
        }
    }

    #[test]
    fn a_misfiled_task_moves_and_keeps_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let src = write(dir.path(), "tasks/open/t-AAAA.md", TASK_DONE);
        let dest = reconcile_path(&cfg, &src);
        assert_eq!(dest, dir.path().join("tasks/done/t-AAAA.md"));
        assert!(!src.exists());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), TASK_DONE);
    }

    #[test]
    fn a_misfiled_note_moves_into_its_type_folder() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let src = write(dir.path(), "notes/n-BBBB.md", NOTE_DECISION);
        let dest = reconcile_path(&cfg, &src);
        assert_eq!(dest, dir.path().join("notes/decisions/n-BBBB.md"));
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), NOTE_DECISION);
    }

    #[test]
    fn a_correctly_filed_file_returns_the_callers_own_path() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let src = write(dir.path(), "tasks/done/t-AAAA.md", TASK_DONE);
        // The caller's path space, verbatim — not a realpath.
        assert_eq!(reconcile_path(&cfg, &src), src);
        assert!(src.exists());
    }

    #[test]
    fn every_hostile_file_is_left_where_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let hostile: Vec<(&str, &str)> = vec![
            ("notes/notmarkdown.txt", TASK_DONE),
            ("notes/malformed.md", "---\nid: n-CCCC\n  bad: [\n---\n\nx"),
            ("notes/tabs.md", "---\nid: n-DDDD\n\ttype: note\n---\n\nx"),
            (
                "notes/bogus-type.md",
                "---\nid: n-EEEE\ntype: wat\ntitle: X\n---\n\nx",
            ),
            (
                "tasks/open/bogus-status.md",
                "---\nid: t-FFFF\ntype: task\nstatus: wat\n---\n\nx",
            ),
            ("notes/empty.md", ""),
            ("notes/no-frontmatter.md", "# Just a heading\n"),
            (
                "notes/null-id.md",
                "---\nid: null\ntype: decision\n---\n\nx",
            ),
            (
                "notes/list-id.md",
                "---\nid:\n  - n-1\ntype: decision\n---\n\nx",
            ),
            (
                "notes/foreign.md",
                "---\ntitle: Foreign\ntype: decision\n---\n\nx",
            ),
            (
                "notes/no-status-task.md",
                "---\nid: t-GGGG\ntype: task\ntitle: X\n---\n\nx",
            ),
        ];
        for (rel, body) in &hostile {
            let path = write(dir.path(), rel, body);
            assert_eq!(reconcile_path(&cfg, &path), path, "{rel} must not move");
            assert!(path.exists(), "{rel} must survive");
        }
    }

    #[test]
    fn a_file_outside_the_note_and_task_spaces_is_never_moved() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        // A memory carrying a note-shaped type must not be dragged into notes/.
        let path = write(
            dir.path(),
            "memories/m-AAAA.md",
            "---\nid: m-AAAA\ntype: note\ntitle: X\n---\n\nx",
        );
        assert_eq!(reconcile_path(&cfg, &path), path);
        assert!(path.exists());
        // A path outside every space root is left alone too.
        let outside = write(dir.path(), "stray/n-ZZZZ.md", NOTE_DECISION);
        assert_eq!(reconcile_path(&cfg, &outside), outside);
        assert!(outside.exists());
    }

    #[test]
    fn a_contended_entity_is_left_for_a_later_event() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let src = write(dir.path(), "tasks/open/t-AAAA.md", TASK_DONE);
        let tasks_root = cfg.root(Space::Tasks).unwrap().to_path_buf();
        let held = crate::storage::acquire(&crate::storage::entity_lock(&tasks_root, "t-AAAA"))
            .expect("hold the entity lock");
        assert_eq!(reconcile_path(&cfg, &src), src);
        assert!(src.exists());
        drop(held);
        // Once the writer is gone the next event heals it.
        assert_eq!(
            reconcile_path(&cfg, &src),
            dir.path().join("tasks/done/t-AAAA.md")
        );
    }

    #[test]
    fn reconcile_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let src = write(dir.path(), "notes/n-BBBB.md", NOTE_DECISION);
        let first = reconcile_path(&cfg, &src);
        let second = reconcile_path(&cfg, &first);
        assert_eq!(first, second);
        assert_eq!(std::fs::read_to_string(&second).unwrap(), NOTE_DECISION);
    }

    #[test]
    fn the_lock_path_is_vault_keyed_and_stable() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let one = watch_lock_path(&config_for(a.path()));
        let two = watch_lock_path(&config_for(b.path()));
        assert_ne!(one, two);
        assert_eq!(one, watch_lock_path(&config_for(a.path())));
        let name = one.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("mesh-"), "{name}");
        assert!(name.ends_with(".watch.lock"), "{name}");
        assert_eq!(name.len(), "mesh-".len() + 12 + ".watch.lock".len());
    }

    #[test]
    fn watcher_pid_reads_the_lock_and_ignores_a_dead_one() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let lock = watch_lock_path(&cfg);
        assert_eq!(watcher_pid(&cfg), None, "no lock, no watcher");
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        std::fs::write(&lock, format!("{}\n", std::process::id())).unwrap();
        assert_eq!(watcher_pid(&cfg), Some(std::process::id()));
        std::fs::write(&lock, "4194303\n").unwrap();
        assert_eq!(watcher_pid(&cfg), None, "a dead pid is not a watcher");
        std::fs::write(&lock, "not-a-pid\n").unwrap();
        assert_eq!(watcher_pid(&cfg), None);
        let _ = std::fs::remove_file(&lock);
    }

    #[test]
    fn watched_roots_default_to_every_enabled_space() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        assert_eq!(watched_roots(&cfg, None).unwrap().len(), 5);
        assert_eq!(watched_roots(&cfg, Some("notes")).unwrap().len(), 1);
        assert_eq!(watched_roots(&cfg, Some("notes,tasks")).unwrap().len(), 2);
        assert_eq!(watched_roots(&cfg, Some("notes,notes")).unwrap().len(), 1);
        let err = watched_roots(&cfg, Some("nope")).unwrap_err();
        assert_eq!(err.code(), 2);
    }

    #[test]
    fn space_of_prefers_the_deepest_root() {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[core]\nvault_path = \"{}\"\n[spaces]\nnotes = \".\"\n",
            dir.path().display()
        );
        let table: toml::Table = text.parse().unwrap();
        let cfg = crate::config::from_table(&table, None).unwrap();
        let root = crate::storage::realpath(dir.path());
        assert_eq!(space_of(&cfg, &root.join("x.md")), Some(Space::Notes));
        assert_eq!(
            space_of(&cfg, &root.join("tasks/open/t-1.md")),
            Some(Space::Tasks)
        );
    }

    #[test]
    fn correct_folder_maps_types_and_statuses() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let mut meta = crate::fm::Meta::new();
        meta.insert("type".into(), crate::fm::Value::str("log"));
        assert_eq!(
            correct_folder(&cfg, &meta),
            Some(cfg.root(Space::Notes).unwrap().join("logs"))
        );
        meta.insert("type".into(), crate::fm::Value::str("note"));
        assert_eq!(
            correct_folder(&cfg, &meta),
            Some(cfg.root(Space::Notes).unwrap().to_path_buf())
        );
        meta.insert("type".into(), crate::fm::Value::str("task"));
        assert_eq!(correct_folder(&cfg, &meta), None, "a task needs a status");
        meta.insert("status".into(), crate::fm::Value::str("claimed"));
        assert_eq!(
            correct_folder(&cfg, &meta),
            Some(cfg.root(Space::Tasks).unwrap().join("open"))
        );
        // A missing type defaults to "note".
        let empty = crate::fm::Meta::new();
        assert_eq!(
            correct_folder(&cfg, &empty),
            Some(cfg.root(Space::Notes).unwrap().to_path_buf())
        );
    }

    #[test]
    fn only_write_shaped_events_on_real_files_are_acted_on() {
        use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind};
        use notify::EventKind;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("notes");
        std::fs::create_dir_all(root.join(".locks")).unwrap();
        let roots = vec![root.clone()];
        let file = write(dir.path(), "notes/n-AAAA.md", "x");

        assert!(interesting(
            &EventKind::Create(CreateKind::File),
            &file,
            &roots
        ));
        assert!(interesting(
            &EventKind::Modify(ModifyKind::Any),
            &file,
            &roots
        ));
        assert!(interesting(
            &EventKind::Remove(RemoveKind::File),
            &file,
            &roots
        ));
        // Reading a file emits IN_OPEN; acting on it would spin this loop forever.
        assert!(!interesting(
            &EventKind::Access(AccessKind::Open(AccessMode::Read)),
            &file,
            &roots
        ));
        assert!(!interesting(&EventKind::Other, &file, &roots));
        // Directories and dot paths are never events worth handling.
        assert!(!interesting(
            &EventKind::Create(CreateKind::Folder),
            &root,
            &roots
        ));
        assert!(!interesting(
            &EventKind::Create(CreateKind::File),
            &root.join(".locks/n-AAAA.lock"),
            &roots
        ));
        // A vault that itself lives under a dot directory is still watchable.
        let hidden_root = dir.path().join(".hidden/notes");
        let hidden_file = write(dir.path(), ".hidden/notes/n-BBBB.md", "x");
        assert!(interesting(
            &EventKind::Create(CreateKind::File),
            &hidden_file,
            &[hidden_root]
        ));
    }

    #[test]
    fn the_indexed_notice_is_the_documented_one() {
        assert_eq!(
            INDEXED_NOTICE,
            "watch: indexed unavailable — watching for reconcile only"
        );
    }
}
