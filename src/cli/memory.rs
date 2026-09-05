// STUB: owned by agent 2 (memory).
//! `mesh memory …`.

use crate::cli::MemorySub;
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};

/// Run one `memory` subcommand.
pub fn run(ctx: &mut Ctx, sub: MemorySub) -> Result<()> {
    let (json, quiet, owner, verb) = match &sub {
        MemorySub::New { out, owner, .. } => (out.json, out.quiet, owner.clone(), "memory new"),
        MemorySub::Append { out, .. } => (out.json, out.quiet, None, "memory append"),
        // The global --owner is deliberately not folded into the reassignment --owner.
        MemorySub::Update { out, .. } => (out.json, out.quiet, None, "memory update"),
        MemorySub::Get { out, .. } => (out.json, out.quiet, None, "memory get"),
        MemorySub::List {
            out, owner, mine, ..
        } => {
            ctx.coalesce_mine(*mine);
            (out.json, out.quiet, owner.clone(), "memory list")
        }
        MemorySub::Recall {
            out, owner, mine, ..
        } => {
            ctx.coalesce_mine(*mine);
            (out.json, out.quiet, owner.clone(), "memory recall")
        }
        MemorySub::Forget { out, .. } => (out.json, out.quiet, None, "memory forget"),
    };
    ctx.coalesce(json, quiet, owner);
    ctx.cfg()?;
    Err(MeshError::Validation(format!("not implemented: {verb}")))
}
