// STUB: owned by agent 3 (task). Signatures are frozen; the bodies are not.
//! Task lifecycle verbs.

use crate::config::Config;
use crate::domain::{AppendOpts, Filter};
use crate::error::{MeshError, Result};
use crate::fm::{Row, View};
use crate::model::task::Task;

/// Which slice of the task list a caller wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Availability {
    #[default]
    Any,
    /// `status == open && claimed_by == null` — the Python meaning, dependency-blind.
    Available,
    /// Available and unblocked.
    Ready,
    /// Open or claimed with at least one unsatisfied blocker.
    Blocked,
}

/// The two terminal transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminal {
    Finish,
    Cancel,
}

/// What `task new` was asked to create.
#[derive(Clone, Debug, Default)]
pub struct NewTask {
    pub priority: Option<String>,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub body: String,
    pub project: Option<String>,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
}

/// What `task update` was asked to change.
#[derive(Clone, Debug, Default)]
pub struct UpdateTask {
    pub priority: Option<String>,
    pub tags: Option<String>,
    pub title: Option<String>,
    pub project: Option<String>,
    pub owner: Option<String>,
    pub blocks: Option<Vec<String>>,
    pub blocked_by: Option<Vec<String>>,
}

fn todo(what: &str) -> MeshError {
    MeshError::Validation(format!("not implemented: {what}"))
}

pub fn create(_cfg: &Config, _title: &str, _o: NewTask) -> Result<Task> {
    Err(todo("tasks::create"))
}

pub fn update(_cfg: &Config, _id: &str, _o: UpdateTask) -> Result<Task> {
    Err(todo("tasks::update"))
}

pub fn append(_cfg: &Config, _id: &str, _text: &str, _o: AppendOpts) -> Result<Task> {
    Err(todo("tasks::append"))
}

/// Returns the task and its unsatisfied blockers (empty when it was ready).
pub fn claim(
    _cfg: &Config,
    _id: &str,
    _claimer: &str,
    _strict: bool,
) -> Result<(Task, Vec<String>)> {
    Err(todo("tasks::claim"))
}

pub fn release(_cfg: &Config, _id: &str, _releaser: &str, _force: bool) -> Result<Task> {
    Err(todo("tasks::release"))
}

/// Returns the task and the ids that became ready — a report; nothing is written to them.
pub fn terminate(
    _cfg: &Config,
    _id: &str,
    _kind: Terminal,
    _text: Option<&str>,
    _actor: Option<&str>,
) -> Result<(Task, Vec<String>)> {
    Err(todo("tasks::terminate"))
}

pub fn get(_cfg: &Config, _id: &str) -> Result<View<Task>> {
    Err(todo("tasks::get"))
}

pub fn list(_cfg: &Config, _f: &Filter, _av: Availability) -> Result<Vec<View<Task>>> {
    Err(todo("tasks::list"))
}

pub fn delete(_cfg: &Config, _id: &str) -> Result<String> {
    Err(todo("tasks::delete"))
}

pub fn rows(_cfg: &Config) -> Vec<Row> {
    Vec::new()
}

pub fn find_duplicate_title(_cfg: &Config, _title: &str) -> Option<String> {
    None
}
