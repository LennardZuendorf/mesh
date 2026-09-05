// STUB: owned by agent 7 (lenses). Signatures are frozen; the bodies are not.
//! `project`, `session-start` and the `status` payload.

use std::path::PathBuf;

use crate::config::Config;
use crate::error::{MeshError, Result};
use crate::fm::View;
use crate::model::memory::Memory;
use crate::model::note::Note;
use crate::model::task::Task;

/// The `--since` window `session-start` uses for mentions and activity.
pub const SESSION_SINCE: &str = "7d";

fn todo(what: &str) -> MeshError {
    MeshError::Validation(format!("not implemented: {what}"))
}

pub fn project_view(_cfg: &Config, _project_id: &str) -> Result<serde_json::Value> {
    Err(todo("lenses::project_view"))
}

pub fn session_mentions(
    _cfg: &Config,
    _tasks: &[View<Task>],
    _notes: &[View<Note>],
    _me: Option<&str>,
    _since: &str,
) -> Vec<serde_json::Value> {
    Vec::new()
}

pub fn session_start_entries(
    _cfg: &Config,
    _tasks: &[View<Task>],
    _mentions: Vec<serde_json::Value>,
    _memories: Vec<View<Memory>>,
    _activity: Vec<serde_json::Value>,
    _meta_only: bool,
    _budget: usize,
) -> Vec<serde_json::Value> {
    Vec::new()
}

pub fn scan_stale_locks(_cfg: &Config) -> Vec<PathBuf> {
    Vec::new()
}

pub fn status_report(_cfg: &Config) -> serde_json::Value {
    serde_json::json!({})
}
