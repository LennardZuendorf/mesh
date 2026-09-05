// STUB: owned by agent 5 (asset).
//! `mesh asset …`.

use crate::cli::AssetSub;
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};

/// Run one `asset` subcommand.
pub fn run(ctx: &mut Ctx, sub: AssetSub) -> Result<()> {
    let (json, quiet, owner, verb) = match &sub {
        AssetSub::Add { out, owner, .. } => (out.json, out.quiet, owner.clone(), "asset add"),
        AssetSub::Get { out, .. } => (out.json, out.quiet, None, "asset get"),
        AssetSub::Path { out, .. } => (out.json, out.quiet, None, "asset path"),
        AssetSub::List {
            out, owner, mine, ..
        } => {
            ctx.coalesce_mine(*mine);
            (out.json, out.quiet, owner.clone(), "asset list")
        }
        AssetSub::Attach { out, .. } => (out.json, out.quiet, None, "asset attach"),
        AssetSub::Detach { out, .. } => (out.json, out.quiet, None, "asset detach"),
        AssetSub::Remove { out, .. } => (out.json, out.quiet, None, "asset remove"),
        AssetSub::Gc { out, .. } => (out.json, out.quiet, None, "asset gc"),
    };
    ctx.coalesce(json, quiet, owner);
    ctx.cfg()?;
    Err(MeshError::Validation(format!("not implemented: {verb}")))
}
