//! Tool dispatch: the 37 names, wired to the same domain seams the CLI calls.
//!
//! Every tool builds a `Ctx` (never a tty, always machine mode) around a freshly loaded
//! config, so a config that appears after startup is picked up on the next call and a missing
//! one is a per-call `config_missing` envelope rather than a dead server. Nothing here
//! re-implements domain logic; the tools are adapters over `domain::*` and `search::*`.

use std::path::PathBuf;

use serde_json::{Map, Value as Json};

use crate::cli::globals::GlobalOpts;
use crate::cli::task::TASK_SORT_KEYS;
use crate::config::load_config;
use crate::ctx::Ctx;
use crate::domain::deps;
use crate::domain::memories::{self, ListMemoryOpts, NewMemory, RecallOpts, UpdateMemory};
use crate::domain::notes::{self, NewNote, UpdateNote};
use crate::domain::scratch as scratch_domain;
use crate::domain::tasks::{self, Availability, NewTask, Terminal, UpdateTask};
use crate::domain::{activity, assets, context, lenses};
use crate::domain::{AppendOpts, Filter, SortKey};
use crate::error::{MeshError, Result};
use crate::fm::View;
use crate::mcp::{errors, instructions, schema};
use crate::model::asset::ASSET_FIELDS;
use crate::model::memory::MEMORY_FIELDS;
use crate::model::note::NOTE_FIELDS;
use crate::model::scratch::SCRATCH_FIELDS;
use crate::model::task::{Task, TASK_FIELDS};
use crate::render::entry;
use crate::search::{self, Engine, SearchFilter};
use crate::spaces::Space;

/// How many memories `session-start` picks.
const SESSION_MEMORY_CAP: usize = 5;
/// The activity window `session-start` reads.
const SESSION_ACTIVITY_LIMIT: i64 = 20;
/// The sort keys `note list` accepts.
const NOTE_SORTS: [SortKey; 3] = [SortKey::Updated, SortKey::Created, SortKey::Title];
/// The sort keys `memory list` accepts.
const MEMORY_SORTS: [SortKey; 4] = [
    SortKey::Updated,
    SortKey::Created,
    SortKey::Title,
    SortKey::Importance,
];
/// The sort keys `asset list` accepts.
const ASSET_SORTS: [SortKey; 4] = [
    SortKey::Updated,
    SortKey::Created,
    SortKey::Title,
    SortKey::Bytes,
];
/// The `graph` directions.
const DIRECTIONS: [&str; 3] = ["out", "in", "both"];

/// A tool's return value plus whether it needs the list wrapper.
#[derive(Debug)]
pub struct Outcome {
    pub value: Json,
    pub list: bool,
}

fn object(value: Json) -> Outcome {
    Outcome { value, list: false }
}

fn rows(values: Vec<Json>) -> Outcome {
    Outcome {
        value: Json::Array(values),
        list: true,
    }
}

// ---------------------------------------------------------------------------------------
// argument access
// ---------------------------------------------------------------------------------------

/// A tool call's arguments, typed on read.
struct Args<'a> {
    raw: &'a Json,
}

