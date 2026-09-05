//! `mesh memory …` — the seven memory subcommands and their output branches.

use std::path::PathBuf;

use serde_json::{Map, Value as Json};

use crate::cli::out;
use crate::cli::MemorySub;
use crate::ctx::Ctx;
use crate::domain::memories::{
    self, ListMemoryOpts, NewMemory, RecallOpts, UpdateMemory, EXPIRES_NONE,
};
use crate::domain::select::parse_csv;
use crate::domain::{AppendOpts, Filter, SortKey};
use crate::error::{MeshError, Result};
use crate::model::memory::{Memory, MEMORY_FIELDS};
use crate::render;
use crate::search;
use crate::spaces::Space;
use crate::text::preview;
use crate::timefmt::{now_utc, parse_since};

/// The sort keys `memory list` accepts, in the order the rejection message names them.
const MEMORY_SORTS: [SortKey; 4] = [
    SortKey::Updated,
    SortKey::Created,
    SortKey::Title,
    SortKey::Importance,
];

/// Run one `memory` subcommand.
pub fn run(ctx: &mut Ctx, sub: MemorySub) -> Result<()> {
    match sub {
        MemorySub::New {
            title,
            kind,
            scope,
            importance,
            source,
            expires,
            supersedes,
            tags,
            owner,
            body,
            file,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, owner);
            new(
                ctx,
                &title,
                NewFlags {
                    kind,
                    scope,
                    importance,
                    source,
                    expires,
                    supersedes,
                    tags,
                    body,
                    file,
                },
            )
        }
        MemorySub::Append {
            target,
            text,
            section,
            timestamp,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, None);
            append(ctx, &target, &text, section, timestamp)
        }
        MemorySub::Update {
            target,
            tags,
            title,
            kind,
            scope,
            importance,
            source,
            expires,
            owner,
            out,
        } => {
            // The global `--owner` is deliberately not folded into the reassignment `--owner`.
            ctx.coalesce(out.json, out.quiet, None);
            update(
                ctx,
                &target,
                UpdateFlags {
                    tags,
                    title,
                    kind,
                    scope,
                    importance,
                    source,
                    expires,
                    owner,
                },
            )
        }
        MemorySub::Get {
            target,
            full,
            meta_only,
            related,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, None);
            get(ctx, &target, full, meta_only, related)
        }
        MemorySub::List {
            kind,
            scope,
            tags,
            any_tag,
            owner,
            mine,
            min_importance,
            since,
            include_expired,
            include_superseded,
            sort,
            limit,
            out,
        } => {
            ctx.coalesce_mine(mine);
            ctx.coalesce(out.json, out.quiet, owner);
            list(
                ctx,
                ListFlags {
                    kind,
                    scope,
                    tags,
                    any_tag,
                    min_importance,
                    since,
                    include_expired,
                    include_superseded,
                    sort,
                    limit,
                },
            )
        }
        MemorySub::Recall {
            query,
            kind,
            tags,
            owner,
            mine,
            min_importance,
            limit,
            threshold,
            no_decay,
            include_expired,
            meta_only,
            full,
            out,
        } => {
            ctx.coalesce_mine(mine);
            ctx.coalesce(out.json, out.quiet, owner);
            recall(
                ctx,
                &query,
                RecallFlags {
                    kind,
                    tags,
                    min_importance,
                    limit,
                    threshold,
                    no_decay,
                    include_expired,
                    meta_only,
                    full,
                },
            )
        }
        MemorySub::Forget {
            target,
            force,
            expired,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, None);
            forget(ctx, &target, force, expired)
        }
    }
}

// ---------------------------------------------------------------------------------------
// new
// ---------------------------------------------------------------------------------------

/// The `memory new` flags, grouped so the verb takes one argument.
struct NewFlags {
    kind: String,
    scope: String,
    importance: i64,
    source: Option<String>,
    expires: Option<String>,
    supersedes: Option<String>,
    tags: Option<String>,
    body: Option<String>,
    file: Option<PathBuf>,
}

