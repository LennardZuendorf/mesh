// STUB: owned by agent 3 (task).
//! `mesh task …` (lifecycle verbs).

use crate::cli::TaskSub;
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};

/// Run one lifecycle `task` subcommand. `block`/`unblock`/`next` live in `task_dep`.
pub fn run(ctx: &mut Ctx, sub: TaskSub) -> Result<()> {
    let (json, quiet, owner, verb) = match &sub {
        TaskSub::New { out, owner, .. } => (out.json, out.quiet, owner.clone(), "task new"),
        // The global --owner is deliberately not folded into the reassignment --owner.
        TaskSub::Update { out, .. } => (out.json, out.quiet, None, "task update"),
        TaskSub::Append { out, .. } => (out.json, out.quiet, None, "task append"),
        TaskSub::Claim { out, .. } => (out.json, out.quiet, None, "task claim"),
        TaskSub::Release { out, .. } => (out.json, out.quiet, None, "task release"),
        TaskSub::Finish { out, .. } => (out.json, out.quiet, None, "task finish"),
        TaskSub::Cancel { out, .. } => (out.json, out.quiet, None, "task cancel"),
        TaskSub::Get { out, .. } => (out.json, out.quiet, None, "task get"),
        TaskSub::List {
            out, owner, mine, ..
        } => {
            ctx.coalesce_mine(*mine);
            (out.json, out.quiet, owner.clone(), "task list")
        }
        TaskSub::Delete { out, .. } => (out.json, out.quiet, None, "task delete"),
        TaskSub::Block { out, .. } => (out.json, out.quiet, None, "task block"),
        TaskSub::Unblock { out, .. } => (out.json, out.quiet, None, "task unblock"),
        TaskSub::Next { out, .. } => (out.json, out.quiet, None, "task next"),
    };
    ctx.coalesce(json, quiet, owner);
    ctx.cfg()?;
    Err(MeshError::Validation(format!("not implemented: {verb}")))
}