impl<'a> Args<'a> {
    fn new(raw: &'a Json) -> Args<'a> {
        Args { raw }
    }

    /// The value at `key`, treating an absent key and an explicit `null` alike.
    fn get(&self, key: &str) -> Option<&Json> {
        match self.raw.get(key) {
            None | Some(Json::Null) => None,
            Some(value) => Some(value),
        }
    }

    fn str(&self, key: &str) -> Result<Option<String>> {
        match self.get(key) {
            None => Ok(None),
            Some(Json::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(errors::wrong_type(key, "a string")),
        }
    }

    fn req_str(&self, key: &str) -> Result<String> {
        self.str(key)?.ok_or_else(|| errors::missing(key))
    }

    fn str_or(&self, key: &str, default: &str) -> Result<String> {
        Ok(self.str(key)?.unwrap_or_else(|| default.to_string()))
    }

    fn flag(&self, key: &str) -> Result<bool> {
        match self.get(key) {
            None => Ok(false),
            Some(Json::Bool(b)) => Ok(*b),
            Some(_) => Err(errors::wrong_type(key, "a boolean")),
        }
    }

    fn int(&self, key: &str) -> Result<Option<i64>> {
        match self.get(key) {
            None => Ok(None),
            Some(value) => value
                .as_i64()
                .map(Some)
                .ok_or_else(|| errors::wrong_type(key, "an integer")),
        }
    }

    fn int_or(&self, key: &str, default: i64) -> Result<i64> {
        Ok(self.int(key)?.unwrap_or(default))
    }

    fn num(&self, key: &str) -> Result<Option<f64>> {
        match self.get(key) {
            None => Ok(None),
            Some(value) => value
                .as_f64()
                .map(Some)
                .ok_or_else(|| errors::wrong_type(key, "a number")),
        }
    }

    fn list(&self, key: &str) -> Result<Option<Vec<String>>> {
        match self.get(key) {
            None => Ok(None),
            Some(Json::Array(items)) => {
                let mut out: Vec<String> = Vec::new();
                for item in items {
                    match item {
                        Json::String(s) => out.push(s.clone()),
                        _ => return Err(errors::wrong_type(key, "an array of strings")),
                    }
                }
                Ok(Some(out))
            }
            Some(_) => Err(errors::wrong_type(key, "an array of strings")),
        }
    }

    fn list_or_empty(&self, key: &str) -> Result<Vec<String>> {
        Ok(self.list(key)?.unwrap_or_default())
    }

    fn req_list(&self, key: &str) -> Result<Vec<String>> {
        let values = self.list(key)?.ok_or_else(|| errors::missing(key))?;
        if values.is_empty() {
            return Err(MeshError::Validation(format!(
                "parameter '{key}' must name at least one id"
            )));
        }
        Ok(values)
    }
}

// ---------------------------------------------------------------------------------------
// the server
// ---------------------------------------------------------------------------------------

/// The stdio server's state: where the config lives, and the instructions built from it once.
pub struct Server {
    config_flag: Option<PathBuf>,
    vault_flag: Option<PathBuf>,
    instructions: String,
}

impl Server {
    /// Build the server. A config that will not load is not fatal: the instructions degrade
    /// and every tool call reports `config_missing` in its own envelope.
    pub fn new(config_flag: Option<PathBuf>, vault_flag: Option<PathBuf>) -> Server {
        let loaded = load_config(config_flag.as_deref(), vault_flag.as_deref()).ok();
        let instructions = instructions::build(loaded.as_ref());
        Server {
            config_flag,
            vault_flag,
            instructions,
        }
    }

    /// The instructions block sent with `initialize`.
    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    /// A per-call context: machine mode, never a tty, acting as `owner` when one was passed.
    fn ctx(&self, owner: Option<String>) -> Result<Ctx> {
        let config = load_config(self.config_flag.as_deref(), self.vault_flag.as_deref())?;
        let globals = GlobalOpts {
            json: true,
            quiet: false,
            owner,
            mine: false,
            config: self.config_flag.clone(),
            vault: self.vault_flag.clone(),
        };
        Ok(Ctx::with_config(globals, config, false))
    }

    /// Dispatch one `tools/call`.
    pub fn call(&self, name: &str, arguments: &Json) -> Result<Outcome> {
        if schema::find(name).is_none() {
            return Err(errors::unknown_tool(name));
        }
        let a = Args::new(arguments);
        match name {
            "mesh_note_get" => self.note_get(&a),
            "mesh_note_list" => self.note_list(&a),
            "mesh_task_get" => self.task_get(&a),
            "mesh_task_list" => self.task_list(&a),
            "mesh_search" => self.search(&a),
            "mesh_health" => self.health(),
            "mesh_recent_activity" => self.recent_activity(&a),
            "mesh_build_context" => self.build_context(&a),
            "mesh_graph" => self.graph(&a),
            "mesh_project" => self.project(&a),
            "mesh_session_start" => self.session_start(&a),
            "mesh_note_new" => self.note_new(&a),
            "mesh_note_append" => self.note_append(&a),
            "mesh_task_new" => self.task_new(&a),
            "mesh_task_append" => self.task_append(&a),
            "mesh_note_update" => self.note_update(&a),
            "mesh_task_claim" => self.task_claim(&a),
            "mesh_task_release" => self.task_release(&a),
            "mesh_task_finish" => self.terminate(&a, Terminal::Finish),
            "mesh_task_update" => self.task_update(&a),
            "mesh_task_cancel" => self.terminate(&a, Terminal::Cancel),
            "mesh_memory_new" => self.memory_new(&a),
            "mesh_memory_append" => self.memory_append(&a),
            "mesh_memory_update" => self.memory_update(&a),
            "mesh_memory_get" => self.memory_get(&a),
            "mesh_memory_list" => self.memory_list(&a),
            "mesh_memory_recall" => self.memory_recall(&a),
            "mesh_scratch_set" => self.scratch_set(&a),
            "mesh_scratch_append" => self.scratch_append(&a),
            "mesh_scratch_get" => self.scratch_get(&a),
            "mesh_scratch_list" => self.scratch_list(&a),
            "mesh_asset_get" => self.asset_get(&a),
            "mesh_asset_list" => self.asset_list(&a),
            "mesh_asset_attach" => self.asset_attach(&a),
            "mesh_task_block" => self.task_block(&a),
            "mesh_task_unblock" => self.task_unblock(&a),
            "mesh_task_next" => self.task_next(&a),
            other => Err(errors::unknown_tool(other)),
        }
    }

    // -----------------------------------------------------------------------------------
    // notes
    // -----------------------------------------------------------------------------------

    fn note_get(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let view = notes::get(ctx.cfg()?, &a.req_str("id")?)?;
        Ok(object(entry(
            &view.item.meta,
            NOTE_FIELDS.fields(),
            Some(&view.body),
            Some(&view.path),
        )))
    }

    fn note_list(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let cfg = ctx.cfg()?;
        let filter = Filter {
            tags: a.list("tags")?,
            any_tag: a.flag("any_tag")?,
            owner: a.str("owner")?,
            mine: false,
            me: cfg.agent().map(str::to_string),
            cutoff: cutoff(a, "since")?,
            stale_cutoff: None,
            sort: SortKey::parse(&a.str_or("sort", "updated")?, &NOTE_SORTS)?,
            limit: Some(a.int_or("limit", 20)?),
            extra: Vec::new(),
        }
        .with_extra("type", a.str("note_type")?.as_deref());
        let views = notes::list(cfg, &filter, false)?;
        Ok(rows(
            views
                .iter()
                .map(|v| entry(&v.item.meta, NOTE_FIELDS.fields(), None, Some(&v.path)))
                .collect(),
        ))
    }

    fn note_new(&self, a: &Args) -> Result<Outcome> {
        let owner = a.str("owner")?;
        let ctx = self.ctx(owner.clone())?;
        let cfg = ctx.cfg()?;
        let title = a.req_str("title")?;
        // The advisory names the *prior* id, so it is computed before the create.
        let duplicate = notes::find_duplicate_title(cfg, &title);
        let note = notes::create(
            cfg,
            &title,
            NewNote {
                note_type: a.str_or("note_type", "note")?,
                tags: a.list_or_empty("tags")?,
                owner,
                body: a.str_or("body", "")?,
            },
        )?;
        Ok(object(with_warnings(
            entry(&note.meta, NOTE_FIELDS.fields(), None, None),
            duplicate_warning(duplicate.as_deref()),
        )))
    }

    fn note_append(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let opts = AppendOpts {
            section: a.str("section")?,
            timestamp: a.flag("timestamp")?,
            actor: ctx.actor().map(str::to_string),
        };
        let note = notes::append(ctx.cfg()?, &a.req_str("target")?, &a.req_str("text")?, opts)?;
        Ok(object(entry(&note.meta, NOTE_FIELDS.fields(), None, None)))
    }

    fn note_update(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let note = notes::update(
            ctx.cfg()?,
            &a.req_str("target")?,
            UpdateNote {
                tags: a.str("tags")?,
                new_type: a.str("new_type")?,
                title: None,
            },
        )?;
        Ok(object(entry(&note.meta, NOTE_FIELDS.fields(), None, None)))
    }

    // -----------------------------------------------------------------------------------
    // tasks
    // -----------------------------------------------------------------------------------

    fn task_get(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let cfg = ctx.cfg()?;
        let id = a.req_str("id")?;
        let view = tasks::get(cfg, &id)?;
        let ready = deps::readiness(&tasks::rows(cfg), &id).ready;
        let mut payload = entry(
            &view.item.meta,
            TASK_FIELDS.fields(),
            Some(&view.body),
            Some(&view.path),
        );
        if let Some(map) = payload.as_object_mut() {
            map.insert("ready".to_string(), Json::Bool(ready));
        }
        Ok(object(payload))
    }

    fn task_list(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let cfg = ctx.cfg()?;
        let (ready, blocked, available) =
            (a.flag("ready")?, a.flag("blocked")?, a.flag("available")?);
        let availability = if ready {
            Availability::Ready
        } else if blocked {
            Availability::Blocked
        } else if available {
            Availability::Available
        } else {
            Availability::Any
        };
        let sort_name = match a.str("sort")? {
            Some(value) => value,
            None if available || ready => "priority".to_string(),
            None => "updated".to_string(),
        };
        let limit = a.int_or("limit", 20)?;
        let statuses = match a.str("status")? {
            Some(value) => tasks::parse_status_csv(&value)?,
            None => None,
        };
        let filter = Filter {
            tags: a.list("tags")?,
            any_tag: a.flag("any_tag")?,
            owner: a.str("owner")?,
            mine: a.flag("mine")?,
            me: cfg.agent().map(str::to_string),
            cutoff: cutoff(a, "since")?,
            stale_cutoff: cutoff(a, "stale")?,
            sort: SortKey::parse(&sort_name, &TASK_SORT_KEYS)?,
            // A status union is a membership set the shared filter cannot express, so it and
            // the limit are both applied after the select.
            limit: if statuses.is_some() {
                None
            } else {
                Some(limit)
            },
            extra: Vec::new(),
        }
        .with_extra("project", a.str("project")?.as_deref());

        let views = tasks::list(cfg, &filter, availability)?;
        let views: Vec<View<Task>> = match &statuses {
            Some(wanted) => {
                let mut kept: Vec<View<Task>> = views
                    .into_iter()
                    .filter(|v| wanted.iter().any(|s| s == &v.item.status))
                    .collect();
                if limit >= 0 {
                    kept.truncate(usize::try_from(limit).unwrap_or(0));
                }
                kept
            }
            None => views,
        };
        Ok(rows(
            views
                .iter()
                .map(|v| entry(&v.item.meta, TASK_FIELDS.fields(), None, Some(&v.path)))
                .collect(),
        ))
    }

    fn task_new(&self, a: &Args) -> Result<Outcome> {
        let owner = a.str("owner")?;
        let ctx = self.ctx(owner.clone())?;
        let cfg = ctx.cfg()?;
        let title = a.req_str("title")?;
        let duplicate = tasks::find_duplicate_title(cfg, &title);
        let task = tasks::create(
            cfg,
            &title,
            NewTask {
                priority: a.str("priority")?,
                tags: a.list_or_empty("tags")?,
                owner,
                body: a.str_or("body", "")?,
                project: a.str("project")?,
                blocks: a.list_or_empty("blocks")?,
                blocked_by: a.list_or_empty("blocked_by")?,
            },
        )?;
        Ok(object(with_warnings(
            entry(&task.meta, TASK_FIELDS.fields(), None, None),
            duplicate_warning(duplicate.as_deref()),
        )))
    }

    fn task_append(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let opts = AppendOpts {
            section: a.str("section")?,
            timestamp: a.flag("timestamp")?,
            actor: ctx.actor().map(str::to_string),
        };
        let task = tasks::append(
            ctx.cfg()?,
            &a.req_str("task_id")?,
            &a.req_str("text")?,
            opts,
        )?;
        Ok(object(entry(&task.meta, TASK_FIELDS.fields(), None, None)))
    }

    fn task_update(&self, a: &Args) -> Result<Outcome> {
        // `owner` here reassigns accountability; it is not the acting identity.
        let ctx = self.ctx(None)?;
        let task = tasks::update(
            ctx.cfg()?,
            &a.req_str("task_id")?,
            UpdateTask {
                priority: a.str("priority")?,
                tags: a.str("tags")?,
                title: a.str("title")?,
                project: a.str("project")?,
                owner: a.str("owner")?,
                blocks: a.list("blocks")?,
                blocked_by: a.list("blocked_by")?,
            },
        )?;
        Ok(object(entry(&task.meta, TASK_FIELDS.fields(), None, None)))
    }

    fn task_claim(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(a.str("claimer")?)?;
        let cfg = ctx.cfg()?;
        let id = a.req_str("task_id")?;
        let who = ctx.actor().map(str::to_string).ok_or_else(|| {
            MeshError::Validation("no agent identity: pass claimer or set [core].agent".to_string())
        })?;
        let strict = a.flag("strict")? || cfg.tasks.strict;
        let (task, unsatisfied) = tasks::claim(cfg, &id, &who, strict)?;
        let mut payload = entry(&task.meta, TASK_FIELDS.fields(), None, None);
        if !unsatisfied.is_empty() {
            if let Some(map) = payload.as_object_mut() {
                map.insert("blocked_by_unsatisfied".to_string(), strings(&unsatisfied));
            }
        }
        Ok(object(payload))
    }

    fn task_release(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(a.str("owner")?)?;
        let who = ctx.actor().map(str::to_string).ok_or_else(|| {
            MeshError::Validation("no agent identity: pass owner or set [core].agent".to_string())
        })?;
        // No `force`: breaking a peer's claim stays a human, CLI-only act.
        let task = tasks::release(ctx.cfg()?, &a.req_str("task_id")?, &who, false)?;
        Ok(object(entry(&task.meta, TASK_FIELDS.fields(), None, None)))
    }

    fn terminate(&self, a: &Args, kind: Terminal) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let text = match kind {
            Terminal::Finish => a.str("outcome")?,
            Terminal::Cancel => a.str("reason")?,
        };
        let actor = ctx.actor().map(str::to_string);
        let (task, unblocked) = tasks::terminate(
            ctx.cfg()?,
            &a.req_str("task_id")?,
            kind,
            text.as_deref(),
            actor.as_deref(),
        )?;
        let mut payload = entry(&task.meta, TASK_FIELDS.fields(), None, None);
        if !unblocked.is_empty() {
            if let Some(map) = payload.as_object_mut() {
                map.insert("unblocked".to_string(), strings(&unblocked));
            }
        }
        Ok(object(payload))
    }

    fn task_block(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let (task, _warnings) =
            deps::block(ctx.cfg()?, &a.req_str("task_id")?, &a.req_list("on")?)?;
        Ok(object(entry(&task.meta, TASK_FIELDS.fields(), None, None)))
    }

    fn task_unblock(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let all = a.flag("all")?;
        let on = a.list_or_empty("on")?;
        if on.is_empty() && !all {
            return Err(MeshError::Validation(
                "pass on with task ids, or all".to_string(),
            ));
        }
        let (task, _warnings) = deps::unblock(ctx.cfg()?, &a.req_str("task_id")?, &on, all)?;
        Ok(object(entry(&task.meta, TASK_FIELDS.fields(), None, None)))
    }

    fn task_next(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let cfg = ctx.cfg()?;
        let claim = a.flag("claim")?;
        let filter = Filter {
            tags: a.list("tags")?,
            any_tag: false,
            owner: None,
            mine: a.flag("mine")?,
            me: cfg.agent().map(str::to_string),
            cutoff: None,
            stale_cutoff: None,
            sort: SortKey::Priority,
            limit: None,
            extra: Vec::new(),
        }
        .with_extra("project", a.str("project")?.as_deref());
        let claimer = if claim {
            Some(ctx.actor().map(str::to_string).ok_or_else(|| {
                MeshError::Validation(
                    "no agent identity: set [core].agent or pass --owner".to_string(),
                )
            })?)
        } else {
            None
        };
        let strict = a.flag("strict")? || cfg.tasks.strict;
        let picked = deps::next(cfg, &filter, claim, strict, claimer.as_deref())?;
        let Some(view) = picked else {
            return Err(MeshError::Empty(
                crate::cli::task_dep::NO_READY_TASK.to_string(),
            ));
        };
        Ok(object(entry(
            &view.item.meta,
            TASK_FIELDS.fields(),
            None,
            Some(&view.path),
        )))
    }

    // -----------------------------------------------------------------------------------
    // memories
    // -----------------------------------------------------------------------------------

    fn memory_new(&self, a: &Args) -> Result<Outcome> {
        let owner = a.str("owner")?;
        let ctx = self.ctx(owner.clone())?;
        let cfg = ctx.cfg()?;
        let title = a.req_str("title")?;
        let duplicate = memories::find_duplicate_title(cfg, &title);
        let expires = match a.str("expires")? {
            Some(value) => Some(memories::parse_expires(&value)?),
            None => None,
        };
        let (memory, mut warnings) = memories::create_with_warnings(
            cfg,
            &title,
            NewMemory {
                kind: a.str_or("kind", "fact")?,
                scope: a.str_or("scope", "shared")?,
                importance: a.int("importance")?,
                source: a.str("source")?,
                expires,
                supersedes: a.str("supersedes")?,
                tags: a.list_or_empty("tags")?,
                owner,
                body: a.str_or("body", "")?,
            },
        )?;
        let mut all = duplicate_warning(duplicate.as_deref());
        all.append(&mut warnings);
        Ok(object(with_warnings(
            entry(&memory.meta, MEMORY_FIELDS.fields(), None, None),
            all,
        )))
    }

    fn memory_append(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let opts = AppendOpts {
            section: a.str("section")?,
            timestamp: a.flag("timestamp")?,
            actor: ctx.actor().map(str::to_string),
        };
        let memory =
            memories::append(ctx.cfg()?, &a.req_str("target")?, &a.req_str("text")?, opts)?;
        Ok(object(entry(
            &memory.meta,
            MEMORY_FIELDS.fields(),
            None,
            None,
        )))
    }

    fn memory_update(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        // `none` clears the expiry; anything else parses as a duration or an ISO datetime.
        let expires = match a.str("expires")? {
            None => None,
            Some(value) if value.eq_ignore_ascii_case(memories::EXPIRES_NONE) => Some(None),
            Some(value) => Some(Some(memories::parse_expires(&value)?)),
        };
        let memory = memories::update(
            ctx.cfg()?,
            &a.req_str("target")?,
            UpdateMemory {
                tags: a.str("tags")?,
                title: a.str("title")?,
                kind: a.str("kind")?,
                scope: a.str("scope")?,
                importance: a.int("importance")?,
                source: a.str("source")?,
                expires,
                owner: a.str("owner")?,
            },
        )?;
        Ok(object(entry(
            &memory.meta,
            MEMORY_FIELDS.fields(),
            None,
            None,
        )))
    }

    fn memory_get(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let view = memories::get(ctx.cfg()?, &a.req_str("target")?)?;
        Ok(object(entry(
            &view.item.meta,
            MEMORY_FIELDS.fields(),
            Some(&view.body),
            Some(&view.path),
        )))
    }

    fn memory_list(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let cfg = ctx.cfg()?;
        let filter = Filter {
            tags: a.list("tags")?,
            any_tag: a.flag("any_tag")?,
            owner: a.str("owner")?,
            mine: a.flag("mine")?,
            me: cfg.agent().map(str::to_string),
            cutoff: cutoff(a, "since")?,
            stale_cutoff: None,
            sort: SortKey::parse(&a.str_or("sort", "updated")?, &MEMORY_SORTS)?,
            limit: Some(a.int_or("limit", 20)?),
            extra: Vec::new(),
        };
        let opts = ListMemoryOpts {
            kind: a.str("kind")?,
            scope: a.str("scope")?,
            min_importance: a.int("min_importance")?,
            include_expired: a.flag("include_expired")?,
            include_superseded: a.flag("include_superseded")?,
        };
        let views = memories::list(cfg, &filter, &opts)?;
        Ok(rows(
            views
                .iter()
                .map(|v| entry(&v.item.meta, MEMORY_FIELDS.fields(), None, Some(&v.path)))
                .collect(),
        ))
    }

    fn memory_recall(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let cfg = ctx.cfg()?;
        let meta_only = a.flag("meta_only")?;
        let full = a.flag("full")?;
        let filter = Filter {
            tags: a.list("tags")?,
            any_tag: false,
            owner: a.str("owner")?,
            mine: a.flag("mine")?,
            me: cfg.agent().map(str::to_string),
            cutoff: None,
            stale_cutoff: None,
            sort: SortKey::Updated,
            limit: None,
            extra: Vec::new(),
        }
        .with_extra("kind", a.str("kind")?.as_deref());
        let opts = RecallOpts {
            limit: a.int_or("limit", 10)?,
            threshold: a.num("threshold")?,
            decay: !a.flag("no_decay")?,
            include_expired: a.flag("include_expired")?,
            min_importance: a.int("min_importance")?,
            meta_only,
            full,
        };
        let hits = memories::recall(cfg, &a.req_str("query")?, &filter, &opts)?;
        let space_key = search::emit_space_key(cfg, &[Space::Memories], false);
        Ok(rows(
            hits.iter()
                .map(|h| crate::render::hit(h, meta_only, full, space_key.then_some(h.space)))
                .collect(),
        ))
    }

    // -----------------------------------------------------------------------------------
    // scratch
    // -----------------------------------------------------------------------------------

    fn scratch_agent(&self, ctx: &Ctx, a: &Args) -> Result<String> {
        if let Some(agent) = a.str("agent")? {
            return Ok(agent);
        }
        ctx.actor().map(str::to_string).ok_or_else(|| {
            MeshError::Validation("no agent identity: pass agent or set [core].agent".to_string())
        })
    }

    fn scratch_set(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let agent = self.scratch_agent(&ctx, a)?;
        let scratch =
            scratch_domain::set(ctx.cfg()?, &agent, &a.req_str("name")?, &a.req_str("body")?)?;
        Ok(object(entry(
            &scratch.meta,
            SCRATCH_FIELDS.fields(),
            None,
            None,
        )))
    }

    fn scratch_append(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let agent = self.scratch_agent(&ctx, a)?;
        let opts = AppendOpts {
            section: a.str("section")?,
            timestamp: a.flag("timestamp")?,
            actor: ctx.actor().map(str::to_string),
        };
        let scratch = scratch_domain::append(
            ctx.cfg()?,
            &agent,
            &a.req_str("name")?,
            &a.req_str("text")?,
            opts,
        )?;
        Ok(object(entry(
            &scratch.meta,
            SCRATCH_FIELDS.fields(),
            None,
            None,
        )))
    }

    fn scratch_get(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let agent = self.scratch_agent(&ctx, a)?;
        let view = scratch_domain::get(ctx.cfg()?, &agent, &a.req_str("name")?)?;
        Ok(object(entry(
            &view.item.meta,
            SCRATCH_FIELDS.fields(),
            Some(&view.body),
            Some(&view.path),
        )))
    }

    fn scratch_list(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let all = a.flag("all_agents")?;
        let agent = if all {
            None
        } else {
            Some(self.scratch_agent(&ctx, a)?)
        };
        let filter = Filter {
            cutoff: cutoff(a, "since")?,
            ..Filter::unbounded()
        };
        let views = scratch_domain::list(ctx.cfg()?, agent.as_deref(), all, &filter)?;
        Ok(rows(
            views
                .iter()
                .map(|v| entry(&v.item.meta, SCRATCH_FIELDS.fields(), None, Some(&v.path)))
                .collect(),
        ))
    }

    // -----------------------------------------------------------------------------------
    // assets
    // -----------------------------------------------------------------------------------

    fn asset_get(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let view = assets::get(ctx.cfg()?, &a.req_str("asset_id")?)?;
        Ok(object(entry(
            &view.item.meta,
            ASSET_FIELDS.fields(),
            Some(&view.body),
            Some(&view.path),
        )))
    }

    fn asset_list(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let cfg = ctx.cfg()?;
        let filter = Filter {
            tags: a.list("tags")?,
            any_tag: a.flag("any_tag")?,
            owner: a.str("owner")?,
            mine: a.flag("mine")?,
            me: cfg.agent().map(str::to_string),
            cutoff: cutoff(a, "since")?,
            stale_cutoff: None,
            sort: SortKey::parse(&a.str_or("sort", "updated")?, &ASSET_SORTS)?,
            limit: Some(a.int_or("limit", 20)?),
            extra: Vec::new(),
        };
        let views = assets::list(cfg, &filter, a.str("media_type")?.as_deref())?;
        Ok(rows(
            views
                .iter()
                .map(|v| entry(&v.item.meta, ASSET_FIELDS.fields(), None, Some(&v.path)))
                .collect(),
        ))
    }

    fn asset_attach(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let sidecar = assets::attach(
            ctx.cfg()?,
            &a.req_str("asset_id")?,
            &a.req_str("target")?,
            a.str("section")?.as_deref(),
        )?;
        Ok(object(entry(
            &sidecar.meta,
            ASSET_FIELDS.fields(),
            None,
            None,
        )))
    }

    // -----------------------------------------------------------------------------------
    // search and the lenses
    // -----------------------------------------------------------------------------------

    fn search(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let cfg = ctx.cfg()?;
        let engine = match a.str("engine")? {
            Some(value) => Engine::parse(&value)?,
            None => Engine::Auto,
        };
        let requested = a.list("spaces")?;
        let csv = requested.as_ref().map(|s| s.join(","));
        let spaces = search::resolve_spaces(cfg, csv.as_deref())?;
        let meta_only = a.flag("meta_only")?;
        let full = a.flag("full")?;
        let filter = SearchFilter {
            spaces: spaces.clone(),
            type_filter: a.str("type_filter")?,
            tags: a.list_or_empty("tags")?,
            owner: a.str("owner")?,
            status: a.str("status")?,
            kind: a.str("kind")?,
            limit: a.int_or("limit", 10)?,
            threshold: search::resolve_effective_threshold(a.num("threshold")?, cfg),
            engine,
            // MCP has no stderr an agent reads; the per-hit `mode` key is that channel.
            quiet: true,
        };
        let (mut hits, mode) = match a.str("query")? {
            None => (search::tag_pull(cfg, &filter)?, None),
            Some(query) => {
                let (hits, mode) = search::query(cfg, &query, &filter)?;
                (hits, Some(mode))
            }
        };
        if full && !meta_only {
            search::fill_full_bodies(&mut hits);
        }
        let space_key = search::emit_space_key(cfg, &spaces, requested.is_some());
        let payload: Vec<Json> = hits
            .iter()
            .map(|h| {
                let mut value =
                    crate::render::hit(h, meta_only, full, space_key.then_some(h.space));
                // `mode` is observed from the branch that answered, never predicted, and it
                // is appended after every other key.
                if let (Some(mode), Some(map)) = (mode, value.as_object_mut()) {
                    map.insert("mode".to_string(), Json::String(mode.name().to_string()));
                }
                value
            })
            .collect();
        Ok(rows(payload))
    }

    fn health(&self) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        Ok(object(search::health(ctx.cfg()?)))
    }

