// STUB: owned by agent 1 (note). Signatures are frozen; the bodies are not.
//! Note verbs.

use std::path::PathBuf;

use crate::config::Config;
use crate::error::{MeshError, Result};
use crate::fm::{Row, View};
use crate::model::note::{ForeignView, Note};

pub use crate::domain::AppendOpts;
use crate::domain::Filter;

/// What `note new` was asked to create.
#[derive(Clone, Debug, Default)]
pub struct NewNote {
    pub note_type: String,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub body: String,
}

/// What `note update` was asked to change.
#[derive(Clone, Debug, Default)]
pub struct UpdateNote {
    pub tags: Option<String>,
    pub new_type: Option<String>,
    pub title: Option<String>,
}

fn todo(what: &str) -> MeshError {
    MeshError::Validation(format!("not implemented: {what}"))
}

pub fn create(_cfg: &Config, _title: &str, _o: NewNote) -> Result<Note> {
    Err(todo("notes::create"))
}

pub fn append(_cfg: &Config, _target: &str, _text: &str, _o: AppendOpts) -> Result<Note> {
    Err(todo("notes::append"))
}

pub fn update(_cfg: &Config, _target: &str, _o: UpdateNote) -> Result<Note> {
    Err(todo("notes::update"))
}

pub fn get(_cfg: &Config, _target: &str) -> Result<View<Note>> {
    Err(todo("notes::get"))
}

pub fn get_foreign(_cfg: &Config, _target: &str) -> Result<ForeignView> {
    Err(todo("notes::get_foreign"))
}

pub fn list(_cfg: &Config, _f: &Filter, _foreign: bool) -> Result<Vec<View<Note>>> {
    Err(todo("notes::list"))
}

pub fn delete(_cfg: &Config, _target: &str) -> Result<String> {
    Err(todo("notes::delete"))
}

pub fn rows(_cfg: &Config) -> Vec<Row> {
    Vec::new()
}

pub fn resolve(_cfg: &Config, _target: &str) -> Result<PathBuf> {
    Err(todo("notes::resolve"))
}

pub fn find_duplicate_title(_cfg: &Config, _title: &str) -> Option<String> {
    None
}

pub fn note_folder(_cfg: &Config, _note_type: &str) -> Result<PathBuf> {
    Err(todo("notes::note_folder"))
}
