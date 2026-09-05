// STUB: owned by agent 1 (note).
//! `mesh note …`.

use crate::cli::NoteSub;
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};

/// Run one `note` subcommand.
pub fn run(ctx: &mut Ctx, sub: NoteSub) -> Result<()> {
    let (json, quiet, owner, verb) = match &sub {
        NoteSub::New { out, owner, .. } => (out.json, out.quiet, owner.clone(), "note new"),
        NoteSub::Append { out, .. } => (out.json, out.quiet, None, "note append"),
        NoteSub::Update { out, .. } => (out.json, out.quiet, None, "note update"),
        NoteSub::Get { out, .. } => (out.json, out.quiet, None, "note get"),
        NoteSub::List { out, owner, .. } => (out.json, out.quiet, owner.clone(), "note list"),
        NoteSub::Delete { out, .. } => (out.json, out.quiet, None, "note delete"),
    };
    ctx.coalesce(json, quiet, owner);
    ctx.cfg()?;
    Err(MeshError::Validation(format!("not implemented: {verb}")))
}
