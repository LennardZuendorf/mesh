//! `mesh search --health` — the five-key gate report.
//!
//! Never raises, never shells `indexed`: every gate is a config read, a lock-file read or a
//! `PATH` lookup. `--health` short-circuits before any query or tag pull.

use serde_json::{Map, Value as Json};

use crate::config::Config;
use crate::search::indexed;

/// `[search].hybrid = false`.
pub const REASON_HYBRID: &str = "hybrid disabled ([search].hybrid = false)";
/// `[search].collection` unset.
pub const REASON_COLLECTION: &str = "no collection configured ([search].collection unset)";
/// No `indexed` on `PATH` and no `$MESH_INDEXED_BIN`.
pub const REASON_BINARY: &str = "indexed binary not found on PATH";

/// The first closed gate, in priority order, or `None` when every gate is open.
///
/// Python's third gate, `daemon down`, went with the daemon: `daemon_up` is still reported —
/// truthfully, as watcher liveness — but it no longer gates recall.
pub fn reason(hybrid: bool, collection: Option<&str>, binary: bool) -> Option<&'static str> {
    if !hybrid {
        return Some(REASON_HYBRID);
    }
    if collection.is_none() {
        return Some(REASON_COLLECTION);
    }
    if !binary {
        return Some(REASON_BINARY);
    }
    None
}

/// Whether a `mesh watch` holds this vault.
pub fn watcher_up(cfg: &Config) -> bool {
    crate::cli::watch::watcher_pid(cfg).is_some()
}

/// The `--health` payload: five keys in order, plus `reason` only when degraded.
pub fn payload(cfg: &Config) -> Json {
    let hybrid = cfg.search.hybrid;
    let collection = cfg.search.collection.clone();
    let daemon_up = watcher_up(cfg);
    let binary = indexed::available();
    let reason = reason(hybrid, collection.as_deref(), binary);

    let mut out = Map::new();
    out.insert(
        "mode".to_string(),
        Json::String(
            if reason.is_none() {
                "indexed"
            } else {
                "fallback"
            }
            .to_string(),
        ),
    );
    out.insert("hybrid_configured".to_string(), Json::Bool(hybrid));
    out.insert(
        "collection".to_string(),
        collection.map_or(Json::Null, Json::String),
    );
    out.insert("daemon_up".to_string(), Json::Bool(daemon_up));
    out.insert("indexed_binary_available".to_string(), Json::Bool(binary));
    if let Some(text) = reason {
        out.insert("reason".to_string(), Json::String(text.to_string()));
    }
    Json::Object(out)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::config::test_support::config_for;

    #[test]
    fn the_first_closed_gate_wins() {
        assert_eq!(reason(false, None, false), Some(REASON_HYBRID));
        assert_eq!(reason(false, Some("c"), true), Some(REASON_HYBRID));
        assert_eq!(reason(true, None, true), Some(REASON_COLLECTION));
        assert_eq!(reason(true, Some("c"), false), Some(REASON_BINARY));
        assert_eq!(reason(true, Some("c"), true), None);
    }

    #[test]
    fn payload_keeps_five_keys_in_order_when_healthy() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.search.hybrid = true;
        cfg.search.collection = Some("c".into());
        let json = payload(&cfg);
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            &keys[..5],
            [
                "mode",
                "hybrid_configured",
                "collection",
                "daemon_up",
                "indexed_binary_available"
            ]
        );
    }

    #[test]
    fn a_degraded_payload_appends_reason_last() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.search.hybrid = false;
        let json = payload(&cfg);
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys.last(), Some(&"reason"));
        assert_eq!(json["mode"], Json::String("fallback".into()));
        assert_eq!(json["reason"], Json::String(REASON_HYBRID.into()));
    }

    #[test]
    fn collection_is_null_when_unset() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let json = payload(&cfg);
        assert_eq!(json["collection"], Json::Null);
        assert_eq!(json["hybrid_configured"], Json::Bool(true));
    }

    #[test]
    fn every_gate_is_evaluated_even_when_an_earlier_one_closed() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.search.hybrid = false;
        cfg.search.collection = Some("c".into());
        let json = payload(&cfg);
        assert_eq!(json["collection"], Json::String("c".into()));
        assert!(json["indexed_binary_available"].is_boolean());
        assert!(json["daemon_up"].is_boolean());
    }
}
