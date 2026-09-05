// STUB: owned by agent 9 (admin + watch).
//! `init`, `status`, `reindex`, `config`, `completions` and the hidden `daemon` shim.

use crate::cli::{CompletionsArgs, ConfigSub, DaemonSub, InitArgs, ReindexArgs, StatusArgs};
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};

fn todo(what: &str) -> MeshError {
    MeshError::Validation(format!("not implemented: {what}"))
}

pub fn init(_ctx: &mut Ctx, _args: InitArgs) -> Result<()> {
    Err(todo("init"))
}

pub fn status(ctx: &mut Ctx, _args: StatusArgs) -> Result<()> {
    ctx.cfg()?;
    Err(todo("status"))
}

pub fn reindex(ctx: &mut Ctx, _args: ReindexArgs) -> Result<()> {
    ctx.cfg()?;
    Err(todo("reindex"))
}

pub fn config(ctx: &mut Ctx, sub: ConfigSub) -> Result<()> {
    // `config path` answers without a config file; every other form needs one.
    let verb = match sub {
        ConfigSub::Path => "config path",
        ConfigSub::Show { json } => {
            ctx.coalesce(json, false, None);
            ctx.cfg()?;
            "config show"
        }
        ConfigSub::Get { .. } => {
            ctx.cfg()?;
            "config get"
        }
        ConfigSub::Set { .. } => {
            ctx.cfg()?;
            "config set"
        }
    };
    Err(todo(verb))
}

pub fn completions(_ctx: &mut Ctx, _args: CompletionsArgs) -> Result<()> {
    Err(todo("completions"))
}

pub fn daemon(_ctx: &mut Ctx, sub: DaemonSub) -> Result<()> {
    let verb = match sub {
        DaemonSub::Start => "daemon start",
        DaemonSub::Stop => "daemon stop",
        DaemonSub::Status => "daemon status",
    };
    Err(todo(verb))
}
