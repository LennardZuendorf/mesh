//! `mesh task …` (lifecycle verbs).

use serde_json::{Map, Value as Json};

use crate::cli::{out, TaskSub};
use crate::ctx::Ctx;
use crate::domain::tasks::{self, Availability, NewTask, Terminal, UpdateTask};
use crate::domain::{deps, select::parse_csv, AppendOpts, Filter, SortKey};
use crate::error::{MeshError, Result};
use crate::fm::{Meta, View};
use crate::model::task::{Task, TASK_FIELDS};
use crate::render::entry;
use crate::text::preview;
use crate::timefmt::{parse_since, ts_wire};

/// The sort keys `task list` accepts.
pub const TASK_SORT_KEYS: [SortKey; 4] = [
    SortKey::Updated,
    SortKey::Created,
    SortKey::Title,
    SortKey::Priority,
];

/// Run one lifecycle `task` subcommand. `block`/`unblock`/`next` live in `task_dep`.
pub fn run(ctx: &mut Ctx, sub: TaskSub) -> Result<()> {
    let (json, quiet, owner) = match &sub {
        TaskSub::New { out, owner, .. } => (out.json, out.quiet, owner.clone()),
        // The global --owner is deliberately not folded into the reassignment --owner.
        TaskSub::Update { out, .. } => (out.json, out.quiet, None),
        TaskSub::Append { out, .. } => (out.json, out.quiet, None),
        TaskSub::Claim { out, .. } => (out.json, out.quiet, None),
        TaskSub::Release { out, .. } => (out.json, out.quiet, None),
        TaskSub::Finish { out, .. } => (out.json, out.quiet, None),
        TaskSub::Cancel { out, .. } => (out.json, out.quiet, None),
        TaskSub::Get { out, .. } => (out.json, out.quiet, None),
        TaskSub::List {
            out, owner, mine, ..
        } => {
            ctx.coalesce_mine(*mine);
            (out.json, out.quiet, owner.clone())
        }
        TaskSub::Delete { out, .. } => (out.json, out.quiet, None),
        TaskSub::Block { out, .. } => (out.json, out.quiet, None),
        TaskSub::Unblock { out, .. } => (out.json, out.quiet, None),
        TaskSub::Next { out, .. } => (out.json, out.quiet, None),
    };
    ctx.coalesce(json, quiet, owner);
    ctx.cfg()?;
    dispatch(ctx, sub)
}

fn dispatch(ctx: &Ctx, sub: TaskSub) -> Result<()> {
    match sub {
        TaskSub::New {
            title,
            priority,
            tags,
            owner,
            body,
            project,
            blocks,
            blocked_by,
            ..
        } => new(
            ctx,
            &title,
            NewTask {
                priority,
                tags: tags.as_deref().map(parse_csv).unwrap_or_default(),
                owner,
                body: body.unwrap_or_default(),
                project,
                blocks: blocks.as_deref().map(parse_csv).unwrap_or_default(),
                blocked_by: blocked_by.as_deref().map(parse_csv).unwrap_or_default(),
            },
        ),
        TaskSub::Update {
            task_id,
            priority,
            tags,
            title,
            project,
            owner,
            blocks,
            blocked_by,
            ..
        } => update(
            ctx,
            &task_id,
            UpdateTask {
                priority,
                tags,
                title,
                project,
                owner,
                blocks: blocks.as_deref().map(parse_csv),
                blocked_by: blocked_by.as_deref().map(parse_csv),
            },
        ),
        TaskSub::Append {
            task_id,
            text,
            section,
            timestamp,
            ..
        } => append(ctx, &task_id, &text, section, timestamp),
        TaskSub::Claim {
            task_id,
            strict,
            no_strict,
            ..
        } => claim(ctx, &task_id, strict, no_strict),
        TaskSub::Release {
            task_id,
            force,
            note,
            ..
        } => release(ctx, &task_id, force, note.as_deref()),
        TaskSub::Finish {
            task_id, outcome, ..
        } => terminate(ctx, &task_id, Terminal::Finish, outcome.as_deref()),
        TaskSub::Cancel {
            task_id, reason, ..
        } => terminate(ctx, &task_id, Terminal::Cancel, reason.as_deref()),
        TaskSub::Get {
            task_id,
            full,
            meta_only,
            ..
        } => get(ctx, &task_id, full, meta_only),
        TaskSub::List {
            status,
            owner,
            tags,
            any_tag,
            project,
            since,
            stale,
            available,
            ready,
            blocked,
            sort,
            limit,
            ..
        } => list(
            ctx,
            ListArgs {
                status,
                owner,
                tags,
                any_tag,
                project,
                since,
                stale,
                available,
                ready,
                blocked,
                sort,
                limit,
            },
        ),
        TaskSub::Delete { task_id, force, .. } => delete(ctx, &task_id, force),
        // `dispatch` in cli/mod.rs routes these three to `task_dep`; they cannot arrive here.
        TaskSub::Block { .. } | TaskSub::Unblock { .. } | TaskSub::Next { .. } => {
            Err(MeshError::Validation("not a lifecycle verb".to_string()))
        }
    }
}

