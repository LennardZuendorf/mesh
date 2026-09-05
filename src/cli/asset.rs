//! `mesh asset …` — the eight asset subcommands and their output branches.

use std::path::PathBuf;

use serde_json::{Map, Value as Json};

use crate::cli::out;
use crate::cli::AssetSub;
use crate::ctx::Ctx;
use crate::domain::assets::{self, NewAsset};
use crate::domain::select::parse_csv;
use crate::domain::{Filter, SortKey};
use crate::error::Result;
use crate::model::asset::ASSET_FIELDS;
use crate::render;
use crate::text::preview;
use crate::timefmt::{iso_z, now_utc, parse_since};

/// The sort keys `asset list` accepts, in the order the rejection message names them.
pub const ASSET_SORTS: [SortKey; 4] = [
    SortKey::Updated,
    SortKey::Created,
    SortKey::Title,
    SortKey::Bytes,
];

/// Run one `asset` subcommand.
pub fn run(ctx: &mut Ctx, sub: AssetSub) -> Result<()> {
    match sub {
        AssetSub::Add {
            path,
            title,
            tags,
            owner,
            caption,
            attach,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, owner);
            add(ctx, path, title, tags.as_deref(), caption, attach)
        }
        AssetSub::Get {
            asset_id,
            meta_only,
            full,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, None);
            get(ctx, &asset_id, meta_only, full)
        }
        AssetSub::Path { asset_id, out } => {
            ctx.coalesce(out.json, out.quiet, None);
            path(ctx, &asset_id)
        }
        AssetSub::List {
            tags,
            any_tag,
            owner,
            mine,
            media_type,
            since,
            sort,
            limit,
            out,
        } => {
            ctx.coalesce_mine(mine);
            ctx.coalesce(out.json, out.quiet, None);
            list(
                ctx,
                tags.as_deref(),
                any_tag,
                owner,
                media_type.as_deref(),
                since.as_deref(),
                &sort,
                limit,
            )
        }
        AssetSub::Attach {
            asset_id,
            target,
            section,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, None);
            attach(ctx, &asset_id, &target, section.as_deref())
        }
        AssetSub::Detach {
            asset_id,
            target,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, None);
            detach(ctx, &asset_id, &target)
        }
        AssetSub::Remove {
            asset_id,
            force,
            out,
        } => {
            ctx.coalesce(out.json, out.quiet, None);
            remove(ctx, &asset_id, force)
        }
        AssetSub::Gc { apply, out } => {
            ctx.coalesce(out.json, out.quiet, None);
            gc(ctx, apply)
        }
    }
}

// ---------------------------------------------------------------------------------------
// add
// ---------------------------------------------------------------------------------------

