//! `mesh note …` — the six note subcommands and their output branches.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value as Json};

use crate::cli::out;
use crate::cli::NoteSub;
use crate::ctx::Ctx;
use crate::domain::notes::{self, AppendOpts, NewNote, UpdateNote};
use crate::domain::select::parse_csv;
use crate::domain::{Filter, SortKey};
use crate::error::{MeshError, Result};
use crate::model::note::{ForeignView, Note, NOTE_FIELDS};
use crate::render;
use crate::text::preview;
use crate::timefmt::{now_utc, parse_since};

/// The sort keys `note list` accepts, in the order the rejection message names them.
const NOTE_SORTS: [SortKey; 3] = [SortKey::Updated, SortKey::Created, SortKey::Title];

/// How many ids the dangling-backlink advisory names before it elides.
const MAX_ADVISORY_IDS: usize = 3;

/// Run one `note` subcommand.
pub fn run(ctx: &mut Ctx, sub: NoteSub) -> Result<()> {
    match sub {
        NoteSub::New {
            title,
            note_type,
            tags,
            owner,
            body,
            file,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, owner);
            new(ctx, &title, &note_type, tags.as_deref(), body, file)
        }
        NoteSub::Append {
            target,
            text,
            section,
            timestamp,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, None);
            append(ctx, &target, &text, section, timestamp)
        }
        NoteSub::Update {
            target,
            tags,
            new_type,
            title,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, None);
            update(ctx, &target, tags, new_type, title)
        }
        NoteSub::Get {
            target,
            full,
            meta_only,
            related,
            foreign,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, None);
            get(ctx, &target, full, meta_only, related, foreign)
        }
        NoteSub::List {
            tags,
            any_tag,
            owner,
            note_type,
            since,
            sort,
            limit,
            foreign,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, None);
            list(
                ctx,
                tags.as_deref(),
                any_tag,
                owner,
                note_type.as_deref(),
                since.as_deref(),
                &sort,
                limit,
                foreign,
            )
        }
        NoteSub::Delete { target, force, out } => {
            ctx.coalesce(out.json, out.quiet, None);
            delete(ctx, &target, force)
        }
    }
}

// ---------------------------------------------------------------------------------------
// new
// ---------------------------------------------------------------------------------------

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

fn new(
    ctx: &mut Ctx,
    title: &str,
    note_type: &str,
    tags: Option<&str>,
    body: Option<String>,
    file: Option<PathBuf>,
) -> Result<()> {
    let body = resolve_body(ctx, body, file)?;
    let owner = ctx.g.owner.clone();
    let cfg = ctx.cfg()?;
    let duplicate = notes::find_duplicate_title(cfg, title);
    let note = notes::create(
        cfg,
        title,
        NewNote {
            note_type: note_type.to_string(),
            tags: tags.map(parse_csv).unwrap_or_default(),
            owner,
            body,
        },
    )?;
    if let Some(existing) = duplicate {
        out::notice(
            ctx,
            &format!("note new: duplicate title, also used by {existing}"),
        );
    }
    report(ctx, &note, "created");
    Ok(())
}

