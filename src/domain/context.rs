// STUB: owned by agent 7 (lenses). Signatures are frozen; the bodies are not.
//! `build-context` and `graph`: BFS over `related`, plus the inbound index.

use std::collections::HashMap;

use crate::config::Config;
use crate::error::{MeshError, Result};

/// A graph query result: the visited nodes, the link-direction edges, and the traversal tree.
#[derive(Clone, Debug, Default)]
pub struct GraphResult {
    pub seed: String,
    pub entries: Vec<serde_json::Value>,
    pub edges: Vec<(String, String)>,
    pub tree_edges: Vec<(String, String)>,
}

impl GraphResult {
    /// `{"seed", "nodes", "edges"}`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "seed": self.seed,
            "nodes": self.entries,
            "edges": self.edges.iter().map(|(a, b)| vec![a, b]).collect::<Vec<_>>(),
        })
    }

    /// Visited ids in BFS order, seed first.
    pub fn ids(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|e| e.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .collect()
    }

    /// The human tree: `"  " * depth + {id}\t{type}\t{title}`.
    pub fn tree_lines(&self) -> Vec<String> {
        Vec::new()
    }
}

fn todo(what: &str) -> MeshError {
    MeshError::Validation(format!("not implemented: {what}"))
}

pub fn build_context(_cfg: &Config, _seed: &str, _depth: i64) -> Result<Vec<serde_json::Value>> {
    Err(todo("context::build_context"))
}

pub fn graph_query(_cfg: &Config, _seed: &str, _depth: i64, _dir: &str) -> Result<GraphResult> {
    Err(todo("context::graph_query"))
}

pub fn inbound_index(_cfg: &Config) -> HashMap<String, Vec<String>> {
    HashMap::new()
}
