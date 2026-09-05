// STUB: owned by agent 5 (asset). Signatures are frozen; the bodies are not.
//! Asset ingest, sidecars, attach/detach and gc.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::domain::Filter;
use crate::error::{MeshError, Result};
use crate::fm::{Row, View};
use crate::model::asset::{AssetSidecar, AssetSummary, GcReport};

/// What `asset add` was asked to store.
#[derive(Clone, Debug, Default)]
pub struct NewAsset {
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub caption: String,
    pub attach: Option<String>,
}

/// The outcome of an ingest, including the content-address dedupe branch.
#[derive(Clone, Debug)]
pub struct AddOutcome {
    pub asset: AssetSidecar,
    pub deduplicated: bool,
}

fn todo(what: &str) -> MeshError {
    MeshError::Validation(format!("not implemented: {what}"))
}

pub fn add(_cfg: &Config, _src: &Path, _o: NewAsset) -> Result<AddOutcome> {
    Err(todo("assets::add"))
}

pub fn get(_cfg: &Config, _id: &str) -> Result<View<AssetSidecar>> {
    Err(todo("assets::get"))
}

pub fn blob_path(_cfg: &Config, _id: &str) -> Result<PathBuf> {
    Err(todo("assets::blob_path"))
}

pub fn list(_cfg: &Config, _f: &Filter, _media: Option<&str>) -> Result<Vec<View<AssetSidecar>>> {
    Err(todo("assets::list"))
}

pub fn attach(
    _cfg: &Config,
    _id: &str,
    _target: &str,
    _section: Option<&str>,
) -> Result<AssetSidecar> {
    Err(todo("assets::attach"))
}

pub fn detach(_cfg: &Config, _id: &str, _target: &str) -> Result<AssetSidecar> {
    Err(todo("assets::detach"))
}

pub fn remove(_cfg: &Config, _id: &str, _force: bool) -> Result<String> {
    Err(todo("assets::remove"))
}

pub fn gc(_cfg: &Config, _apply: bool) -> Result<GcReport> {
    Err(todo("assets::gc"))
}

pub fn references(_cfg: &Config, _id: &str) -> Vec<String> {
    Vec::new()
}

pub fn summary(_cfg: &Config) -> AssetSummary {
    AssetSummary::default()
}

pub fn rows(_cfg: &Config) -> Vec<Row> {
    Vec::new()
}