fn add(
    ctx: &mut Ctx,
    path: PathBuf,
    title: Option<String>,
    tags: Option<&str>,
    caption: Option<String>,
    attach: Option<String>,
) -> Result<()> {
    let owner = ctx.g.owner.clone();
    let cfg = ctx.cfg()?;
    let outcome = assets::add(
        cfg,
        &path,
        NewAsset {
            title,
            tags: tags.map(parse_csv).unwrap_or_default(),
            owner,
            caption: caption.unwrap_or_default(),
            attach,
        },
    )?;
    if outcome.deduplicated {
        out::notice(
            ctx,
            &format!(
                "asset add: identical content already stored as {}",
                outcome.asset.id
            ),
        );
    }
    out::mutation(
        ctx,
        &outcome.asset.id,
        "added",
        &[("bytes", Json::from(outcome.asset.bytes))],
        outcome.asset.updated.unwrap_or_else(now_utc),
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------------------

/// One scalar rendered for the human block: strings bare, numbers as numbers, null empty.
fn field(entry: &Json, key: &str) -> String {
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

/// The twelve-line human metadata block, rendered from the same JSON the payload uses so the
/// two surfaces cannot print a value differently.
fn meta_block(entry: &Json) -> String {
    [
        format!("id: {}", field(entry, "id")),
        format!("type: {}", field(entry, "type")),
        format!("title: {}", field(entry, "title")),
        format!("filename: {}", field(entry, "filename")),
        format!("media_type: {}", field(entry, "media_type")),
        format!("bytes: {}", field(entry, "bytes")),
        format!("sha256: {}", field(entry, "sha256")),
        format!("owner: {}", field(entry, "owner")),
        format!("tags: {}", string_list(entry, "tags").join(", ")),
        format!("created: {}", field(entry, "created")),
        format!("updated: {}", field(entry, "updated")),
        format!("related: {}", string_list(entry, "related").join(", ")),
    ]
    .join("\n")
}

fn get(ctx: &mut Ctx, id: &str, meta_only: bool, full: bool) -> Result<()> {
    let cfg = ctx.cfg()?;
    let view = assets::get(cfg, id)?;
    let mut entry = render::entry(&view.item.meta, ASSET_FIELDS.fields(), None, None);
    let body = view.body;

    // `--quiet` returns the id and returns early.
    if ctx.g.quiet {
        out::line(&field(&entry, "id"));
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
// path
// ---------------------------------------------------------------------------------------

fn path(ctx: &mut Ctx, id: &str) -> Result<()> {
    let cfg = ctx.cfg()?;
    let blob = assets::blob_path(cfg, id)?;
    let text = blob.display().to_string();
    let mut payload = Map::new();
    payload.insert("id".to_string(), Json::String(id.to_string()));
    payload.insert("path".to_string(), Json::String(text.clone()));
    out::object(ctx, &Json::Object(payload), |_| text.clone());
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
    media_type: Option<&str>,
    since: Option<&str>,
    sort: &str,
    limit: i64,
) -> Result<()> {
    let sort = SortKey::parse(sort, &ASSET_SORTS)?;
    let cutoff = match since {
        Some(value) => Some(parse_since(value)?),
        None => None,
    };
    let mine = ctx.g.mine;
    let me = ctx.actor().map(str::to_string);
    let filter = Filter {
        tags: tags.map(parse_csv).filter(|t| !t.is_empty()),
        any_tag,
        owner,
        mine,
        me,
        cutoff,
        sort,
        limit: Some(limit),
        ..Filter::default()
    };

    let cfg = ctx.cfg()?;
    let entries: Vec<Json> = assets::list(cfg, &filter, media_type)?
        .iter()
        .map(|view| render::entry(&view.item.meta, ASSET_FIELDS.fields(), None, None))
        .collect();
    out::rows(ctx, &entries, |entry| {
        format!(
            "{}\t{}\t{}\t{}",
            field(entry, "id"),
            field(entry, "media_type"),
            field(entry, "bytes"),
            field(entry, "title"),
        )
    });
    Ok(())
}

// ---------------------------------------------------------------------------------------
// attach / detach
// ---------------------------------------------------------------------------------------

/// Class M with a `target` key: `{"id", "target", "updated"}`, human `{verb} {id} {prep} {t}`.
fn report_link(ctx: &Ctx, id: &str, target: &str, updated: &str, verb: &str, prep: &str) {
    if ctx.g.quiet {
        out::line(id);
        return;
    }
    if ctx.g.json {
        let mut payload = Map::new();
        payload.insert("id".to_string(), Json::String(id.to_string()));
        payload.insert("target".to_string(), Json::String(target.to_string()));
        payload.insert("updated".to_string(), Json::String(updated.to_string()));
        out::line(out::json_line(&Json::Object(payload)).trim_end_matches('\n'));
        return;
    }
    out::line(&format!("{verb} {id} {prep} {target}"));
}

fn attach(ctx: &mut Ctx, id: &str, target: &str, section: Option<&str>) -> Result<()> {
    let cfg = ctx.cfg()?;
    let asset = assets::attach(cfg, id, target, section)?;
    let updated = iso_z(&asset.updated.unwrap_or_else(now_utc));
    report_link(ctx, &asset.id, target, &updated, "attached", "to");
    Ok(())
}

fn detach(ctx: &mut Ctx, id: &str, target: &str) -> Result<()> {
    let cfg = ctx.cfg()?;
    let asset = assets::detach(cfg, id, target)?;
    let updated = iso_z(&asset.updated.unwrap_or_else(now_utc));
    report_link(ctx, &asset.id, target, &updated, "detached", "from");
    Ok(())
}

// ---------------------------------------------------------------------------------------
// remove / gc
// ---------------------------------------------------------------------------------------

fn remove(ctx: &mut Ctx, id: &str, force: bool) -> Result<()> {
    let cfg = ctx.cfg()?;
    // Resolve first, so a bad id exits 3 before any prompt.
    let path = assets::resolve(cfg, id)?;
    let asset_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map_or_else(|| id.to_string(), str::to_string);
    // The reference refusal comes before the prompt: never ask a question whose "yes" is
    // then refused anyway.
    if !force {
        assets::check_removable(cfg, &asset_id)?;
    }
    out::delete_guard(ctx, &asset_id, force)?;
    let cfg = ctx.cfg()?;
    let removed = assets::remove(cfg, &asset_id, force)?;
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

fn strings(items: &[String]) -> Json {
    Json::Array(items.iter().map(|i| Json::String(i.clone())).collect())
}

fn gc(ctx: &mut Ctx, apply: bool) -> Result<()> {
    let cfg = ctx.cfg()?;
    let report = assets::gc(cfg, apply)?;
    let mut payload = Map::new();
    payload.insert("orphan_blobs".to_string(), strings(&report.orphan_blobs));
    payload.insert(
        "orphan_sidecars".to_string(),
        strings(&report.orphan_sidecars),
    );
    payload.insert("removed".to_string(), Json::from(report.removed));
    out::object(ctx, &Json::Object(payload), |value| {
        [
            format!(
                "orphan_blobs: {}",
                string_list(value, "orphan_blobs").join(", ")
            ),
            format!(
                "orphan_sidecars: {}",
                string_list(value, "orphan_sidecars").join(", ")
            ),
            format!("removed: {}", field(value, "removed")),
        ]
        .join("\n")
    });
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
            "id": "a-7Q3K",
            "type": "asset",
            "title": "photo.png",
            "tags": ["trip", "japan"],
            "owner": "demo-agent",
            "created": "2026-09-05T07:27:18.265543Z",
            "updated": "2026-09-05T07:27:18.265543Z",
            "related": ["n-CFCC"],
            "filename": "photo.png",
            "media_type": "image/png",
            "bytes": 2048,
            "sha256": "ff00",
            "blob": "a-7Q3K.png",
        })
    }

    #[test]
    fn the_meta_block_is_twelve_lines_in_order() {
        let text = meta_block(&entry());
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 12);
        assert_eq!(lines[0], "id: a-7Q3K");
        assert_eq!(lines[1], "type: asset");
        assert_eq!(lines[2], "title: photo.png");
        assert_eq!(lines[3], "filename: photo.png");
        assert_eq!(lines[4], "media_type: image/png");
        assert_eq!(lines[5], "bytes: 2048");
        assert_eq!(lines[6], "sha256: ff00");
        assert_eq!(lines[7], "owner: demo-agent");
        assert_eq!(lines[8], "tags: trip, japan");
        assert_eq!(lines[9], "created: 2026-09-05T07:27:18.265543Z");
        assert_eq!(lines[10], "updated: 2026-09-05T07:27:18.265543Z");
        assert_eq!(lines[11], "related: n-CFCC");
    }

    #[test]
    fn a_null_owner_renders_as_an_empty_value() {
        let mut e = entry();
        e["owner"] = Json::Null;
        assert!(meta_block(&e).contains("\nowner: \n"));
    }

    #[test]
    fn a_row_is_tab_separated() {
        let e = entry();
        let row = format!(
            "{}\t{}\t{}\t{}",
            field(&e, "id"),
            field(&e, "media_type"),
            field(&e, "bytes"),
            field(&e, "title"),
        );
        assert_eq!(row, "a-7Q3K\timage/png\t2048\tphoto.png");
    }

    #[test]
    fn the_sort_rejection_names_the_four_asset_keys() {
        let err = SortKey::parse("bogus", &ASSET_SORTS).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid sort field: 'bogus' (use updated, created, title, bytes)"
        );
    }
}
