// STUB: owned by agent 6 (search) after phase 0. The `Engine`/`route` seam is frozen.
//! Search: engine routing, the built-in ranker, and the `indexed` wrapper.

pub mod builtin;
pub mod corpus;
pub mod health;
pub mod indexed;
pub mod tagpull;
pub mod tokenize;

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::config::Config;
use crate::error::{MeshError, Result};
use crate::fm::Row;
use crate::spaces::Space;

/// The branch a query actually took. Never predicted from the gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Indexed,
    Builtin,
}

impl Mode {
    /// The `mode` value in the `--health` payload.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Indexed => "indexed",
            Mode::Builtin => "fallback",
        }
    }
}

/// What `--engine` asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Engine {
    #[default]
    Auto,
    Indexed,
    Builtin,
    Substring,
}

impl Engine {
    /// Parse an `--engine` value.
    pub fn parse(value: &str) -> Result<Engine> {
        match value {
            "auto" => Ok(Engine::Auto),
            "indexed" => Ok(Engine::Indexed),
            "builtin" => Ok(Engine::Builtin),
            "substring" => Ok(Engine::Substring),
            other => Err(MeshError::Validation(format!(
                "invalid engine: '{other}' (use auto, indexed, builtin, substring)"
            ))),
        }
    }
}

/// The conjunctive filter a search applies, plus its routing switches.
#[derive(Clone, Debug)]
pub struct SearchFilter {
    pub spaces: Vec<Space>,
    pub type_filter: Option<String>,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub status: Option<String>,
    pub kind: Option<String>,
    pub limit: i64,
    pub threshold: Option<f64>,
    pub engine: Engine,
    pub quiet: bool,
}

impl Default for SearchFilter {
    fn default() -> Self {
        SearchFilter {
            spaces: vec![Space::Notes, Space::Tasks, Space::Memories, Space::Assets],
            type_filter: None,
            tags: Vec::new(),
            owner: None,
            status: None,
            kind: None,
            limit: 10,
            threshold: None,
            engine: Engine::Auto,
            quiet: false,
        }
    }
}

/// One search hit. `id`/`type`/`title` are null for foreign Markdown.
#[derive(Clone, Debug)]
pub struct Hit {
    pub id: Option<String>,
    pub r#type: Option<String>,
    pub title: Option<String>,
    pub score: f64,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub updated: Option<DateTime<Utc>>,
    pub snippet: Option<String>,
    pub path: PathBuf,
    pub space: Space,
}

fn todo(what: &str) -> MeshError {
    MeshError::Validation(format!("not implemented: {what}"))
}

pub fn query(_cfg: &Config, _q: &str, _f: &SearchFilter) -> Result<(Vec<Hit>, Mode)> {
    Err(todo("search::query"))
}

pub fn tag_pull(_cfg: &Config, _f: &SearchFilter) -> Result<Vec<Hit>> {
    Err(todo("search::tag_pull"))
}

pub fn health(_cfg: &Config) -> serde_json::Value {
    serde_json::json!({})
}

pub fn route(_cfg: &Config, _e: Engine) -> Mode {
    Mode::Builtin
}

pub fn resolve_effective_threshold(flag: Option<f64>, cfg: &Config) -> Option<f64> {
    match flag {
        Some(t) => Some(t),
        None if cfg.search.threshold_explicit => Some(cfg.search.threshold),
        None => None,
    }
}

pub fn corpus_rows(_cfg: &Config, _spaces: &[Space]) -> Vec<Row> {
    Vec::new()
}

pub fn indexed_available(_cfg: &Config) -> bool {
    false
}

pub fn reindex(_cfg: &Config, _roots: &[PathBuf]) -> Result<()> {
    Ok(())
}

pub fn index_update(_cfg: &Config, _path: &Path) -> Result<()> {
    Ok(())
}
