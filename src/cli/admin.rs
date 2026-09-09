//! `init`, `status`, `reindex`, `config`, `completions` and the hidden `daemon` shim.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value as Json};

use crate::cli::out;
use crate::cli::{CompletionsArgs, ConfigSub, DaemonSub, InitArgs, ReindexArgs, StatusArgs};
use crate::config::{self, Config, InitOptions};
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};
use crate::spaces::{expand_user, Space};

/// The notice `status` prints when the config still carries a `[daemon]` table.
pub const DAEMON_TABLE_NOTICE: &str =
    "config: [daemon] is ignored — the daemon was removed; see 'mesh watch'";
/// The notice `reindex` prints when `indexed` is missing or fails.
pub const REINDEX_NOTICE: &str = "search index unavailable (indexed binary missing or failed)";
/// The notice the `daemon start` shim prints before answering.
pub const DAEMON_SHIM_NOTICE: &str = "daemon: removed — use 'mesh watch'";

/// `~/.mesh/vault` — what `init` writes when `--path` is absent.
fn default_vault_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mesh")
        .join("vault")
}

/// Trimmed CSV, empties dropped, duplicates dropped — the one CSV split every flag uses.
fn parse_csv(value: Option<&str>) -> Vec<String> {
    crate::domain::select::parse_csv(value.unwrap_or(""))
}

// --------------------------------------------------------------------------------------------
// init
// --------------------------------------------------------------------------------------------

/// `mesh init` — write the config file and create `vault_path`.
pub fn init(ctx: &mut Ctx, args: InitArgs) -> Result<()> {
    let cfg_path = config::resolve_config_path(ctx.g.config.as_deref());
    // Refuse first: on this branch the file is never opened for writing.
    if cfg_path.is_file() && !args.force {
        return Err(MeshError::Validation(format!(
            "config already exists at {} — pass --force to overwrite",
            cfg_path.display()
        )));
    }
    // Validate `--engine` before touching the filesystem.
    crate::search::Engine::parse(&args.engine)?;

    let vault_path = match args.path.as_deref() {
        Some(raw) => expand_user(raw),
        None => default_vault_path(),
    };
    let agent = args
        .agent
        .filter(|a| !a.is_empty())
        .or_else(|| {
            std::env::var(config::ENV_AGENT)
                .ok()
                .filter(|a| !a.is_empty())
        })
        .unwrap_or_else(|| "agent".to_string());

    let opts = InitOptions {
        vault_path: vault_path.clone(),
        agent: agent.clone(),
        collections: parse_csv(args.collections.as_deref()),
        search_collection: args.search_collection.filter(|c| !c.is_empty()),
        hybrid: !args.no_hybrid,
        threshold: args.threshold,
        engine: args.engine,
        spaces: !args.no_spaces,
        obsidian: args.obsidian,
    };

    std::fs::create_dir_all(&vault_path)?;
    crate::storage::atomic_write(&cfg_path, &config::render_config_toml(&opts))?;

    // Class L with a twist: `--json` wins, `--quiet` prints nothing at all.
    if ctx.g.json {
        let mut payload = Map::new();
        payload.insert(
            "path".to_string(),
            Json::String(cfg_path.display().to_string()),
        );
        payload.insert(
            "vault_path".to_string(),
            Json::String(vault_path.display().to_string()),
        );
        payload.insert("agent".to_string(), Json::String(agent));
        out::line(out::json_line(&Json::Object(payload)).trim_end());
    } else if !ctx.g.quiet {
        out::line(&format!("wrote config to {}", cfg_path.display()));
    }
    Ok(())
}

// --------------------------------------------------------------------------------------------
// status
// --------------------------------------------------------------------------------------------

/// Whether the raw config file still declares a `[daemon]` table.
fn config_has_daemon_table(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    text.parse::<toml::Table>()
        .map(|t| t.contains_key("daemon"))
        .unwrap_or(false)
}

fn as_i64(value: Option<&Json>) -> Option<i64> {
    value.and_then(Json::as_i64)
}

fn scalar_text(value: &Json) -> String {
    match value {
        Json::String(s) => s.clone(),
        Json::Null => String::new(),
        Json::Array(items) => items
            .iter()
            .map(scalar_text)
            .collect::<Vec<String>>()
            .join(", "),
        other => other.to_string(),
    }
}

