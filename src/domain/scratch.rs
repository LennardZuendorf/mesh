// STUB: owned by agent 4 (scratch). Signatures are frozen; the bodies are not.
//! Scratch verbs — name-addressed per-agent working state.

use crate::config::Config;
use crate::domain::{AppendOpts, Filter};
use crate::error::{MeshError, Result};
use crate::fm::View;
use crate::model::scratch::{Scratch, ScratchSummary};

fn todo(what: &str) -> MeshError {
    MeshError::Validation(format!("not implemented: {what}"))
}

pub fn set(_cfg: &Config, _agent: &str, _name: &str, _body: &str) -> Result<Scratch> {
    Err(todo("scratch::set"))
}

pub fn append(
    _cfg: &Config,
    _agent: &str,
    _name: &str,
    _text: &str,
    _o: AppendOpts,
) -> Result<Scratch> {
    Err(todo("scratch::append"))
}

pub fn get(_cfg: &Config, _agent: &str, _name: &str) -> Result<View<Scratch>> {
    Err(todo("scratch::get"))
}

pub fn list(
    _cfg: &Config,
    _agent: Option<&str>,
    _all: bool,
    _f: &Filter,
) -> Result<Vec<View<Scratch>>> {
    Err(todo("scratch::list"))
}

pub fn clear(_cfg: &Config, _agent: &str, _name: &str) -> Result<String> {
    Err(todo("scratch::clear"))
}

pub fn summary(_cfg: &Config) -> ScratchSummary {
    ScratchSummary::default()
}