/// The class-M mutation line every writing note verb ends with.
fn report(ctx: &Ctx, note: &Note, verb: &str) {
    out::mutation(
        ctx,
        &note.id,
        verb,
        &[("type", Json::String(note.note_type.clone()))],
        note.updated.unwrap_or_else(now_utc),
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
    let note = notes::append(
        cfg,
        target,
        text,
        AppendOpts {
            section,
            timestamp,
            actor,
        },
    )?;
    report(ctx, &note, "appended");
    Ok(())
}

/// `note update: renaming dangles {n} title link(s) in {id1}, {id2}, …`
fn dangling_advisory(ctx: &Ctx, ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    let named: Vec<&str> = ids
        .iter()
        .take(MAX_ADVISORY_IDS)
        .map(String::as_str)
        .collect();
    let mut listed = named.join(", ");
    if ids.len() > MAX_ADVISORY_IDS {
        listed.push_str(", …");
    }
    out::notice(
        ctx,
        &format!(
            "note update: renaming dangles {} title link(s) in {listed}",
            ids.len()
        ),
    );
}

fn update(
    ctx: &mut Ctx,
    target: &str,
    tags: Option<String>,
    new_type: Option<String>,
    title: Option<String>,
) -> Result<()> {
    let cfg = ctx.cfg()?;
    // Count title-form backlinks before the rename: a renamed note silently dangles every
    // `[[Old Title]]`, and mesh never rewrites a body it did not write.
    let mut dangled: Vec<String> = Vec::new();
    if let Some(new_title) = &title {
        if !ctx.g.quiet {
            let before = notes::get(cfg, target)?;
            if &before.item.title != new_title {
                dangled = crate::domain::backlinks_by_title(cfg, &before.item.title);
            }
        }
    }
    let note = notes::update(
        cfg,
        target,
        UpdateNote {
            tags,
            new_type,
            title,
        },
    )?;
    dangling_advisory(ctx, &dangled);
    report(ctx, &note, "updated");
    Ok(())
}

// ---------------------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------------------

/// A foreign file's payload: `id` and `type` null, the title derived, `path` last.
fn foreign_entry(view: &ForeignView, body: Option<&str>) -> Json {
    let mut out = Map::new();
    out.insert("id".to_string(), Json::Null);
    out.insert("type".to_string(), Json::Null);
    out.insert(
        "title".to_string(),
        view.title.clone().map_or(Json::Null, Json::String),
    );
    if let Some(text) = body {
        out.insert("body".to_string(), Json::String(text.to_string()));
    }
    out.insert(
        "path".to_string(),
        Json::String(view.path.display().to_string()),
    );
    Json::Object(out)
}

fn field(entry: &Json, key: &str) -> String {
    entry
        .get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string()
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

/// The eight-line human metadata block, rendered from the same JSON the payload uses so the
/// two surfaces cannot print a timestamp differently.
fn meta_block(entry: &Json) -> String {
    [
        format!("id: {}", field(entry, "id")),
        format!("type: {}", field(entry, "type")),
        format!("title: {}", field(entry, "title")),
        format!("tags: {}", string_list(entry, "tags").join(", ")),
        format!("owner: {}", field(entry, "owner")),
        format!("created: {}", field(entry, "created")),
        format!("updated: {}", field(entry, "updated")),
        format!("related: {}", string_list(entry, "related").join(", ")),
    ]
    .join("\n")
}

fn get(
    ctx: &mut Ctx,
    target: &str,
    full: bool,
    meta_only: bool,
    related: bool,
    foreign: bool,
) -> Result<()> {
    let cfg = ctx.cfg()?;
    let (mut entry, body) = match notes::get(cfg, target) {
        Ok(view) => {
            let entry = render::entry(&view.item.meta, NOTE_FIELDS.fields(), None, None);
            (entry, view.body)
        }
        Err(missing) if foreign => match notes::get_foreign(cfg, target) {
            Ok(view) => (foreign_entry(&view, None), view.body.clone()),
            Err(_) => return Err(missing),
        },
        Err(missing) => return Err(missing),
    };

    // `--quiet` returns the id and returns early — it beats `--related`.
    if ctx.g.quiet {
        out::line(&field(&entry, "id"));
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
            let path = map.remove("path");
            map.insert("body".to_string(), Json::String(body.clone()));
            if let Some(path) = path {
                map.insert("path".to_string(), path);
            }
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

#[allow(clippy::too_many_arguments)]
fn list(
    ctx: &mut Ctx,
    tags: Option<&str>,
    any_tag: bool,
    owner: Option<String>,
    note_type: Option<&str>,
    since: Option<&str>,
    sort: &str,
    limit: i64,
    foreign: bool,
) -> Result<()> {
    let sort = SortKey::parse(sort, &NOTE_SORTS)?;
    let cutoff = match since {
        Some(value) => Some(parse_since(value)?),
        None => None,
    };
    let filter = Filter {
        tags: tags.map(parse_csv).filter(|t| !t.is_empty()),
        any_tag,
        owner,
        cutoff,
        sort,
        limit: Some(limit),
        ..Filter::default()
    }
    .with_extra("type", note_type);

    let cfg = ctx.cfg()?;
    let mut entries: Vec<Json> = notes::list(cfg, &filter, foreign)?
        .iter()
        .map(|view| render::entry(&view.item.meta, NOTE_FIELDS.fields(), None, None))
        .collect();
    if foreign {
        for view in notes::foreign_rows(cfg, &filter) {
            entries.push(foreign_entry(&view, None));
        }
        if limit >= 0 {
            entries.truncate(usize::try_from(limit).unwrap_or(0));
        }
    }
    out::rows(ctx, &entries, |entry| {
        format!(
            "{}  {}  {}",
            entry.get("id").and_then(Json::as_str).unwrap_or("-"),
            entry.get("type").and_then(Json::as_str).unwrap_or("-"),
            entry.get("title").and_then(Json::as_str).unwrap_or(""),
        )
    });
    Ok(())
}

// ---------------------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------------------

fn delete(ctx: &mut Ctx, target: &str, force: bool) -> Result<()> {
    let cfg = ctx.cfg()?;
    // Resolve first, so a bad target exits 3 before any prompt.
    let path = notes::resolve(cfg, target)?;
    let note_id = stem_of(&path).unwrap_or_else(|| target.to_string());
    out::delete_guard(ctx, &note_id, force)?;
    let removed = notes::delete(cfg, &note_id)?;
    if ctx.g.quiet {
        out::line(&removed);
        return Ok(());
    }
    if ctx.g.json {
        let mut payload = Map::new();
        payload.insert("id".to_string(), Json::String(removed));
        payload.insert("deleted".to_string(), Json::Bool(true));
        out::line(out::json_line(&Json::Object(payload)).trim_end_matches('\n'));
        return Ok(());
    }
    out::line(&format!("deleted {removed}"));
    Ok(())
}

fn stem_of(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
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
            "id": "n-CFCC",
            "type": "note",
            "title": "Alpha Note",
            "tags": ["a", "b"],
            "owner": "demo-agent",
            "created": "2026-09-05T07:27:18.265543Z",
            "updated": "2026-09-05T07:27:18.265543Z",
            "related": [],
        })
    }

    #[test]
    fn the_meta_block_is_eight_lines_in_order() {
        let text = meta_block(&entry());
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[0], "id: n-CFCC");
        assert_eq!(lines[1], "type: note");
        assert_eq!(lines[2], "title: Alpha Note");
        assert_eq!(lines[3], "tags: a, b");
        assert_eq!(lines[4], "owner: demo-agent");
        assert_eq!(lines[5], "created: 2026-09-05T07:27:18.265543Z");
        assert_eq!(lines[6], "updated: 2026-09-05T07:27:18.265543Z");
        // An empty related list keeps the trailing space.
        assert_eq!(lines[7], "related: ");
    }

    #[test]
    fn a_null_owner_renders_as_an_empty_value() {
        let mut e = entry();
        e["owner"] = Json::Null;
        assert!(meta_block(&e).contains("\nowner: \n"));
    }

    #[test]
    fn a_foreign_entry_nulls_the_id_and_type() {
        let view = ForeignView {
            title: Some("Loose".into()),
            body: "text".into(),
            path: PathBuf::from("/v/notes/loose.md"),
        };
        let payload = foreign_entry(&view, Some("text"));
        let keys: Vec<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["id", "type", "title", "body", "path"]);
        assert!(payload["id"].is_null());
        assert!(payload["type"].is_null());
        assert_eq!(payload["title"], Json::String("Loose".into()));
    }

    #[test]
    fn the_sort_rejection_names_the_three_note_keys() {
        let err = SortKey::parse("bogus", &NOTE_SORTS).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid sort field: 'bogus' (use updated, created, title)"
        );
    }
}
