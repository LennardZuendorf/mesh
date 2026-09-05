// STUB: owned by agent 3 (task).
//! `mesh task block | unblock | next` and the `--ready` / `--blocked` plumbing.

use crate::cli::TaskSub;
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};

/// Run one dependency-graph `task` subcommand.
pub fn run(ctx: &mut Ctx, sub: TaskSub) -> Result<()> {
    let (json, quiet, verb) = match &sub {
        TaskSub::Block { out, .. } => (out.json, out.quiet, "task block"),
        TaskSub::Unblock { out, .. } => (out.json, out.quiet, "task unblock"),
        TaskSub::Next { out, mine, .. } => {
            ctx.coalesce_mine(*mine);
            (out.json, out.quiet, "task next")
        }
        other => {
            let _ = other;
            (false, false, "task")
        }
    };
    ctx.coalesce(json, quiet, None);
    ctx.cfg()?;
    Err(MeshError::Validation(format!("not implemented: {verb}")))
}
