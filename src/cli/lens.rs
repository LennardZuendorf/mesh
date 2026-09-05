// STUB: owned by agent 7 (lenses).
//! `recent-activity`, `build-context`, `graph`, `project`, `session-start`.

use crate::cli::{BuildContextArgs, GraphArgs, ProjectArgs, RecentActivityArgs, SessionStartArgs};
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};

fn todo(what: &str) -> MeshError {
    MeshError::Validation(format!("not implemented: {what}"))
}

pub fn recent_activity(ctx: &mut Ctx, args: RecentActivityArgs) -> Result<()> {
    ctx.coalesce_mine(args.mine);
    ctx.coalesce(args.out.json, args.out.quiet, args.owner.clone());
    ctx.cfg()?;
    Err(todo("recent-activity"))
}

pub fn build_context(ctx: &mut Ctx, args: BuildContextArgs) -> Result<()> {
    ctx.coalesce(args.out.json, args.out.quiet, None);
    ctx.cfg()?;
    Err(todo("build-context"))
}

pub fn graph(ctx: &mut Ctx, args: GraphArgs) -> Result<()> {
    ctx.coalesce(args.out.json, args.out.quiet, None);
    ctx.cfg()?;
    Err(todo("graph"))
}

pub fn project(ctx: &mut Ctx, args: ProjectArgs) -> Result<()> {
    ctx.coalesce(args.out.json, args.out.quiet, None);
    ctx.cfg()?;
    Err(todo("project"))
}

pub fn session_start(ctx: &mut Ctx, args: SessionStartArgs) -> Result<()> {
    ctx.coalesce(args.out.json, args.out.quiet, args.owner.clone());
    ctx.cfg()?;
    Err(todo("session-start"))
}