    fn recent_activity(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let values = activity::recent_activity(
            ctx.cfg()?,
            a.str("since")?.as_deref(),
            a.str("owner")?.as_deref(),
            a.flag("mine")?,
            a.int_or("limit", SESSION_ACTIVITY_LIMIT)?,
        )?;
        Ok(rows(values))
    }

    fn build_context(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        let values =
            context::build_context(ctx.cfg()?, &a.req_str("seed_id")?, a.int_or("depth", 1)?)?;
        Ok(rows(values))
    }

    fn graph(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        // The direction is validated before the seed resolves: a bad direction on a missing
        // seed is a validation failure, not a not-found.
        let direction = a.str_or("direction", "out")?;
        if !DIRECTIONS.contains(&direction.as_str()) {
            return Err(MeshError::Validation(format!(
                "invalid direction: '{direction}' (use out, in, both)"
            )));
        }
        let result = context::graph_query(
            ctx.cfg()?,
            &a.req_str("seed_id")?,
            a.int_or("depth", 1)?,
            &direction,
        )?;
        Ok(object(result.to_json()))
    }

    fn project(&self, a: &Args) -> Result<Outcome> {
        let ctx = self.ctx(None)?;
        Ok(object(lenses::project_view(
            ctx.cfg()?,
            &a.req_str("project_id")?,
        )?))
    }