/// `label: k=v k=v` over the keys that are present; `None` when none of them are.
fn count_line(label: &str, value: &Json, keys: &[&str]) -> Option<String> {
    let obj = value.as_object()?;
    let parts: Vec<String> = keys
        .iter()
        .filter_map(|k| {
            obj.get(*k).map(|v| {
                let text = match v {
                    Json::Array(items) => items.len().to_string(),
                    other => scalar_text(other),
                };
                format!("{k}={text}")
            })
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(format!("{label}: {}", parts.join(" ")))
}

fn liveness_line(label: &str, value: &Json) -> String {
    let running = value
        .get("running")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let pid = as_i64(value.get("pid"));
    match (running, pid) {
        (true, Some(pid)) => format!("{label}: running (pid {pid})"),
        (true, None) => format!("{label}: running"),
        _ => format!("{label}: stopped"),
    }
}

/// The human `status` block: one line per group the payload actually carries.
pub fn status_block(report: &Json) -> String {
    let mut lines: Vec<String> = Vec::new();
    let Some(obj) = report.as_object() else {
        return String::new();
    };

    if let Some(vault) = obj.get("vault") {
        let path = vault.get("path").and_then(Json::as_str).unwrap_or("");
        let exists = vault.get("exists").and_then(Json::as_bool).unwrap_or(true);
        let suffix = if exists { "" } else { " (does not exist)" };
        lines.push(format!("vault: {path}{suffix}"));
    }
    if let Some(notes) = obj.get("notes") {
        lines.push(format!("notes: {}", scalar_text(notes)));
    }
    if let Some(line) = obj
        .get("tasks")
        .and_then(|t| count_line("tasks", t, &crate::model::task::TASK_STATUSES))
    {
        lines.push(line);
    }
    if let Some(freshness) = obj.get("freshness") {
        let age = freshness.get("age_seconds").and_then(Json::as_f64);
        lines.push(match age {
            Some(age) => format!("freshness: {age:.1}s ago"),
            None => "freshness: (no vault files)".to_string(),
        });
    }
    if let Some(dangling) = obj.get("dangling_links") {
        let listed: Vec<String> = dangling
            .as_array()
            .map(|items| items.iter().map(scalar_text).collect())
            .unwrap_or_default();
        let total = as_i64(obj.get("dangling_links_total"))
            .unwrap_or_else(|| i64::try_from(listed.len()).unwrap_or(i64::MAX));
        let detail = if listed.is_empty() {
            String::new()
        } else {
            format!(" ({})", listed.join(", "))
        };
        lines.push(format!("dangling links: {total}{detail}"));
    }
    if let Some(locks) = obj.get("stale_locks") {
        let count = locks.as_array().map_or(0, Vec::len);
        lines.push(format!("stale locks: {count}"));
    }
    if let Some(daemon) = obj.get("daemon") {
        lines.push(liveness_line("daemon", daemon));
    }
    if let Some(agents) = obj.get("agents").and_then(Json::as_object) {
        if agents.is_empty() {
            lines.push("agents: (none)".to_string());
        } else {
            lines.push("agents:".to_string());
            let mut names: Vec<&String> = agents.keys().collect();
            names.sort();
            for name in names {
                let row = agents.get(name).cloned().unwrap_or(Json::Null);
                let open = as_i64(row.get("owns_open")).unwrap_or(0);
                let claimed = as_i64(row.get("claimed")).unwrap_or(0);
                let stale = as_i64(row.get("stale_claims")).unwrap_or(0);
                lines.push(format!(
                    "  {name}: open={open} claimed={claimed} stale={stale}"
                ));
            }
        }
    }
    for (key, fields) in [
        ("memories", &["total", "expired", "superseded"][..]),
        ("scratch", &["files", "agents"][..]),
        ("assets", &["count", "bytes", "orphan_blobs"][..]),
        (
            "deps",
            &["blocked", "ready", "cycles", "dangling_blockers"][..],
        ),
    ] {
        if let Some(line) = obj.get(key).and_then(|v| count_line(key, v, fields)) {
            lines.push(line);
        }
    }
    if let Some(spaces) = obj.get("spaces").and_then(Json::as_object) {
        lines.push("spaces:".to_string());
        for (name, value) in spaces {
            let path = value.get("path").and_then(Json::as_str).unwrap_or("");
            let exists = value.get("exists").and_then(Json::as_bool).unwrap_or(true);
            let suffix = if exists { "" } else { " (does not exist)" };
            lines.push(format!("  {name}: {path}{suffix}"));
        }
    }
    if let Some(watcher) = obj.get("watcher") {
        lines.push(liveness_line("watcher", watcher));
    }
    lines.join("\n")
}

/// `mesh status` — the read-only vault report. Exit 0 even with a missing vault.
pub fn status(ctx: &mut Ctx, _args: StatusArgs) -> Result<()> {
    let cfg_path = config::resolve_config_path(ctx.g.config.as_deref());
    let cfg = ctx.cfg()?;
    let report = crate::domain::lenses::status_report(cfg);
    if config_has_daemon_table(&cfg_path) {
        out::notice(ctx, DAEMON_TABLE_NOTICE);
    }
    out::object(ctx, &report, status_block);
    Ok(())
}

// --------------------------------------------------------------------------------------------
// reindex
// --------------------------------------------------------------------------------------------

/// Parse a `--space` CSV into space roots, rejecting unknown names.
fn space_roots(cfg: &Config, csv: Option<&str>) -> Result<Vec<PathBuf>> {
    let names = parse_csv(csv);
    if names.is_empty() {
        return Ok(vec![cfg.vault().to_path_buf()]);
    }
    let mut roots: Vec<PathBuf> = Vec::new();
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

/// `mesh reindex` — hand the vault (or the named space roots) to `indexed`. Always exit 0.
pub fn reindex(ctx: &mut Ctx, args: ReindexArgs) -> Result<()> {
    let cfg = ctx.cfg()?;
    let roots = space_roots(cfg, args.space.as_deref())?;
    // No collection configured: a silent no-op, not even a notice.
    if cfg.search.collection.is_none() {
        return Ok(());
    }
    // `search::reindex` is infallible by design, so `.is_err()` was statically false and
    // this notice never fired: an operator got no signal at all that the rebuild failed.
    // Read the outcome the rebuild actually produced.
    if crate::search::reindex_status(cfg, &roots).degraded() {
        out::notice(ctx, REINDEX_NOTICE);
    }
    Ok(())
}

// --------------------------------------------------------------------------------------------
// config
// --------------------------------------------------------------------------------------------

fn toml_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn toml_scalar(value: &Json) -> Option<String> {
    match value {
        Json::Null | Json::Object(_) => None,
        Json::String(s) => Some(toml_string(s)),
        Json::Bool(b) => Some(b.to_string()),
        Json::Number(n) => Some(n.to_string()),
        Json::Array(items) => {
            let rendered: Vec<String> = items.iter().filter_map(toml_scalar).collect();
            Some(format!("[{}]", rendered.join(", ")))
        }
    }
}

/// Render the effective-config object as TOML: bare keys first, then one table per object.
pub fn render_effective_toml(value: &Json) -> String {
    let mut lines: Vec<String> = Vec::new();
    let Some(obj) = value.as_object() else {
        return String::new();
    };
    for (key, item) in obj {
        if let Some(rendered) = toml_scalar(item) {
            lines.push(format!("{key} = {rendered}"));
        }
    }
    for (key, item) in obj {
        let Some(table) = item.as_object() else {
            continue;
        };
        lines.push(String::new());
        lines.push(format!("[{key}]"));
        for (name, entry) in table {
            if let Some(rendered) = toml_scalar(entry) {
                lines.push(format!("{name} = {rendered}"));
            }
        }
    }
    lines.join("\n")
}

/// The effective config: env overlay applied, every space resolved, the sandbox listed.
pub fn effective_config(cfg: &Config, cfg_path: &Path) -> Json {
    let mut root = Map::new();
    root.insert(
        "config_path".to_string(),
        Json::String(cfg_path.display().to_string()),
    );
    root.insert(
        "sandbox".to_string(),
        Json::Array(
            cfg.spaces
                .sandbox()
                .iter()
                .map(|p| Json::String(p.display().to_string()))
                .collect(),
        ),
    );

    let mut core = Map::new();
    core.insert(
        "vault_path".to_string(),
        Json::String(cfg.vault().display().to_string()),
    );
    core.insert(
        "agent".to_string(),
        cfg.agent()
            .map_or(Json::Null, |a| Json::String(a.to_string())),
    );
    root.insert("core".to_string(), Json::Object(core));

    let mut spaces = Map::new();
    for space in Space::ALL {
        let value = cfg.spaces.root(space).map_or(Json::Bool(false), |path| {
            Json::String(path.display().to_string())
        });
        spaces.insert(space.name().to_string(), value);
    }
    root.insert("spaces".to_string(), Json::Object(spaces));

    let mut search = Map::new();
    search.insert(
        "collection".to_string(),
        cfg.search
            .collection
            .as_deref()
            .map_or(Json::Null, |c| Json::String(c.to_string())),
    );
    search.insert("hybrid".to_string(), Json::Bool(cfg.search.hybrid));
    search.insert("threshold".to_string(), Json::from(cfg.search.threshold));
    search.insert(
        "threshold_explicit".to_string(),
        Json::Bool(cfg.search.threshold_explicit),
    );
    search.insert(
        "engine".to_string(),
        Json::String(cfg.search.engine.clone()),
    );
    search.insert(
        "spaces".to_string(),
        Json::Array(
            cfg.search
                .spaces
                .iter()
                .map(|s| Json::String(s.clone()))
                .collect(),
        ),
    );
    root.insert("search".to_string(), Json::Object(search));

    let mut tasks = Map::new();
    tasks.insert(
        "collections".to_string(),
        Json::Array(
            cfg.tasks
                .collections
                .iter()
                .map(|c| Json::String(c.clone()))
                .collect(),
        ),
    );
    tasks.insert("strict".to_string(), Json::Bool(cfg.tasks.strict));
    root.insert("tasks".to_string(), Json::Object(tasks));

    Json::Object(root)
}

/// Look one dotted key up in the effective-config object.
fn lookup(value: &Json, key: &str) -> Option<Json> {
    let mut current = value;
    for segment in key.split('.') {
        current = current.as_object()?.get(segment)?;
    }
    Some(current.clone())
}

/// `raw` read as a TOML fragment, when it is one.
fn toml_fragment(raw: &str) -> Option<toml_edit::Value> {
    let doc = format!("x = {raw}")
        .parse::<toml_edit::DocumentMut>()
        .ok()?;
    doc.get("x").and_then(toml_edit::Item::as_value).cloned()
}

/// The truthy and falsy spellings `config set` accepts for a boolean key.
fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// A TOML *string* fragment unwrapped, else the raw argument stored verbatim.
fn quoted_or_raw(raw: &str) -> toml_edit::Value {
    toml_fragment(raw)
        .filter(toml_edit::Value::is_str)
        .unwrap_or_else(|| toml_edit::Value::from(raw))
}

fn wrong_type(key: &str, want: &str, raw: &str) -> MeshError {
    MeshError::Validation(format!("config set {key}: expected {want}, got '{raw}'"))
}

/// Parse a `config set` value **as the type its reader accepts**.
///
/// The kind comes from [`config::value_kind`], beside the readers themselves, so the writer
/// can never emit a TOML type `load_config` then discards — which used to exit 0 while the
/// setting stayed off.
fn parse_set_value(key: &str, kind: config::ValueKind, raw: &str) -> Result<toml_edit::Value> {
    use config::ValueKind;
    match kind {
        // A quoted TOML string unwraps; everything else is stored verbatim, so an agent
        // named `true` or `0x1F` stays a string instead of becoming a bool or an integer.
        ValueKind::Str => Ok(quoted_or_raw(raw)),
        ValueKind::Bool => parse_bool(raw)
            .map(toml_edit::Value::from)
            .ok_or_else(|| wrong_type(key, "a boolean (true/false)", raw)),
        ValueKind::Float => raw
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .map(toml_edit::Value::from)
            .ok_or_else(|| wrong_type(key, "a finite number", raw)),
        ValueKind::Strings => {
            // A TOML array wins when it is one; otherwise the CSV every other flag speaks.
            if let Some(found) = toml_fragment(raw) {
                if let Some(array) = found.as_array() {
                    if array.iter().all(toml_edit::Value::is_str) {
                        return Ok(found.clone());
                    }
                    return Err(wrong_type(key, "an array of strings", raw));
                }
            }
            let mut array = toml_edit::Array::new();
            for item in parse_csv(Some(raw)) {
                array.push(item);
            }
            Ok(toml_edit::Value::Array(array))
        }
        // `false` disables the space; `true` means the default folder; anything else is a path.
        ValueKind::SpacePath => match raw.trim() {
            "false" => Ok(toml_edit::Value::from(false)),
            "true" => Ok(toml_edit::Value::from(true)),
            other => Ok(quoted_or_raw(other)),
        },
    }
}

/// `mesh config path|show|get|set`.
pub fn config(ctx: &mut Ctx, sub: ConfigSub) -> Result<()> {
    let cfg_path = config::resolve_config_path(ctx.g.config.as_deref());
    match sub {
        // `config path` answers without a config file; every other form needs one.
        ConfigSub::Path => {
            let text = cfg_path.display().to_string();
            let payload = serde_json::json!({ "path": text.clone() });
            out::object(ctx, &payload, |_| text.clone());
            Ok(())
        }
        ConfigSub::Show { json } => {
            ctx.coalesce(json, false, None);
            let cfg = ctx.cfg()?;
            let payload = effective_config(cfg, &cfg_path);
            out::object(ctx, &payload, render_effective_toml);
            Ok(())
        }
        ConfigSub::Get { key } => {
            let cfg = ctx.cfg()?;
            let payload = effective_config(cfg, &cfg_path);
            let Some(found) = lookup(&payload, &key) else {
                return Err(MeshError::Validation(format!(
                    "unknown config key: '{key}'"
                )));
            };
            out::object(ctx, &found, scalar_text);
            Ok(())
        }
        ConfigSub::Set { key, value } => set_key(ctx, &cfg_path, &key, &value),
    }
}

/// `config set KEY VALUE` — an in-place `toml_edit` write; comments and ordering survive.
fn set_key(ctx: &mut Ctx, cfg_path: &Path, key: &str, value: &str) -> Result<()> {
    // A config must exist and load before it can be edited in place.
    ctx.cfg()?;
    let segments: Vec<&str> = key.split('.').filter(|s| !s.is_empty()).collect();
    let (Some(table), Some(leaf), 2) = (segments.first(), segments.get(1), segments.len()) else {
        return Err(MeshError::Validation(format!(
            "config set expects a dotted key like core.agent, got '{key}'"
        )));
    };
    // A key no reader consumes is a silent no-op, so it is an error, exactly as `config get`
    // treats one.
    let Some(kind) = config::value_kind(key) else {
        return Err(MeshError::Validation(format!(
            "unknown config key: '{key}'"
        )));
    };
    let text = std::fs::read_to_string(cfg_path)?;
    let mut doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| {
        MeshError::Validation(format!("invalid config at {}: {e}", cfg_path.display()))
    })?;
    let parsed = parse_set_value(key, kind, value)?;
    let rendered = parsed.to_string().trim().to_string();
    let entry = doc
        .entry(table)
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(target) = entry.as_table_like_mut() else {
        return Err(MeshError::Validation(format!(
            "[{table}] is not a table in {}",
            cfg_path.display()
        )));
    };
    target.insert(leaf, toml_edit::Item::Value(parsed));
    crate::storage::atomic_write(cfg_path, &doc.to_string())?;
    out::mutation_named(ctx, key, "set", &[("value", Json::String(rendered))]);
    Ok(())
}

// --------------------------------------------------------------------------------------------
// completions
// --------------------------------------------------------------------------------------------

/// `mesh completions <SHELL>` — the script on stdout, exit 0.
pub fn completions(_ctx: &mut Ctx, args: CompletionsArgs) -> Result<()> {
    let mut command = <crate::cli::Cli as clap::CommandFactory>::command();
    let mut buffer: Vec<u8> = Vec::new();
    clap_complete::generate(args.shell, &mut command, "mesh", &mut buffer);
    out::line(String::from_utf8_lossy(&buffer).trim_end());
    Ok(())
}

// --------------------------------------------------------------------------------------------
// the daemon shim (final.md §5.10, overrides.md O9)
// --------------------------------------------------------------------------------------------

/// `mesh daemon start|stop|status` — hidden, never spawns, answers from the watch lock.
pub fn daemon(ctx: &mut Ctx, sub: DaemonSub) -> Result<()> {
    let cfg = ctx.cfg()?;
    let socket = crate::cli::watch::watch_lock_path(cfg)
        .display()
        .to_string();
    let pid = crate::cli::watch::watcher_pid(cfg);
    let pid_json = pid.map_or(Json::Null, Json::from);
    let (payload, human) = match sub {
        DaemonSub::Start => {
            out::notice(ctx, DAEMON_SHIM_NOTICE);
            let human = pid.map_or_else(
                || "daemon not running".to_string(),
                |p| format!("daemon already running (pid {p})"),
            );
            let body =
                serde_json::json!({"running": pid.is_some(), "started": false, "pid": pid_json});
            (body, human)
        }
        DaemonSub::Stop => {
            let stopped = pid.is_some_and(terminate);
            let mut body = Map::new();
            body.insert("running".to_string(), Json::Bool(false));
            body.insert("stopped".to_string(), Json::Bool(stopped));
            if let Some(p) = pid {
                body.insert("pid".to_string(), Json::from(p));
            }
            let human = match pid {
                Some(p) if stopped => format!("daemon stopped (pid {p})"),
                _ => "daemon not running".to_string(),
            };
            (Json::Object(body), human)
        }
        DaemonSub::Status => {
            let human = pid.map_or_else(
                || format!("stopped — socket {socket}"),
                |p| format!("running (pid {p}) — socket {socket}"),
            );
            let body =
                serde_json::json!({"running": pid.is_some(), "pid": pid_json, "socket": socket});
            (body, human)
        }
    };
    out::object(ctx, &payload, |_| human.clone());
    Ok(())
}

/// SIGTERM a watcher. A process that vanished first is not an error.
fn terminate(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    let Some(target) = rustix::process::Pid::from_raw(raw) else {
        return false;
    };
    rustix::process::kill_process(target, rustix::process::Signal::TERM).is_ok()
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

    #[test]
    fn csv_is_trimmed_and_compacted() {
        assert_eq!(parse_csv(Some(" a , b ,, c ")), ["a", "b", "c"]);
        assert!(parse_csv(None).is_empty());
        assert!(parse_csv(Some("  ")).is_empty());
    }

    #[test]
    fn the_status_block_only_prints_the_groups_it_has() {
        let report = serde_json::json!({
            "notes": 2,
            "tasks": {"open": 1, "claimed": 0, "done": 0, "cancelled": 0},
            "vault": {"path": "/v", "exists": false},
            "agents": {},
        });
        let block = status_block(&report);
        assert_eq!(
            block,
            "vault: /v (does not exist)\nnotes: 2\n\
             tasks: open=1 claimed=0 done=0 cancelled=0\nagents: (none)"
        );
        assert!(!block.contains("freshness"));
    }

    #[test]
    fn the_status_block_renders_every_documented_group() {
        let report = serde_json::json!({
            "notes": 1,
            "tasks": {"open": 0, "claimed": 1, "done": 0, "cancelled": 0},
            "tasks_total": 1,
            "freshness": {"mtime": 1.0, "age_seconds": 0.25},
            "dangling_links": ["Ghost"],
            "stale_locks": ["/v/tasks/.locks/t-x.lock"],
            "vault": {"path": "/v", "exists": true},
            "daemon": {"running": false, "pid": null},
            "agents": {"bob": {"owns_open": 0, "claimed": 1, "stale_claims": 0},
                       "alice": {"owns_open": 2, "claimed": 0, "stale_claims": 0}},
            "dangling_links_total": 3,
            "memories": {"total": 4, "expired": 1, "superseded": 0},
            "scratch": {"files": 2, "agents": 1},
            "assets": {"count": 1, "bytes": 10, "orphan_blobs": 0},
            "deps": {"blocked": 1, "ready": 2, "cycles": [["t-a", "t-b"]], "dangling_blockers": 0},
            "spaces": {"notes": {"path": "/v/notes", "exists": true}},
            "watcher": {"running": true, "pid": 42},
        });
        let block = status_block(&report);
        let lines: Vec<&str> = block.lines().collect();
        assert_eq!(lines[0], "vault: /v");
        assert_eq!(lines[1], "notes: 1");
        assert_eq!(lines[2], "tasks: open=0 claimed=1 done=0 cancelled=0");
        assert_eq!(lines[3], "freshness: 0.2s ago");
        assert_eq!(lines[4], "dangling links: 3 (Ghost)");
        assert_eq!(lines[5], "stale locks: 1");
        assert_eq!(lines[6], "daemon: stopped");
        assert_eq!(lines[7], "agents:");
        assert_eq!(lines[8], "  alice: open=2 claimed=0 stale=0");
        assert_eq!(lines[9], "  bob: open=0 claimed=1 stale=0");
        assert_eq!(lines[10], "memories: total=4 expired=1 superseded=0");
        assert_eq!(lines[11], "scratch: files=2 agents=1");
        assert_eq!(lines[12], "assets: count=1 bytes=10 orphan_blobs=0");
        assert_eq!(
            lines[13],
            "deps: blocked=1 ready=2 cycles=1 dangling_blockers=0"
        );
        assert_eq!(lines[14], "spaces:");
        assert_eq!(lines[15], "  notes: /v/notes");
        assert_eq!(lines[16], "watcher: running (pid 42)");
    }

    #[test]
    fn a_missing_vault_and_no_files_read_as_documented() {
        let report = serde_json::json!({
            "freshness": {"mtime": null, "age_seconds": null},
            "vault": {"path": "/gone", "exists": false},
            "dangling_links": [],
            "stale_locks": [],
        });
        let block = status_block(&report);
        assert!(block.contains("vault: /gone (does not exist)"));
        assert!(block.contains("freshness: (no vault files)"));
        assert!(block.contains("dangling links: 0"));
        assert!(!block.contains("dangling links: 0 ("));
    }

    #[test]
    fn an_empty_report_renders_an_empty_block() {
        assert_eq!(status_block(&serde_json::json!({})), "");
        assert_eq!(status_block(&Json::Null), "");
    }

    #[test]
    fn the_effective_config_carries_spaces_and_the_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let payload = effective_config(&cfg, Path::new("/tmp/config.toml"));
        let keys: Vec<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "config_path",
                "sandbox",
                "core",
                "spaces",
                "search",
                "tasks"
            ]
        );
        assert_eq!(payload["core"]["agent"], Json::String("test-agent".into()));
        assert!(payload["spaces"]["notes"]
            .as_str()
            .unwrap()
            .ends_with("notes"));
        assert_eq!(payload["sandbox"].as_array().unwrap().len(), 5);
        assert_eq!(payload["search"]["collection"], Json::Null);
        assert_eq!(payload["search"]["threshold_explicit"], Json::Bool(false));
    }

    #[test]
    fn the_effective_toml_puts_bare_keys_before_tables() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let text = render_effective_toml(&effective_config(&cfg, Path::new("/c.toml")));
        let first_table = text.find("[core]").unwrap();
        assert!(text.find("config_path = ").unwrap() < first_table);
        assert!(text.find("sandbox = [").unwrap() < first_table);
        // Null scalars are omitted, never rendered as `null`.
        assert!(!text.contains("null"));
        let parsed: toml::Table = text.parse().unwrap();
        assert!(parsed.contains_key("search"));
        assert!(parsed.contains_key("tasks"));
    }

    #[test]
    fn a_disabled_space_renders_as_false() {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[core]\nvault_path = \"{}\"\n[spaces]\nscratch = false\n",
            dir.path().display()
        );
        let table: toml::Table = text.parse().unwrap();
        let cfg = config::from_table(&table, None).unwrap();
        let payload = effective_config(&cfg, Path::new("/c.toml"));
        assert_eq!(payload["spaces"]["scratch"], Json::Bool(false));
        assert!(render_effective_toml(&payload).contains("scratch = false"));
    }

    #[test]
    fn dotted_lookup_walks_the_object() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let payload = effective_config(&cfg, Path::new("/c.toml"));
        assert_eq!(
            lookup(&payload, "core.agent"),
            Some(Json::String("test-agent".into()))
        );
        assert_eq!(lookup(&payload, "search.hybrid"), Some(Json::Bool(true)));
        assert!(lookup(&payload, "sandbox").is_some());
        assert!(lookup(&payload, "core.nope").is_none());
        assert!(lookup(&payload, "nope").is_none());
    }

    #[test]
    fn set_values_parse_as_the_type_their_reader_accepts() {
        use crate::config::ValueKind;
        let parse = parse_set_value;

        // A string key never re-parses as another TOML type: these all used to become a
        // bool / an integer / an array that `table_str` then dropped on the floor.
        for raw in ["true", "0", "7", "bob", "0x1F", "[1]"] {
            assert_eq!(
                parse("core.agent", ValueKind::Str, raw).unwrap().as_str(),
                Some(raw),
                "core.agent = {raw}"
            );
        }
        // A quoted TOML string still unwraps, and a bare path stays a path.
        assert_eq!(
            parse("core.agent", ValueKind::Str, "\"quoted\"")
                .unwrap()
                .as_str(),
            Some("quoted")
        );
        assert_eq!(
            parse("core.vault_path", ValueKind::Str, "/tmp/vault")
                .unwrap()
                .as_str(),
            Some("/tmp/vault")
        );

        // Booleans take every spelling an operator types, and nothing else.
        for (raw, want) in [
            ("true", true),
            ("1", true),
            ("yes", true),
            ("on", true),
            ("false", false),
            ("0", false),
            ("no", false),
            ("off", false),
        ] {
            assert_eq!(
                parse("tasks.strict", ValueKind::Bool, raw)
                    .unwrap()
                    .as_bool(),
                Some(want),
                "tasks.strict = {raw}"
            );
        }
        let err = parse("tasks.strict", ValueKind::Bool, "maybe").unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            "config set tasks.strict: expected a boolean (true/false), got 'maybe'"
        );

        // Floats widen an integer and refuse a non-finite score floor.
        assert_eq!(
            parse("search.threshold", ValueKind::Float, "0.4")
                .unwrap()
                .as_float(),
            Some(0.4)
        );
        assert_eq!(
            parse("search.threshold", ValueKind::Float, "1")
                .unwrap()
                .as_float(),
            Some(1.0)
        );
        for raw in ["nan", "inf", "high"] {
            assert_eq!(
                parse("search.threshold", ValueKind::Float, raw)
                    .unwrap_err()
                    .code(),
                2,
                "search.threshold = {raw}"
            );
        }

        // A roster takes the CSV `mesh init --collections` documents, and a TOML array.
        let strings = |raw: &str| -> Vec<String> {
            parse("tasks.collections", ValueKind::Strings, raw)
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(strings("alice,bob"), ["alice", "bob"]);
        assert_eq!(strings(" alice , bob ,, alice "), ["alice", "bob"]);
        assert_eq!(strings("[\"a\", \"b\"]"), ["a", "b"]);
        assert!(strings("").is_empty());
        assert_eq!(
            parse("tasks.collections", ValueKind::Strings, "[1, 2]")
                .unwrap_err()
                .code(),
            2
        );

        // A space takes a path or `false`; `true` means the default folder.
        assert_eq!(
            parse("spaces.notes", ValueKind::SpacePath, "false")
                .unwrap()
                .as_bool(),
            Some(false)
        );
        assert_eq!(
            parse("spaces.notes", ValueKind::SpacePath, ".")
                .unwrap()
                .as_str(),
            Some(".")
        );
    }

    #[test]
    fn space_roots_default_to_the_vault_and_reject_nonsense() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        assert_eq!(
            space_roots(&cfg, None).unwrap(),
            vec![cfg.vault().to_path_buf()]
        );
        let roots = space_roots(&cfg, Some("notes, tasks , notes")).unwrap();
        assert_eq!(roots.len(), 2);
        let err = space_roots(&cfg, Some("nope")).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            "unknown space: 'nope' (use notes, tasks, memories, scratch, assets)"
        );
    }

    #[test]
    fn a_disabled_space_is_a_validation_error_for_reindex() {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[core]\nvault_path = \"{}\"\n[spaces]\nscratch = false\n",
            dir.path().display()
        );
        let table: toml::Table = text.parse().unwrap();
        let cfg = config::from_table(&table, None).unwrap();
        let err = space_roots(&cfg, Some("scratch")).unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "space 'scratch' is disabled in [spaces]");
    }

    #[test]
    fn a_daemon_table_is_detected_in_the_raw_file() {
        let dir = tempfile::tempdir().unwrap();
        let with = dir.path().join("with.toml");
        std::fs::write(
            &with,
            "[core]\nvault_path = \"/v\"\n[daemon]\nsocket = \"x\"\n",
        )
        .unwrap();
        assert!(config_has_daemon_table(&with));
        let without = dir.path().join("without.toml");
        std::fs::write(&without, "[core]\nvault_path = \"/v\"\n").unwrap();
        assert!(!config_has_daemon_table(&without));
        assert!(!config_has_daemon_table(&dir.path().join("missing.toml")));
    }

    #[test]
    fn the_notice_strings_are_the_documented_ones() {
        assert_eq!(
            DAEMON_TABLE_NOTICE,
            "config: [daemon] is ignored — the daemon was removed; see 'mesh watch'"
        );
        assert_eq!(
            REINDEX_NOTICE,
            "search index unavailable (indexed binary missing or failed)"
        );
        assert_eq!(DAEMON_SHIM_NOTICE, "daemon: removed — use 'mesh watch'");
    }
}
