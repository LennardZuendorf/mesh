// STUB: owned by agent 6 (search).
//! `mesh search …`.

use crate::cli::SearchArgs;
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};

/// Run `mesh search`. Output is always one JSON line; `--json` is accepted and inert.
pub fn run(ctx: &mut Ctx, args: SearchArgs) -> Result<()> {
    ctx.coalesce(args.out.json, args.out.quiet, args.owner.clone());
    ctx.cfg()?;
    Err(MeshError::Validation("not implemented: search".to_string()))
}