    fn session_start(&self, a: &Args) -> Result<Outcome> {
        let owner = a.str("owner")?;
        let ctx = self.ctx(owner.clone())?;
        // The effective identity substitutes for the caller's own across every source.
        let cfg = ctx.cfg()?.with_agent(owner.as_deref());
        let me = cfg.agent().map(str::to_string);
        let team = a.flag("team")?;
        let meta_only = a.flag("meta_only")?;

        let task_filter = Filter {
            mine: true,
            me: me.clone(),
            limit: None,
            ..Filter::default()
        };
        let task_views = tasks::list(&cfg, &task_filter, Availability::Any)?;
        let note_views = match &me {
            None => Vec::new(),
            Some(agent) => notes::list(
                &cfg,
                &Filter {
                    owner: Some(agent.clone()),
                    me: me.clone(),
                    limit: None,
                    ..Filter::default()
                },
                false,
            )?,
        };
        let mentions = lenses::session_mentions(
            &cfg,
            &task_views,
            &note_views,
            me.as_deref(),
            lenses::SESSION_SINCE,
        );
        let memories = if a.flag("no_memories")? {
            Vec::new()
        } else {
            memories::session_picks(&cfg, me.as_deref(), SESSION_MEMORY_CAP)
        };
        let activity = activity::recent_activity(
            &cfg,
            Some(lenses::SESSION_SINCE),
            None,
            !team,
            SESSION_ACTIVITY_LIMIT,
        )?;
        let budget = usize::try_from(a.int_or("budget", 0)?).unwrap_or(0);
        Ok(rows(lenses::session_start_entries(
            &cfg,
            &task_views,
            mentions,
            memories,
            activity,
            meta_only,
            budget,
        )))
    }
}