// ---------------------------------------------------------------------------------------
// mutations (class M)
// ---------------------------------------------------------------------------------------

fn new(ctx: &Ctx, title: &str, o: NewTask) -> Result<()> {
    let cfg = ctx.cfg()?;
    // Advisory only, before the create lock: a concurrent creator can race past it.
    if let Some(existing) = tasks::find_duplicate_title(cfg, title) {
        out::notice(
            ctx,
            &format!("task new: duplicate title, also used by {existing}"),
        );
    }
    let task = tasks::create(cfg, title, o)?;
    report(ctx, &task, "created", &[]);
    Ok(())
}

fn update(ctx: &Ctx, id: &str, o: UpdateTask) -> Result<()> {
    let task = tasks::update(ctx.cfg()?, id, o)?;
    report(ctx, &task, "updated", &[]);
    Ok(())
}

fn append(ctx: &Ctx, id: &str, text: &str, section: Option<String>, timestamp: bool) -> Result<()> {
    let opts = AppendOpts {
        section,
        timestamp,
        actor: ctx.actor().map(str::to_string),
    };
    let task = tasks::append(ctx.cfg()?, id, text, opts)?;
    report(ctx, &task, "appended", &[]);
    Ok(())
}

fn claim(ctx: &Ctx, id: &str, strict: bool, no_strict: bool) -> Result<()> {
    let cfg = ctx.cfg()?;
    // `--no-strict` overrides `--strict`, and both override `[tasks].strict`.
    let strict = if no_strict {
        false
    } else {
        strict || cfg.tasks.strict
    };
    let claimer = identity(ctx)?;
    let (task, unsatisfied) = tasks::claim(cfg, id, &claimer, strict)?;
    if !unsatisfied.is_empty() {
        out::notice(
            ctx,
            &format!("task {id} is blocked by {}", unsatisfied.join(", ")),
        );
    }
    let extra: Vec<(&str, Json)> = if unsatisfied.is_empty() {
        Vec::new()
    } else {
        vec![("blocked_by_unsatisfied", strings(&unsatisfied))]
    };
    report(ctx, &task, "claimed", &extra);
    Ok(())
}

fn release(ctx: &Ctx, id: &str, force: bool, note: Option<&str>) -> Result<()> {
    let cfg = ctx.cfg()?;
    let releaser = identity(ctx)?;
    let mut task = tasks::release(cfg, id, &releaser, force)?;
    if let Some(text) = note {
        // A second call: always timestamped, always attributed to the releaser, and its
        // returned task is what gets reported.
        task = tasks::append(
            cfg,
            id,
            text,
            AppendOpts {
                section: None,
                timestamp: true,
                actor: Some(releaser),
            },
        )?;
    }
    report(ctx, &task, "released", &[]);
    Ok(())
}

fn terminate(ctx: &Ctx, id: &str, kind: Terminal, text: Option<&str>) -> Result<()> {
    let actor = ctx.actor().map(str::to_string);
    let (task, unblocked) = tasks::terminate(ctx.cfg()?, id, kind, text, actor.as_deref())?;
    if !unblocked.is_empty() {
        out::notice(ctx, &format!("unblocked: {}", unblocked.join(", ")));
    }
    let extra: Vec<(&str, Json)> = if unblocked.is_empty() {
        Vec::new()
    } else {
        vec![("unblocked", strings(&unblocked))]
    };
    report(ctx, &task, kind.verb(), &extra);
    Ok(())
}

