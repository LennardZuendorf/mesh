// STUB: owned by agent 3 (task). Signatures are frozen; the bodies are not.
//! The task dependency graph: derived readiness, edge mutation, cycles.

use crate::config::Config;
use crate::domain::Filter;
use crate::error::{MeshError, Result};
use crate::fm::{Row, View};
use crate::model::task::Task;

/// Whether a task is takeable, and why not when it is not.
#[derive(Clone, Debug, Default)]
pub struct Readiness {
    pub ready: bool,
    pub unsatisfied: Vec<String>,
    pub cycle: Option<Vec<String>>,
}

/// A best-effort mirror write that failed, reported on stderr rather than failing the verb.
#[derive(Clone, Debug)]
pub struct Warning(pub String);

fn todo(what: &str) -> MeshError {
    MeshError::Validation(format!("not implemented: {what}"))
}

pub fn effective_blockers(_rows: &[Row], _id: &str) -> Vec<String> {
    Vec::new()
}

pub fn readiness(_rows: &[Row], _id: &str) -> Readiness {
    Readiness::default()
}

pub fn dependents(_rows: &[Row], _id: &str) -> Vec<String> {
    Vec::new()
}

pub fn newly_ready(_rows: &[Row], _finished: &str) -> Vec<String> {
    Vec::new()
}

pub fn cycles(_rows: &[Row]) -> Vec<Vec<String>> {
    Vec::new()
}

pub fn dangling_blockers(_rows: &[Row]) -> Vec<String> {
    Vec::new()
}

pub fn check_acyclic(_rows: &[Row], _add: &[(String, String)]) -> Result<()> {
    Ok(())
}

pub fn block(_cfg: &Config, _id: &str, _on: &[String]) -> Result<(Task, Vec<Warning>)> {
    Err(todo("deps::block"))
}

pub fn unblock(
    _cfg: &Config,
    _id: &str,
    _on: &[String],
    _all: bool,
) -> Result<(Task, Vec<Warning>)> {
    Err(todo("deps::unblock"))
}

pub fn next(
    _cfg: &Config,
    _f: &Filter,
    _claim: bool,
    _strict: bool,
    _claimer: Option<&str>,
) -> Result<Option<View<Task>>> {
    Err(todo("deps::next"))
}
