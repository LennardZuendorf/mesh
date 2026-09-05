// STUB: owned by agent 4 (scratch).
//! `mesh scratch …`.

use crate::cli::ScratchSub;
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};

/// Run one `scratch` subcommand.
pub fn run(ctx: &mut Ctx, sub: ScratchSub) -> Result<()> {
    let (json, quiet, verb) = match &sub {
        ScratchSub::Set { out, .. } => (out.json, out.quiet, "scratch set"),
        ScratchSub::Append { out, .. } => (out.json, out.quiet, "scratch append"),
        ScratchSub::Get { out, .. } => (out.json, out.quiet, "scratch get"),
        ScratchSub::List { out, .. } => (out.json, out.quiet, "scratch list"),
        ScratchSub::Clear { out, .. } => (out.json, out.quiet, "scratch clear"),
    };
    ctx.coalesce(json, quiet, None);
    ctx.cfg()?;
    Err(MeshError::Validation(format!("not implemented: {verb}")))
}