fn delete(ctx: &Ctx, id: &str, force: bool) -> Result<()> {
    // The guard runs against the raw id — there is no pre-resolution on the task side.
    out::delete_guard(ctx, id, force)?;
    let deleted = tasks::delete(ctx.cfg()?, id)?;
    if ctx.g.quiet {
        out::line(&deleted);
    } else if ctx.g.json {
        let mut payload = Map::new();
        payload.insert("id".into(), Json::String(deleted));
        payload.insert("deleted".into(), Json::Bool(true));
        out::object(ctx, &Json::Object(payload), |_| String::new());
    } else {
        out::line(&format!("deleted {deleted}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// reads (class L)
// ---------------------------------------------------------------------------------------

fn get(ctx: &Ctx, id: &str, full: bool, meta_only: bool) -> Result<()> {
    let cfg = ctx.cfg()?;
    let view = tasks::get(cfg, id)?;
    let ready = deps::readiness(&tasks::rows(cfg), id).ready;
    if ctx.g.json {
        let body = (!meta_only).then(|| view.body.clone());
        let mut payload = entry(&view.item.meta, TASK_FIELDS.fields(), body.as_deref(), None);
        if let Some(object) = payload.as_object_mut() {
            object.insert("ready".into(), Json::Bool(ready));
        }
        out::object(ctx, &payload, |_| String::new());
        return Ok(());
    }
    if ctx.g.quiet {
        out::line(&view.item.id);
        return Ok(());
    }
    for line in meta_lines(&view.item.meta) {
        out::line(&line);
    }
    out::line(&format!("ready: {ready}"));
    if !meta_only {
        out::line("");
        out::line(&preview(&view.body, full));
    }
    Ok(())
}

/// One `task list` invocation's flags, straight off the parsed enum.
struct ListArgs {
    status: Option<String>,
    owner: Option<String>,
    tags: Option<String>,
    any_tag: bool,
    project: Option<String>,
    since: Option<String>,
    stale: Option<String>,
    available: bool,
    ready: bool,
    blocked: bool,
    sort: Option<String>,
    limit: i64,
}

fn list(ctx: &Ctx, args: ListArgs) -> Result<()> {
    let cfg = ctx.cfg()?;
    let availability = if args.ready {
        Availability::Ready
    } else if args.blocked {
        Availability::Blocked
    } else if args.available {
        Availability::Available
    } else {
        Availability::Any
    };
    // The `--sort` default is computed, not declared.
    let sort_name = match &args.sort {
        Some(value) => value.clone(),
        None if args.available || args.ready => "priority".to_string(),
        None => "updated".to_string(),
    };
    let sort = SortKey::parse(&sort_name, &TASK_SORT_KEYS)?;
    let statuses = match &args.status {
        Some(value) => tasks::parse_status_csv(value)?,
        None => None,
    };
    let filter = Filter {
        tags: args.tags.as_deref().map(parse_csv),
        any_tag: args.any_tag,
        owner: args.owner.clone(),
        mine: ctx.g.mine,
        me: cfg.agent().map(str::to_string),
        cutoff: args.since.as_deref().map(parse_since).transpose()?,
        stale_cutoff: args.stale.as_deref().map(parse_since).transpose()?,
        sort,
        // A `--status` union is a membership set, which `Filter::extra` cannot express, so it
        // is applied here — which means the limit has to be applied here too, or the union
        // would slice an already-truncated page. Order stays filter → sort → limit.
        limit: if statuses.is_some() {
            None
        } else {
            Some(args.limit)
        },
        extra: Vec::new(),
    }
    .with_extra("project", args.project.as_deref());

    let views = tasks::list(cfg, &filter, availability)?;
    let views: Vec<View<Task>> = match &statuses {
        Some(wanted) => {
            let mut kept: Vec<View<Task>> = views
                .into_iter()
                .filter(|v| wanted.iter().any(|s| s == &v.item.status))
                .collect();
            if args.limit >= 0 {
                kept.truncate(usize::try_from(args.limit).unwrap_or(0));
            }
            kept
        }
        None => views,
    };
    let entries: Vec<Json> = views
        .iter()
        .map(|v| entry(&v.item.meta, TASK_FIELDS.fields(), None, None))
        .collect();
    out::rows(ctx, &entries, row);
    Ok(())
}

/// The tab-separated human row: `{id}\t{status}\t{claimed_by|-}\t{title}`.
fn row(value: &Json) -> String {
    let field = |key: &str| value.get(key).and_then(Json::as_str).unwrap_or_default();
    let holder = match value.get("claimed_by").and_then(Json::as_str) {
        Some(who) if !who.is_empty() => who,
        _ => "-",
    };
    format!(
        "{}\t{}\t{holder}\t{}",
        field("id"),
        field("status"),
        field("title")
    )
}

/// The 14-line meta block, in the pinned order.
pub fn meta_lines(meta: &Meta) -> Vec<String> {
    let text = |key: &str| -> String {
        meta.get(key)
            .and_then(|v| match v {
                crate::fm::Value::Str(s) => Some(s.clone()),
                crate::fm::Value::Ts(ts) => Some(ts_wire(&ts.value, true)),
                _ => None,
            })
            .unwrap_or_default()
    };
    let joined = |key: &str| crate::model::common::meta_strings(meta, key).join(", ");
    vec![
        format!("id: {}", text("id")),
        format!("type: {}", text("type")),
        format!("title: {}", text("title")),
        format!("status: {}", text("status")),
        format!("priority: {}", text("priority")),
        format!("owner: {}", text("owner")),
        format!("claimed_by: {}", text("claimed_by")),
        format!("project: {}", text("project")),
        format!("tags: {}", joined("tags")),
        format!("blocks: {}", joined("blocks")),
        format!("blocked_by: {}", joined("blocked_by")),
        format!("created: {}", text("created")),
        format!("updated: {}", text("updated")),
        format!("related: {}", joined("related")),
    ]
}

// ---------------------------------------------------------------------------------------
// shared bits
// ---------------------------------------------------------------------------------------

/// The class-M report: `{verb} {id}` / `{"id","status",<extra>,"updated"}` / the bare id.
fn report(ctx: &Ctx, task: &Task, verb: &str, extra: &[(&str, Json)]) {
    let mut fields: Vec<(&str, Json)> = vec![("status", Json::String(task.status.clone()))];
    fields.extend(extra.iter().cloned());
    let updated = task.updated.unwrap_or_else(crate::timefmt::now_utc);
    out::mutation(ctx, &task.id, verb, &fields, updated);
}

/// Who is acting: the global `--owner`, else `[core].agent`.
pub fn identity(ctx: &Ctx) -> Result<String> {
    ctx.actor()
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            MeshError::Validation("no agent identity: set [core].agent or pass --owner".to_string())
        })
}

/// A JSON array of strings.
pub fn strings(items: &[String]) -> Json {
    Json::Array(items.iter().map(|s| Json::String(s.clone())).collect())
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
    use crate::fm::parse_meta;

    #[test]
    fn the_meta_block_is_the_pinned_fourteen_lines() {
        let meta = parse_meta(
            "id: t-1\ntype: task\ntitle: T\ntags:\n  - a\n  - b\nowner: alice\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2026-01-02T00:00:00Z\nrelated: []\n\
             status: claimed\npriority: high\nclaimed_by: bob\nproject: n-P\n\
             blocks:\n  - t-2\nblocked_by: []\n",
        )
        .unwrap();
        let lines = meta_lines(&meta);
        assert_eq!(lines.len(), 14);
        assert_eq!(
            lines,
            [
                "id: t-1",
                "type: task",
                "title: T",
                "status: claimed",
                "priority: high",
                "owner: alice",
                "claimed_by: bob",
                "project: n-P",
                "tags: a, b",
                "blocks: t-2",
                "blocked_by: ",
                "created: 2026-01-01T00:00:00Z",
                "updated: 2026-01-02T00:00:00Z",
                "related: ",
            ]
        );
    }

    #[test]
    fn a_null_optional_renders_as_an_empty_value() {
        let meta = parse_meta(
            "id: t-1\ntype: task\ntitle: T\nstatus: open\npriority: null\n\
             claimed_by: null\nproject: null\n",
        )
        .unwrap();
        let lines = meta_lines(&meta);
        assert_eq!(lines[4], "priority: ");
        assert_eq!(lines[6], "claimed_by: ");
        assert_eq!(lines[7], "project: ");
    }

    #[test]
    fn the_row_is_tab_separated_with_a_dash_for_an_unclaimed_holder() {
        let held = serde_json::json!({
            "id": "t-held", "status": "claimed", "claimed_by": "agent-a", "title": "Held Task"
        });
        assert_eq!(
            row(&held).split('\t').collect::<Vec<_>>(),
            ["t-held", "claimed", "agent-a", "Held Task"]
        );
        let open = serde_json::json!({
            "id": "t-open", "status": "open", "claimed_by": Json::Null, "title": "Open Task"
        });
        assert_eq!(
            row(&open).split('\t').collect::<Vec<_>>(),
            ["t-open", "open", "-", "Open Task"]
        );
    }

    #[test]
    fn the_sort_keys_are_the_four_tasks_document() {
        let err = SortKey::parse("bogus", &TASK_SORT_KEYS).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid sort field: 'bogus' (use updated, created, title, priority)"
        );
    }

    #[test]
    fn strings_renders_a_json_array() {
        assert_eq!(
            strings(&["a".to_string(), "b".to_string()]),
            serde_json::json!(["a", "b"])
        );
        assert_eq!(strings(&[]), serde_json::json!([]));
    }
}
