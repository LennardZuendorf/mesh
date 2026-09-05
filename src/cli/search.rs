//! `mesh search …` — class **S**: output is always one JSON line on stdout.
//!
//! `--json` is accepted and inert (search is machine-first by design); `--quiet` suppresses
//! only the stderr degradation notice, never the payload. `--health` short-circuits before
//! everything else, even with a query, and never shells `indexed`.

use serde_json::Value as Json;

use crate::cli::{out, SearchArgs};
use crate::ctx::Ctx;
use crate::error::Result;
use crate::render;
use crate::search::{self, Engine, SearchFilter};

/// Run `mesh search`. Output is always one JSON line; `--json` is accepted and inert.
pub fn run(ctx: &mut Ctx, args: SearchArgs) -> Result<()> {
    ctx.coalesce(args.out.json, args.out.quiet, args.owner.clone());
    let quiet = ctx.g.quiet;
    let owner = ctx.g.owner.clone();
    let cfg = ctx.cfg()?;

    if args.health {
        emit(&search::health(cfg));
        return Ok(());
    }

    let engine = Engine::parse(&args.engine)?;
    let spaces = search::resolve_spaces(cfg, args.space.as_deref())?;
    let filter = SearchFilter {
        spaces: spaces.clone(),
        type_filter: args.type_filter,
        tags: args.tags,
        owner,
        status: args.status,
        kind: args.kind,
        limit: args.limit,
        threshold: search::resolve_effective_threshold(args.threshold, cfg),
        engine,
        quiet,
    };

    let mut hits = match args.query.as_deref() {
        None => search::tag_pull(cfg, &filter)?,
        Some(query) => search::query(cfg, query, &filter)?.0,
    };
    if args.full && !args.meta_only {
        search::fill_full_bodies(&mut hits);
    }

    let space_key = search::emit_space_key(cfg, &spaces, args.space.is_some());
    let payload: Vec<Json> = hits
        .iter()
        .map(|hit| {
            render::hit(
                hit,
                args.meta_only,
                args.full,
                space_key.then_some(hit.space),
            )
        })
        .collect();
    emit(&Json::Array(payload));
    Ok(())
}

/// One compact JSON line on stdout, whatever `--json` / `--quiet` say.
fn emit(value: &Json) {
    out::line(&serde_json::to_string(value).unwrap_or_else(|_| "null".to_string()));
}