/// `--body` beats `--file` beats `$EDITOR` on a tty; a headless path with none is exit 2.
fn resolve_body(ctx: &Ctx, body: Option<String>, file: Option<PathBuf>) -> Result<String> {
    if let Some(text) = body {
        return Ok(text);
    }
    if let Some(path) = file {
        return std::fs::read_to_string(&path).map_err(|e| {
            MeshError::Validation(format!("cannot read --file {}: {e}", path.display()))
        });
    }
    if ctx.is_machine() || !ctx.tty {
        return Err(MeshError::Validation(
            "no body: pass --body or --file on a non-interactive path".to_string(),
        ));
    }
    edit_body()
}

/// Open `$VISUAL` / `$EDITOR` (else `vi`) on a temp file and read back what it left.
fn edit_body() -> Result<String> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".to_string());
    let handle = tempfile::Builder::new().suffix(".md").tempfile()?;
    let path = handle.path().to_path_buf();
    let mut words = editor.split_whitespace();
    let Some(program) = words.next() else {
        return Ok(String::new());
    };
    let mut command = std::process::Command::new(program);
    for word in words {
        command.arg(word);
    }
    command.arg(&path);
    let _ = command.status()?;
    Ok(std::fs::read_to_string(&path).unwrap_or_default())
}

fn new(ctx: &mut Ctx, title: &str, flags: NewFlags) -> Result<()> {
    let body = resolve_body(ctx, flags.body, flags.file)?;
    let expires = match flags.expires.as_deref() {
        Some(value) => Some(memories::parse_expires(value)?),
        None => None,
    };
    let owner = ctx.g.owner.clone();
    let cfg = ctx.cfg()?;
    let duplicate = memories::find_duplicate_title(cfg, title);
    let (memory, warnings) = memories::create_with_warnings(
        cfg,
        title,
        NewMemory {
            kind: flags.kind,
            scope: flags.scope,
            importance: Some(flags.importance),
            source: flags.source,
            expires,
            supersedes: flags.supersedes,
            tags: flags.tags.as_deref().map(parse_csv).unwrap_or_default(),
            owner,
            body,
        },
    )?;
    if let Some(existing) = duplicate {
        out::notice(
            ctx,
            &format!("memory new: duplicate title, also used by {existing}"),
        );
    }
    for warning in &warnings {
        out::notice(ctx, warning);
    }
    report(ctx, &memory, "created");
    Ok(())
}

/// The class-M mutation line every writing memory verb ends with. The memory analogue of
/// notes' `{"type": …}` is `{"kind": …}`.
fn report(ctx: &Ctx, memory: &Memory, verb: &str) {
    out::mutation(
        ctx,
        &memory.id,
        verb,
        &[("kind", Json::String(memory.kind.clone()))],
        memory.updated.unwrap_or_else(now_utc),
    );
}

// ---------------------------------------------------------------------------------------
// append / update
// ---------------------------------------------------------------------------------------

fn append(
    ctx: &mut Ctx,
    target: &str,
    text: &str,
    section: Option<String>,
    timestamp: bool,
) -> Result<()> {
    let actor = ctx.actor().map(str::to_string);
    let cfg = ctx.cfg()?;
    let memory = memories::append(
        cfg,
        target,
        text,
        AppendOpts {
            section,
            timestamp,
            actor,
        },
    )?;
    report(ctx, &memory, "appended");
    Ok(())
}

/// The `memory update` flags, grouped so the verb takes one argument.
struct UpdateFlags {
    tags: Option<String>,
    title: Option<String>,
    kind: Option<String>,
    scope: Option<String>,
    importance: Option<i64>,
    source: Option<String>,
    expires: Option<String>,
    owner: Option<String>,
}

