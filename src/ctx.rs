//! The per-invocation context: merged flags, the lazily loaded config, and tty state.

use std::cell::OnceCell;

use crate::cli::globals::GlobalOpts;
use crate::config::{load_config, Config};
use crate::error::{MeshError, Result};

/// Everything a verb needs that is not its own arguments.
///
/// The config is loaded lazily through [`Ctx::cfg`], so `init`, `completions`, `config path`,
/// `mcp` and `--version` never require one.
#[derive(Debug)]
pub struct Ctx {
    cfg: OnceCell<Config>,
    pub g: GlobalOpts,
    pub tty: bool,
}

impl Ctx {
    /// A context for these global flags, detecting whether stdin is a terminal.
    pub fn new(g: GlobalOpts) -> Ctx {
        let tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
        Ctx {
            cfg: OnceCell::new(),
            g,
            tty,
        }
    }

    /// A context with an explicit config, for tests and for the MCP server.
    pub fn with_config(g: GlobalOpts, config: Config, tty: bool) -> Ctx {
        let cell = OnceCell::new();
        let _ = cell.set(config);
        Ctx { cfg: cell, g, tty }
    }

    /// The effective config, loading it on first use.
    pub fn cfg(&self) -> Result<&Config> {
        if let Some(cfg) = self.cfg.get() {
            return Ok(cfg);
        }
        let loaded = load_config(self.g.config.as_deref(), self.g.vault.as_deref())?;
        let _ = self.cfg.set(loaded);
        self.cfg
            .get()
            .ok_or_else(|| MeshError::Validation("config was not loaded".to_string()))
    }

    /// Merge a subcommand's local `--json` / `--quiet` / `--owner` into the globals.
    pub fn coalesce(&mut self, json: bool, quiet: bool, owner: Option<String>) {
        self.g.coalesce(json, quiet, owner);
    }

    /// OR a subcommand's local `--mine` into the global one.
    pub fn coalesce_mine(&mut self, mine: bool) {
        self.g.mine = self.g.mine || mine;
    }

    /// `--json` or `--quiet`.
    pub fn is_machine(&self) -> bool {
        self.g.is_machine()
    }

    /// Who is running the command: the global `--owner`, else `[core].agent`.
    pub fn actor(&self) -> Option<&str> {
        if let Some(owner) = self.g.owner.as_deref() {
            return Some(owner);
        }
        self.cfg().ok().and_then(Config::agent)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::config::test_support::config_for;

    #[test]
    fn actor_prefers_the_global_owner() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let ctx = Ctx::with_config(GlobalOpts::default(), cfg.clone(), false);
        assert_eq!(ctx.actor(), Some("test-agent"));
        let g = GlobalOpts {
            owner: Some("alice".into()),
            ..GlobalOpts::default()
        };
        let ctx = Ctx::with_config(g, cfg, false);
        assert_eq!(ctx.actor(), Some("alice"));
    }

    #[test]
    fn config_is_only_loaded_when_asked_for() {
        let g = GlobalOpts {
            config: Some(std::path::PathBuf::from("/definitely/not/here.toml")),
            ..GlobalOpts::default()
        };
        let ctx = Ctx::new(g);
        assert!(!ctx.is_machine());
        assert_eq!(ctx.cfg().unwrap_err().code(), 2);
    }
}
