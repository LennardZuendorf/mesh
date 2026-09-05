// STUB: owned by agent 9 (admin + watch).
//! `mesh watch` — the foreground watcher.

use std::path::{Path, PathBuf};

use crate::cli::WatchArgs;
use crate::config::Config;
use crate::ctx::Ctx;
use crate::error::{MeshError, Result};

/// Run the watcher.
pub fn run(ctx: &mut Ctx, args: WatchArgs) -> Result<()> {
    ctx.coalesce(args.json, false, None);
    ctx.cfg()?;
    Err(MeshError::Validation("not implemented: watch".to_string()))
}

/// `$XDG_RUNTIME_DIR/mesh-<hash12>.watch.lock`, else `~/.mesh/run/`.
pub fn watch_lock_path(cfg: &Config) -> PathBuf {
    let digest = crate::ids::sha256_hex(cfg.vault().to_string_lossy().as_bytes());
    let short = digest.get(..12).unwrap_or("mesh").to_string();
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".mesh")
                .join("run")
        });
    dir.join(format!("mesh-{short}.watch.lock"))
}

/// The pid of a live watcher for this vault, if there is one.
pub fn watcher_pid(cfg: &Config) -> Option<u32> {
    let path = watch_lock_path(cfg);
    let raw = std::fs::read_to_string(path).ok()?;
    raw.trim().parse::<u32>().ok()
}

/// Where a file belongs given its frontmatter; the caller's own path on the no-move branch.
pub fn reconcile_path(_cfg: &Config, path: &Path) -> PathBuf {
    path.to_path_buf()
}
