// STUB: owned by agent 2 (memory). Signatures are frozen; the bodies are not.
//! Memory verbs, recall composition and expiry.

use chrono::{DateTime, Utc};

use crate::config::Config;
use crate::domain::{AppendOpts, Filter};
use crate::error::{MeshError, Result};
use crate::fm::{Row, View};
use crate::model::memory::{Memory, MemorySummary};
use crate::search::Hit;

/// What `memory new` was asked to create.
#[derive(Clone, Debug, Default)]
pub struct NewMemory {
    pub kind: String,
    pub scope: String,
    pub importance: Option<i64>,
    pub source: Option<String>,
    pub expires: Option<DateTime<Utc>>,
    pub supersedes: Option<String>,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub body: String,
}

/// What `memory update` was asked to change. `expires: Some(None)` clears it.
#[derive(Clone, Debug, Default)]
pub struct UpdateMemory {
    pub tags: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub scope: Option<String>,
    pub importance: Option<i64>,
    pub source: Option<String>,
    pub expires: Option<Option<DateTime<Utc>>>,
    pub owner: Option<String>,
}

/// The `memory list` switches that are not part of the shared `Filter`.
#[derive(Clone, Debug, Default)]
pub struct ListMemoryOpts {
    pub kind: Option<String>,
    pub scope: Option<String>,
    pub min_importance: Option<i64>,
    pub include_expired: bool,
    pub include_superseded: bool,
}

/// The `memory recall` switches.
#[derive(Clone, Debug, Default)]
pub struct RecallOpts {
    pub limit: i64,
    pub threshold: Option<f64>,
    pub decay: bool,
    pub include_expired: bool,
    pub min_importance: Option<i64>,
    pub meta_only: bool,
    pub full: bool,
}

fn todo(what: &str) -> MeshError {
    MeshError::Validation(format!("not implemented: {what}"))
}

pub fn create(_cfg: &Config, _title: &str, _o: NewMemory) -> Result<Memory> {
    Err(todo("memories::create"))
}

pub fn append(_cfg: &Config, _target: &str, _text: &str, _o: AppendOpts) -> Result<Memory> {
    Err(todo("memories::append"))
}

pub fn update(_cfg: &Config, _target: &str, _o: UpdateMemory) -> Result<Memory> {
    Err(todo("memories::update"))
}

pub fn get(_cfg: &Config, _target: &str) -> Result<View<Memory>> {
    Err(todo("memories::get"))
}

pub fn list(_cfg: &Config, _f: &Filter, _o: &ListMemoryOpts) -> Result<Vec<View<Memory>>> {
    Err(todo("memories::list"))
}

pub fn recall(_cfg: &Config, _query: &str, _f: &Filter, _o: &RecallOpts) -> Result<Vec<Hit>> {
    Err(todo("memories::recall"))
}

pub fn forget(_cfg: &Config, _target: &str) -> Result<String> {
    Err(todo("memories::forget"))
}

pub fn rows(_cfg: &Config) -> Vec<Row> {
    Vec::new()
}

pub fn summary(_cfg: &Config) -> MemorySummary {
    MemorySummary::default()
}

pub fn session_picks(_cfg: &Config, _me: Option<&str>, _cap: usize) -> Vec<View<Memory>> {
    Vec::new()
}

pub fn find_duplicate_title(_cfg: &Config, _title: &str) -> Option<String> {
    None
}
