// STUB: owned by agent 7 (lenses). Signatures are frozen; the bodies are not.
//! `recent-activity`: the mtime-ordered vault change feed.

use crate::config::Config;
use crate::error::{MeshError, Result};
use crate::spaces::Space;

pub fn scan_recent(_cfg: &Config, _limit: i64, _spaces: &[Space]) -> Vec<serde_json::Value> {
    Vec::new()
}

pub fn recent_activity(
    _cfg: &Config,
    _since: Option<&str>,
    _owner: Option<&str>,
    _mine: bool,
    _limit: i64,
) -> Result<Vec<serde_json::Value>> {
    Err(MeshError::Validation(
        "not implemented: activity::recent_activity".to_string(),
    ))
}