fn update(ctx: &mut Ctx, target: &str, flags: UpdateFlags) -> Result<()> {
    // `--expires none` clears the soft TTL; anything else is the `--since` grammar read
    // forwards, or an absolute ISO datetime.
    let expires = match flags.expires.as_deref().map(str::trim) {
        None => None,
        Some(value) if value.eq_ignore_ascii_case(EXPIRES_NONE) => Some(None),
        Some(value) => Some(Some(memories::parse_expires(value)?)),
    };
    let cfg = ctx.cfg()?;
    let memory = memories::update(
        cfg,
        target,
        UpdateMemory {
            tags: flags.tags,
            title: flags.title,
            kind: flags.kind,
            scope: flags.scope,
            importance: flags.importance,
            source: flags.source,
            expires,
            owner: flags.owner,
        },
    )?;
    report(ctx, &memory, "updated");
    Ok(())
}

// ---------------------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------------------

/// A scalar payload field as human text: a string verbatim, a number or bool stringified,
/// null and absent both empty.
fn scalar(entry: &Json, key: &str) -> String {
    match entry.get(key) {
        Some(Json::String(s)) => s.clone(),
        Some(Json::Number(n)) => n.to_string(),
        Some(Json::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn string_list(entry: &Json, key: &str) -> Vec<String> {
    entry
        .get(key)
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The fourteen-line human metadata block, rendered from the same JSON the payload uses so
/// the two surfaces cannot print a timestamp differently.
fn meta_block(entry: &Json) -> String {
    [
        format!("id: {}", scalar(entry, "id")),
        format!("type: {}", scalar(entry, "type")),
        format!("title: {}", scalar(entry, "title")),
        format!("kind: {}", scalar(entry, "kind")),
        format!("scope: {}", scalar(entry, "scope")),
        format!("importance: {}", scalar(entry, "importance")),
        format!("owner: {}", scalar(entry, "owner")),
        format!("source: {}", scalar(entry, "source")),
        format!("expires: {}", scalar(entry, "expires")),
        format!("superseded_by: {}", scalar(entry, "superseded_by")),
        format!("tags: {}", string_list(entry, "tags").join(", ")),
        format!("created: {}", scalar(entry, "created")),
        format!("updated: {}", scalar(entry, "updated")),
        format!("related: {}", string_list(entry, "related").join(", ")),
    ]
    .join("\n")
}

fn get(ctx: &mut Ctx, target: &str, full: bool, meta_only: bool, related: bool) -> Result<()> {
    let cfg = ctx.cfg()?;
    let view = memories::get(cfg, target)?;
    let mut entry = render::entry(&view.item.meta, MEMORY_FIELDS.fields(), None, None);
    let body = view.body;

    // `--quiet` returns the id and returns early — it beats `--related`.
    if ctx.g.quiet {
        out::line(&scalar(&entry, "id"));
        return Ok(());
    }
    if related {
        let ids = string_list(&entry, "related");
        let mut payload = Map::new();
        payload.insert(
            "related".to_string(),
            Json::Array(ids.iter().map(|i| Json::String(i.clone())).collect()),
        );
        out::object(ctx, &Json::Object(payload), |_| ids.join("\n"));
        return Ok(());
    }
    if !meta_only {
        if let Some(map) = entry.as_object_mut() {
            map.insert("body".to_string(), Json::String(body.clone()));
        }
    }
    out::object(ctx, &entry, |value| {
        if meta_only {
            meta_block(value)
        } else {
            format!("{}\n\n{}", meta_block(value), preview(&body, full))
        }
    });
    Ok(())
}

// ---------------------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------------------

/// The `memory list` flags, grouped so the verb takes one argument.
struct ListFlags {
    kind: Option<String>,
    scope: Option<String>,
    tags: Option<String>,
    any_tag: bool,
    min_importance: Option<i64>,
    since: Option<String>,
    include_expired: bool,
    include_superseded: bool,
    sort: String,
    limit: i64,
}

fn list(ctx: &mut Ctx, flags: ListFlags) -> Result<()> {
    let sort = SortKey::parse(&flags.sort, &MEMORY_SORTS)?;
    let cutoff = match flags.since.as_deref() {
        Some(value) => Some(parse_since(value)?),
        None => None,
    };
    let owner = ctx.g.owner.clone();
    let mine = ctx.g.mine;
    let me = ctx.actor().map(str::to_string);
    let filter = Filter {
        tags: flags
            .tags
            .as_deref()
            .map(parse_csv)
            .filter(|t| !t.is_empty()),
        any_tag: flags.any_tag,
        owner,
        mine,
        me,
        cutoff,
        sort,
        limit: Some(flags.limit),
        ..Filter::default()
    };
    let opts = ListMemoryOpts {
        kind: flags.kind,
        scope: flags.scope,
        min_importance: flags.min_importance,
        include_expired: flags.include_expired,
        include_superseded: flags.include_superseded,
    };

    let cfg = ctx.cfg()?;
    let entries: Vec<Json> = memories::list(cfg, &filter, &opts)?
        .iter()
        .map(|view| render::entry(&view.item.meta, MEMORY_FIELDS.fields(), None, None))
        .collect();
    out::rows(ctx, &entries, |entry| {
        format!(
            "{}  {}  {}",
            entry.get("id").and_then(Json::as_str).unwrap_or("-"),
            entry.get("kind").and_then(Json::as_str).unwrap_or("-"),
            entry.get("title").and_then(Json::as_str).unwrap_or(""),
        )
    });
    Ok(())
}

// ---------------------------------------------------------------------------------------
// recall
// ---------------------------------------------------------------------------------------

/// The `memory recall` flags, grouped so the verb takes one argument.
struct RecallFlags {
    kind: Option<String>,
    tags: Option<String>,
    min_importance: Option<i64>,
    limit: i64,
    threshold: Option<f64>,
    no_decay: bool,
    include_expired: bool,
    meta_only: bool,
    full: bool,
}

/// Class **S**: one compact JSON hit array on stdout, whatever `--json` / `--quiet` say.
fn recall(ctx: &mut Ctx, query: &str, flags: RecallFlags) -> Result<()> {
    let owner = ctx.g.owner.clone();
    let mine = ctx.g.mine;
    let me = ctx.actor().map(str::to_string);
    let filter = Filter {
        tags: flags
            .tags
            .as_deref()
            .map(parse_csv)
            .filter(|t| !t.is_empty()),
        owner,
        mine,
        me,
        limit: None,
        ..Filter::default()
    }
    .with_extra("kind", flags.kind.as_deref());
    let opts = RecallOpts {
        limit: flags.limit,
        threshold: flags.threshold,
        decay: !flags.no_decay,
        include_expired: flags.include_expired,
        min_importance: flags.min_importance,
        meta_only: flags.meta_only,
        full: flags.full,
    };

    let cfg = ctx.cfg()?;
    let hits = memories::recall(cfg, query, &filter, &opts)?;
    let space_key = search::emit_space_key(cfg, &[Space::Memories], false);
    let payload: Vec<Json> = hits
        .iter()
        .map(|hit| {
            render::hit(
                hit,
                flags.meta_only,
                flags.full,
                space_key.then_some(hit.space),
            )
        })
        .collect();
    out::line(&serde_json::to_string(&Json::Array(payload)).unwrap_or_else(|_| "[]".to_string()));
    Ok(())
}

// ---------------------------------------------------------------------------------------
// forget
// ---------------------------------------------------------------------------------------

fn deleted_payload(id: &str) -> Json {
    let mut payload = Map::new();
    payload.insert("id".to_string(), Json::String(id.to_string()));
    payload.insert("deleted".to_string(), Json::Bool(true));
    Json::Object(payload)
}

fn forget(ctx: &mut Ctx, target: &str, force: bool, expired: bool) -> Result<()> {
    if expired {
        return forget_expired(ctx, force);
    }
    if target.trim().is_empty() {
        return Err(MeshError::Validation(
            "no target: pass a memory id or --expired".to_string(),
        ));
    }
    let cfg = ctx.cfg()?;
    // Resolve first, so a bad target exits 3 before any prompt.
    let path = memories::resolve(cfg, target)?;
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| target.to_string());
    out::delete_guard(ctx, &id, force)?;
    let removed = memories::forget(cfg, &id)?;
    if ctx.g.quiet {
        out::line(&removed);
        return Ok(());
    }
    if ctx.g.json {
        out::line(out::json_line(&deleted_payload(&removed)).trim_end_matches('\n'));
        return Ok(());
    }
    out::line(&format!("deleted {removed}"));
    Ok(())
}

/// The bulk form: every expired memory, under the same delete guard, reported as an array.
fn forget_expired(ctx: &mut Ctx, force: bool) -> Result<()> {
    let cfg = ctx.cfg()?;
    let doomed = memories::expired_ids(cfg)?;
    out::delete_guard(
        ctx,
        &format!("{} expired memories", doomed.len()),
        force || doomed.is_empty(),
    )?;
    let removed = memories::forget_expired(cfg)?;
    if ctx.g.quiet {
        for id in &removed {
            out::line(id);
        }
        return Ok(());
    }
    if ctx.g.json {
        let payload: Vec<Json> = removed.iter().map(|id| deleted_payload(id)).collect();
        out::line(out::json_line(&Json::Array(payload)).trim_end_matches('\n'));
        return Ok(());
    }
    for id in &removed {
        out::line(&format!("deleted {id}"));
    }
    Ok(())
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

    fn entry() -> Json {
        serde_json::json!({
            "id": "m-CFCC",
            "type": "memory",
            "title": "Alpha Memory",
            "tags": ["a", "b"],
            "owner": "demo-agent",
            "created": "2026-09-05T07:27:18.265543Z",
            "updated": "2026-09-05T07:27:18.265543Z",
            "related": [],
            "kind": "fact",
            "scope": "shared",
            "importance": 3,
            "source": Json::Null,
            "expires": Json::Null,
            "superseded_by": Json::Null,
        })
    }

    #[test]
    fn the_meta_block_is_fourteen_lines_in_order() {
        let text = meta_block(&entry());
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 14);
        assert_eq!(lines[0], "id: m-CFCC");
        assert_eq!(lines[1], "type: memory");
        assert_eq!(lines[2], "title: Alpha Memory");
        assert_eq!(lines[3], "kind: fact");
        assert_eq!(lines[4], "scope: shared");
        assert_eq!(lines[5], "importance: 3");
        assert_eq!(lines[6], "owner: demo-agent");
        assert_eq!(lines[7], "source: ");
        assert_eq!(lines[8], "expires: ");
        assert_eq!(lines[9], "superseded_by: ");
        assert_eq!(lines[10], "tags: a, b");
        assert_eq!(lines[11], "created: 2026-09-05T07:27:18.265543Z");
        assert_eq!(lines[12], "updated: 2026-09-05T07:27:18.265543Z");
        // An empty related list keeps the trailing space.
        assert_eq!(lines[13], "related: ");
    }

    #[test]
    fn the_meta_block_names_the_documented_fourteen_keys() {
        let text = meta_block(&entry());
        let keys: Vec<&str> = text
            .split('\n')
            .filter_map(|line| line.split(':').next())
            .collect();
        assert_eq!(
            keys,
            [
                "id",
                "type",
                "title",
                "kind",
                "scope",
                "importance",
                "owner",
                "source",
                "expires",
                "superseded_by",
                "tags",
                "created",
                "updated",
                "related"
            ]
        );
    }

    #[test]
    fn a_null_owner_renders_as_an_empty_value() {
        let mut e = entry();
        e["owner"] = Json::Null;
        assert!(meta_block(&e).contains("\nowner: \n"));
    }

    #[test]
    fn the_sort_rejection_names_the_four_memory_keys() {
        let err = SortKey::parse("bogus", &MEMORY_SORTS).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid sort field: 'bogus' (use updated, created, title, importance)"
        );
    }

    #[test]
    fn the_deleted_payload_is_id_then_deleted() {
        let payload = deleted_payload("m-X");
        let keys: Vec<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["id", "deleted"]);
        assert_eq!(payload["deleted"], Json::Bool(true));
    }
}