// ---------------------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------------------

fn cutoff(a: &Args, key: &str) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    match a.str(key)? {
        Some(value) => Ok(Some(crate::timefmt::parse_since(&value)?)),
        None => Ok(None),
    }
}

fn strings(values: &[String]) -> Json {
    Json::Array(values.iter().map(|v| Json::String(v.clone())).collect())
}

fn duplicate_warning(existing: Option<&str>) -> Vec<String> {
    match existing {
        Some(id) => vec![format!("duplicate title, also used by {id}")],
        None => Vec::new(),
    }
}

/// Append the `warnings` array a creation tool always carries, empty or not.
fn with_warnings(payload: Json, warnings: Vec<String>) -> Json {
    let mut map = match payload {
        Json::Object(map) => map,
        other => {
            let mut map = Map::new();
            map.insert("value".to_string(), other);
            map
        }
    };
    map.insert(
        "warnings".to_string(),
        Json::Array(warnings.into_iter().map(Json::String).collect()),
    );
    Json::Object(map)
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

    fn args(value: Json) -> Json {
        value
    }

    #[test]
    fn a_missing_required_argument_is_a_validation_error() {
        let raw = args(serde_json::json!({}));
        let a = Args::new(&raw);
        let err = a.req_str("id").unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(err.to_string(), "missing required parameter: 'id'");
    }

    #[test]
    fn null_reads_as_absent() {
        let raw = args(serde_json::json!({"owner": null, "mine": null, "limit": null}));
        let a = Args::new(&raw);
        assert_eq!(a.str("owner").unwrap(), None);
        assert!(!a.flag("mine").unwrap());
        assert_eq!(a.int_or("limit", 20).unwrap(), 20);
    }

    #[test]
    fn wrong_types_are_rejected_by_name() {
        let raw = args(serde_json::json!({"tags": "a,b", "limit": "9", "mine": 1}));
        let a = Args::new(&raw);
        assert_eq!(
            a.list("tags").unwrap_err().to_string(),
            "parameter 'tags' must be an array of strings"
        );
        assert_eq!(
            a.int("limit").unwrap_err().to_string(),
            "parameter 'limit' must be an integer"
        );
        assert_eq!(
            a.flag("mine").unwrap_err().to_string(),
            "parameter 'mine' must be a boolean"
        );
    }

    #[test]
    fn a_list_of_non_strings_is_rejected() {
        let raw = args(serde_json::json!({"on": [1, 2]}));
        let a = Args::new(&raw);
        assert!(a.req_list("on").is_err());
        let raw = args(serde_json::json!({"on": []}));
        let a = Args::new(&raw);
        assert!(a.req_list("on").is_err());
    }

    #[test]
    fn warnings_are_always_present_and_last() {
        let payload = with_warnings(serde_json::json!({"id": "n-A"}), Vec::new());
        assert_eq!(payload["warnings"], serde_json::json!([]));
        let keys: Vec<&String> = payload.as_object().unwrap().keys().collect();
        assert_eq!(keys.last().map(|k| k.as_str()), Some("warnings"));
        let payload = with_warnings(
            serde_json::json!({"id": "n-A"}),
            duplicate_warning(Some("n-B")),
        );
        assert_eq!(
            payload["warnings"],
            serde_json::json!(["duplicate title, also used by n-B"])
        );
    }

    #[test]
    fn an_unknown_tool_is_a_validation_error() {
        let server = Server::new(Some(PathBuf::from("/definitely/not/here.toml")), None);
        let err = server
            .call("mesh_note_delete", &serde_json::json!({}))
            .unwrap_err();
        assert_eq!(err.kind(), "validation");
        assert_eq!(err.to_string(), "unknown tool: 'mesh_note_delete'");
    }

    #[test]
    fn a_missing_config_is_reported_per_call_not_at_startup() {
        let server = Server::new(Some(PathBuf::from("/definitely/not/here.toml")), None);
        assert!(server.instructions().contains("mesh init"));
        for name in crate::mcp::TOOL_NAMES {
            let err = server.call(name, &serde_json::json!({})).unwrap_err();
            assert_eq!(err.kind(), "config_missing", "{name}");
            assert_eq!(err.code(), 2);
        }
    }
}
