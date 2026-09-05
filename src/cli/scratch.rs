// STUB: owned by agent 4 (scratch).
//! `mesh scratch …`.

use std::io::Read as _;
use std::path::Path;

use serde_json::Value as Json;

use crate::cli::out;
use crate::cli::ScratchSub;
use crate::ctx::Ctx;
use crate::domain::scratch as scratch_domain;
use crate::domain::{AppendOpts, Filter};
use crate::error::{MeshError, Result};
use crate::timefmt::iso_z;

/// The agent this invocation addresses: `--agent` when given, else the effective identity.
fn resolve_agent(ctx: &Ctx, flag: Option<&str>) -> Result<String> {
    if let Some(a) = flag {
        return Ok(a.to_string());
    }
    ctx.actor().map(str::to_string).ok_or_else(|| {
        MeshError::Validation("no agent identity: set [core].agent or pass --owner".to_string())
    })
}

fn no_body_error() -> MeshError {
    MeshError::Validation("no body: pass --body or --file on a non-interactive path".to_string())
}

/// `--body` > `--file` > `-` (stdin) > `$EDITOR` on a tty > the standard no-body error.
fn resolve_body(
    ctx: &Ctx,
    body: Option<&str>,
    file: Option<&Path>,
    source: Option<&str>,
) -> Result<String> {
    if let Some(b) = body {
        return Ok(b.to_string());
    }
    if let Some(path) = file {
        return std::fs::read_to_string(path).map_err(|e| {
            MeshError::Validation(format!("cannot read --file {}: {e}", path.display()))
        });
    }
    if source == Some("-") {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(MeshError::Io)?;
        return Ok(buf);
    }
    if ctx.tty {
        if let Ok(editor) = std::env::var("EDITOR") {
            if !editor.trim().is_empty() {
                return spawn_editor(&editor);
            }
        }
    }
    Err(no_body_error())
}

fn spawn_editor(editor: &str) -> Result<String> {
    let tmp = tempfile::NamedTempFile::new().map_err(MeshError::Io)?;
    let status = std::process::Command::new(editor)
        .arg(tmp.path())
        .status()
        .map_err(MeshError::Io)?;
    if !status.success() {
        return Err(MeshError::Validation(format!(
            "editor exited with an error: {editor}"
        )));
    }
    std::fs::read_to_string(tmp.path()).map_err(MeshError::Io)
}

/// `scratch list`'s row rendering: class L, but name-addressed (not id-addressed) so it does
/// not route through `out::rows`, whose `--quiet` path is keyed on `id`.
fn render_list(ctx: &Ctx, entries: &[Json]) {
    if ctx.g.json {
        let text =
            serde_json::to_string(&Json::Array(entries.to_vec())).unwrap_or_else(|_| "[]".into());
        out::line(&text);
        return;
    }
    if ctx.g.quiet {
        for entry in entries {
            out::line(entry.get("name").and_then(Json::as_str).unwrap_or(""));
        }
        return;
    }
    for entry in entries {
        let name = entry.get("name").and_then(Json::as_str).unwrap_or("");
        let bytes = entry.get("bytes").and_then(Json::as_u64).unwrap_or(0);
        let updated = entry.get("updated").and_then(Json::as_str).unwrap_or("");
        out::line(&format!("{name}\t{bytes}\t{updated}"));
    }
}

/// Run one `scratch` subcommand.
pub fn run(ctx: &mut Ctx, sub: ScratchSub) -> Result<()> {
    match sub {
        ScratchSub::Set {
            name,
            source,
            body,
            file,
            agent,
            out: out_flags,
        } => {
            ctx.coalesce(out_flags.json, out_flags.quiet, None);
            let cfg = ctx.cfg()?;
            let agent_id = resolve_agent(ctx, agent.as_deref())?;
            let body_text = resolve_body(ctx, body.as_deref(), file.as_deref(), source.as_deref())?;
            let scratch = scratch_domain::set(cfg, &agent_id, &name, &body_text)?;
            let updated = scratch.updated.map(|u| iso_z(&u)).unwrap_or_default();
            out::mutation_named(
                ctx,
                &scratch.name,
                "wrote",
                &[
                    ("agent", Json::String(scratch.agent.clone())),
                    ("updated", Json::String(updated)),
                ],
            );
            Ok(())
        }
        ScratchSub::Append {
            name,
            text,
            section,
            timestamp,
            agent,
            out: out_flags,
        } => {
            ctx.coalesce(out_flags.json, out_flags.quiet, None);
            let cfg = ctx.cfg()?;
            let agent_id = resolve_agent(ctx, agent.as_deref())?;
            let actor = ctx.actor().map(str::to_string);
            let opts = AppendOpts {
                section,
                timestamp,
                actor,
            };
            let scratch = scratch_domain::append(cfg, &agent_id, &name, &text, opts)?;
            let updated = scratch.updated.map(|u| iso_z(&u)).unwrap_or_default();
            out::mutation_named(
                ctx,
                &scratch.name,
                "appended",
                &[
                    ("agent", Json::String(scratch.agent.clone())),
                    ("updated", Json::String(updated)),
                ],
            );
            Ok(())
        }
        ScratchSub::Get {
            name,
            agent,
            out: out_flags,
        } => {
            ctx.coalesce(out_flags.json, out_flags.quiet, None);
            let cfg = ctx.cfg()?;
            let agent_id = resolve_agent(ctx, agent.as_deref())?;
            let view = scratch_domain::get(cfg, &agent_id, &name)?;
            let updated = view
                .item
                .updated
                .map(|u| Json::String(iso_z(&u)))
                .unwrap_or(Json::Null);
            let payload = serde_json::json!({
                "name": view.item.name,
                "agent": view.item.agent,
                "path": view.path.display().to_string(),
                "bytes": view.item.bytes,
                "updated": updated,
                "content": view.body,
            });
            out::object(ctx, &payload, |v| {
                v.get("content")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string()
            });
            Ok(())
        }
        ScratchSub::List {
            agent,
            all_agents,
            since,
            out: out_flags,
        } => {
            ctx.coalesce(out_flags.json, out_flags.quiet, None);
            let cfg = ctx.cfg()?;
            let cutoff = match since.as_deref() {
                Some(s) => Some(crate::timefmt::parse_since(s)?),
                None => None,
            };
            let filter = Filter {
                cutoff,
                ..Filter::unbounded()
            };
            let list_agent = if all_agents {
                None
            } else {
                Some(resolve_agent(ctx, agent.as_deref())?)
            };
            let views = scratch_domain::list(cfg, list_agent.as_deref(), all_agents, &filter)?;
            let entries: Vec<Json> = views
                .iter()
                .map(|v| {
                    serde_json::json!({
                        "name": v.item.name,
                        "agent": v.item.agent,
                        "path": v.path.display().to_string(),
                        "bytes": v.item.bytes,
                        "updated": v.item.updated.map(|u| iso_z(&u)),
                    })
                })
                .collect();
            render_list(ctx, &entries);
            Ok(())
        }
        ScratchSub::Clear {
            name,
            agent,
            force,
            out: out_flags,
        } => {
            ctx.coalesce(out_flags.json, out_flags.quiet, None);
            let cfg = ctx.cfg()?;
            let agent_id = resolve_agent(ctx, agent.as_deref())?;
            out::delete_guard(ctx, &name, force)?;
            let deleted = scratch_domain::clear(cfg, &agent_id, &name)?;
            out::mutation_named(ctx, &deleted, "deleted", &[("deleted", Json::Bool(true))]);
            Ok(())
        }
    }
}
