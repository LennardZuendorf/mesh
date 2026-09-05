//! `mesh task block | unblock | next` and the `--ready` / `--blocked` plumbing.

use serde_json::{Map, Value as Json};

use crate::cli::task::{identity, strings};
use crate::cli::{out, TaskSub};
use crate::ctx::Ctx;
use crate::domain::tasks::{self, Availability};
use crate::domain::{deps, select::parse_csv, Filter, SortKey};
use crate::error::{MeshError, Result};
use crate::model::task::TASK_FIELDS;
use crate::render::entry;
use crate::timefmt::{iso_z, now_utc};

/// Run one dependency-graph `task` subcommand.
pub fn run(ctx: &mut Ctx, sub: TaskSub) -> Result<()> {
    let (json, quiet) = match &sub {
        TaskSub::Block { out, .. } => (out.json, out.quiet),
        TaskSub::Unblock { out, .. } => (out.json, out.quiet),
        TaskSub::Next { out, mine, .. } => {
            ctx.coalesce_mine(*mine);
            (out.json, out.quiet)
        }
        other => {
            let _ = other;
            (false, false)
        }
    };
    ctx.coalesce(json, quiet, None);
    ctx.cfg()?;
    match sub {
        TaskSub::Block { task_id, on, .. } => block(ctx, &task_id, &parse_csv(&on)),
        TaskSub::Unblock {
            task_id, on, all, ..
        } => unblock(
            ctx,
            &task_id,
            &on.as_deref().map(parse_csv).unwrap_or_default(),
            all,
        ),
        TaskSub::Next {
            claim,
            strict,
            project,
            tags,
            ..
        } => next(ctx, claim, strict, project, tags),
        // `dispatch` in cli/mod.rs only routes the three dependency verbs here.
        other => {
            let _ = other;
            Err(MeshError::Validation("not a dependency verb".to_string()))
        }
    }
}

// ---------------------------------------------------------------------------------------
// block / unblock (class M)
// ---------------------------------------------------------------------------------------

fn block(ctx: &Ctx, id: &str, on: &[String]) -> Result<()> {
    let cfg = ctx.cfg()?;
    let (task, warnings) = deps::block(cfg, id, on)?;
    for warning in &warnings {
        out::notice(ctx, &warning.0);
    }
    edge_report(ctx, &task, "blocked", "by", on);
    Ok(())
}

fn unblock(ctx: &Ctx, id: &str, on: &[String], all: bool) -> Result<()> {
    let cfg = ctx.cfg()?;
    // `--all` reports every blocker it reached, not the (empty) `--on` list.
    let removed = if all {
        deps::effective_blockers(&tasks::rows(cfg), id)
    } else {
        on.to_vec()
    };
    let (task, warnings) = deps::unblock(cfg, id, on, all)?;
    for warning in &warnings {
        out::notice(ctx, &warning.0);
    }
    edge_report(ctx, &task, "unblocked", "from", &removed);
    Ok(())
}

/// `blocked t-T by t-B1, t-B2` / `{"id","blocked_by","ready","updated"}` / the bare id.
fn edge_report(
    ctx: &Ctx,
    task: &crate::model::task::Task,
    verb: &str,
    preposition: &str,
    targets: &[String],
) {
    if ctx.g.quiet {
        out::line(&task.id);
        return;
    }
    if ctx.g.json {
        let ready = ctx
            .cfg()
            .map(|cfg| deps::readiness(&tasks::rows(cfg), &task.id).ready)
            .unwrap_or(false);
        let mut payload = Map::new();
        payload.insert("id".into(), Json::String(task.id.clone()));
        payload.insert("blocked_by".into(), strings(&task.blocked_by));
        payload.insert("ready".into(), Json::Bool(ready));
        payload.insert(
            "updated".into(),
            Json::String(iso_z(&task.updated.unwrap_or_else(now_utc))),
        );
        out::object(ctx, &Json::Object(payload), |_| String::new());
        return;
    }
    if targets.is_empty() {
        out::line(&format!("{verb} {}", task.id));
    } else {
        out::line(&format!(
            "{verb} {} {preposition} {}",
            task.id,
            targets.join(", ")
        ));
    }
}

// ---------------------------------------------------------------------------------------
// next (class L)
// ---------------------------------------------------------------------------------------

fn next(
    ctx: &Ctx,
    claim: bool,
    strict: bool,
    project: Option<String>,
    tags: Option<String>,
) -> Result<()> {
    let cfg = ctx.cfg()?;
    let strict = strict || cfg.tasks.strict;
    // Selection is the `--sort priority` composition: rank, then FIFO by created, then path.
    let filter = Filter {
        tags: tags.as_deref().map(parse_csv),
        any_tag: false,
        owner: None,
        mine: ctx.g.mine,
        me: cfg.agent().map(str::to_string),
        cutoff: None,
        stale_cutoff: None,
        sort: SortKey::Priority,
        limit: None,
        extra: Vec::new(),
    }
    .with_extra("project", project.as_deref());

    let claimer = if claim { Some(identity(ctx)?) } else { None };
    let picked = deps::next(cfg, &filter, claim, strict, claimer.as_deref())?;
    let Some(view) = picked else {
        return Err(MeshError::Empty(NO_READY_TASK.into()));
    };
    if ctx.g.json {
        let payload = entry(
            &view.item.meta,
            TASK_FIELDS.fields(),
            None,
            Some(&view.path),
        );
        out::object(ctx, &payload, |_| String::new());
        return Ok(());
    }
    if ctx.g.quiet {
        out::line(&view.item.id);
        return Ok(());
    }
    let holder = view.item.claimed_by.as_deref().unwrap_or("-");
    out::line(&format!(
        "{}\t{}\t{holder}\t{}",
        view.item.id, view.item.status, view.item.title
    ));
    Ok(())
}

/// The stderr line `task next` prints when the queue is empty.
pub const NO_READY_TASK: &str = "no ready task";

/// The availability slice `task next` selects from — exposed for the MCP and lens agents.
pub const NEXT_AVAILABILITY: Availability = Availability::Ready;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_queue_message_is_pinned() {
        assert_eq!(NO_READY_TASK, "no ready task");
        assert_eq!(NEXT_AVAILABILITY, Availability::Ready);
    }

    #[test]
    fn the_not_found_next_action_is_the_shared_one() {
        assert_eq!(
            MeshError::TaskNotFound(String::new()).next_action(),
            "check the id and retry, or list to find the right one"
        );
    }
}
