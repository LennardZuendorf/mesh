//! `recent-activity`, `build-context`, `graph`, `project`, `session-start`.
//!
//! Every lens is class **L**: `--json` beats `--quiet`. None of them ever writes an
//! infrastructure notice — the daemon-down line is gone with the daemon (final.md §9.3).

use serde_json::Value as Json;

use crate::cli::out;
use crate::cli::{BuildContextArgs, GraphArgs, ProjectArgs, RecentActivityArgs, SessionStartArgs};
use crate::config::Config;
use crate::ctx::Ctx;
use crate::domain::lenses;
use crate::domain::select::{Filter, SortKey};
use crate::domain::tasks::Availability;
use crate::domain::{activity, context};
use crate::error::Result;
use crate::spaces::Space;

// --------------------------------------------------------------------------------------------
// shared helpers
// --------------------------------------------------------------------------------------------

/// The spaces a lens reads: the `--space` CSV when given, else the lens default restricted to
/// the spaces this vault actually enables.
fn spaces_for(cfg: &Config, csv: Option<&str>, default: &[Space]) -> Result<Vec<Space>> {
    match csv {
        Some(value) => crate::search::resolve_spaces(cfg, Some(value)),
        None => Ok(default
            .iter()
            .copied()
            .filter(|space| cfg.root(*space).is_ok())
            .collect()),
    }
}

/// A string column: `""` when the key is absent or null.
fn text_of(entry: &Json, key: &str) -> String {
    entry
        .get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An identity column: `-` when absent, null or empty — the `task list` convention.
fn identity_of(entry: &Json, key: &str) -> String {
    let text = text_of(entry, key);
    if text.is_empty() {
        "-".to_string()
    } else {
        text
    }
}

/// One compact JSON line on stdout — the same bytes `out::json_line` produces.
fn emit_json(value: &Json) {
    out::line(&serde_json::to_string(value).unwrap_or_else(|_| "null".to_string()));
}

/// `{id}\t{type}\t{owner|-}\t{claimed_by|-}\t{title}\t{path}`.
fn activity_row(entry: &Json) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        text_of(entry, "id"),
        text_of(entry, "type"),
        identity_of(entry, "owner"),
        identity_of(entry, "claimed_by"),
        text_of(entry, "title"),
        text_of(entry, "path")
    )
}

/// `{id}\t{type}\t{title}\t{path}`.
fn context_row(entry: &Json) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        text_of(entry, "id"),
        text_of(entry, "type"),
        text_of(entry, "title"),
        text_of(entry, "path")
    )
}

/// `{id}\t{type}\t{reason}\t{owner|-}\t{claimed_by|-}\t{title}\t{path}`.
fn session_row(entry: &Json) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        text_of(entry, "id"),
        text_of(entry, "type"),
        text_of(entry, "reason"),
        identity_of(entry, "owner"),
        identity_of(entry, "claimed_by"),
        text_of(entry, "title"),
        text_of(entry, "path")
    )
}

// --------------------------------------------------------------------------------------------
// the five lenses
// --------------------------------------------------------------------------------------------

/// `mesh recent-activity` — the mtime-ordered change feed.
pub fn recent_activity(ctx: &mut Ctx, args: RecentActivityArgs) -> Result<()> {
    ctx.coalesce_mine(args.mine);
    ctx.coalesce(args.out.json, args.out.quiet, args.owner.clone());
    let owner = ctx.g.owner.clone();
    let mine = ctx.g.mine;
    let cfg = ctx.cfg()?;
    let spaces = spaces_for(cfg, args.space.as_deref(), &activity::DEFAULT_SPACES)?;
    let entries = activity::recent_activity_in(
        cfg,
        args.since.as_deref(),
        owner.as_deref(),
        mine,
        args.limit,
        &spaces,
    )?;
    out::rows(ctx, &entries, activity_row);
    Ok(())
}

/// `mesh build-context SEED_ID` — forward BFS over `related`.
pub fn build_context(ctx: &mut Ctx, args: BuildContextArgs) -> Result<()> {
    ctx.coalesce(args.out.json, args.out.quiet, None);
    let cfg = ctx.cfg()?;
    let spaces = spaces_for(cfg, args.space.as_deref(), &context::DEFAULT_SPACES)?;
    let entries = context::build_context_in(cfg, &args.seed_id, args.depth, &spaces)?;
    out::rows(ctx, &entries, context_row);
    Ok(())
}

/// `mesh graph SEED_ID` — nodes and edges, or the discovery tree.
pub fn graph(ctx: &mut Ctx, args: GraphArgs) -> Result<()> {
    ctx.coalesce(args.out.json, args.out.quiet, None);
    let cfg = ctx.cfg()?;
    let spaces = spaces_for(cfg, args.space.as_deref(), &context::DEFAULT_SPACES)?;
    let result = context::graph_query_in(cfg, &args.seed_id, args.depth, &args.direction, &spaces)?;
    if ctx.g.json {
        emit_json(&result.to_json());
        return Ok(());
    }
    if ctx.g.quiet {
        for id in result.ids() {
            out::line(&id);
        }
        return Ok(());
    }
    for line in result.tree_lines() {
        out::line(&line);
    }
    Ok(())
}

/// `mesh project PROJECT_ID` — a project note and the tasks scoped to it.
pub fn project(ctx: &mut Ctx, args: ProjectArgs) -> Result<()> {
    ctx.coalesce(args.out.json, args.out.quiet, None);
    let cfg = ctx.cfg()?;
    let spaces = spaces_for(cfg, args.space.as_deref(), &lenses::DEFAULT_SPACES)?;
    let payload = lenses::project_view_in(cfg, &args.project_id, &spaces)?;
    if ctx.g.json {
        emit_json(&payload);
        return Ok(());
    }
    let empty: Vec<Json> = Vec::new();
    let tasks = payload
        .get("tasks")
        .and_then(Json::as_array)
        .unwrap_or(&empty);
    let project_node = payload.get("project").cloned().unwrap_or(Json::Null);
    if ctx.g.quiet {
        out::line(&text_of(&project_node, "id"));
        for task in tasks {
            out::line(&text_of(task, "id"));
        }
        return Ok(());
    }
    out::line(&format!(
        "{}\t{}\t{}",
        text_of(&project_node, "id"),
        text_of(&project_node, "type"),
        text_of(&project_node, "title")
    ));
    for task in tasks {
        out::line(&format!(
            "  {}\t{}\t{}",
            text_of(task, "id"),
            text_of(task, "status"),
            text_of(task, "title")
        ));
    }
    Ok(())
}

/// `mesh session-start` — the warm-start composite.
///
/// Fetch order is load-bearing: my tasks (any status) → my notes (only when an identity is
/// set) → mentions of either → memories → recent activity.
pub fn session_start(ctx: &mut Ctx, args: SessionStartArgs) -> Result<()> {
    ctx.coalesce(args.out.json, args.out.quiet, args.owner.clone());
    let owner = ctx.g.owner.clone();
    let budget = usize::try_from(args.budget).unwrap_or(0);
    let cfg = ctx.cfg()?;
    let spaces = spaces_for(cfg, args.space.as_deref(), &activity::DEFAULT_SPACES)?;
    // `--owner X` swaps the effective identity; every source then uses its own `mine` / `me`.
    let effective = cfg.with_agent(owner.as_deref());
    let me = effective.agent().map(str::to_string);

    let task_filter = Filter {
        mine: true,
        me: me.clone(),
        limit: None,
        sort: SortKey::Updated,
        ..Filter::default()
    };
    let tasks = crate::domain::tasks::list(&effective, &task_filter, Availability::Any)?;
    // Skipped entirely with no identity: `owner: None` means *unfiltered*, which would claim
    // every note in the vault as mine.
    let notes = match &me {
        Some(identity) => {
            let filter = Filter {
                owner: Some(identity.clone()),
                limit: None,
                sort: SortKey::Updated,
                ..Filter::default()
            };
            crate::domain::notes::list(&effective, &filter, false)?
        }
        None => Vec::new(),
    };
    let mentions = lenses::session_mentions_in(
        &effective,
        &tasks,
        &notes,
        me.as_deref(),
        lenses::SESSION_SINCE,
        &spaces,
    );
    let memories = if args.no_memories {
        Vec::new()
    } else {
        crate::domain::memories::session_picks(&effective, me.as_deref(), lenses::MEMORY_PICKS)
    };
    // `--team` widens the activity half only; the task queue stays mine.
    let recent = activity::recent_activity_in(
        &effective,
        Some(lenses::SESSION_SINCE),
        None,
        !args.team,
        activity::DEFAULT_LIMIT,
        &spaces,
    )?;
    let entries = lenses::session_start_entries(
        &effective,
        &tasks,
        mentions,
        memories,
        recent,
        args.meta_only,
        budget,
    );
    out::rows(ctx, &entries, session_row);
    Ok(())
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
    use crate::config::test_support::config_for;

    #[test]
    fn identity_columns_render_a_dash_when_absent() {
        let entry = serde_json::json!({"id": "n-a", "type": "note", "title": "T",
                                       "path": "/v/n-a.md", "owner": null, "claimed_by": ""});
        assert_eq!(activity_row(&entry), "n-a\tnote\t-\t-\tT\t/v/n-a.md");
        assert_eq!(context_row(&entry), "n-a\tnote\tT\t/v/n-a.md");
    }

    #[test]
    fn the_session_row_has_seven_columns_with_reason_third() {
        let entry = serde_json::json!({"id": "t-a", "type": "task", "title": "T",
                                       "path": "/v/t-a.md", "owner": "alice",
                                       "claimed_by": "bob", "reason": "task"});
        assert_eq!(
            session_row(&entry),
            "t-a\ttask\ttask\talice\tbob\tT\t/v/t-a.md"
        );
        assert_eq!(session_row(&entry).split('\t').count(), 7);
    }

    #[test]
    fn the_truncated_marker_still_renders() {
        let entry = serde_json::json!({"id": null, "type": "meta",
                                       "reason": "truncated", "dropped": 2});
        assert_eq!(session_row(&entry), "\tmeta\ttruncated\t-\t-\t\t");
    }

    #[test]
    fn a_space_csv_is_validated_and_defaults_are_filtered_to_enabled_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        assert_eq!(
            spaces_for(&cfg, Some("notes,tasks"), &activity::DEFAULT_SPACES).unwrap(),
            vec![Space::Notes, Space::Tasks]
        );
        let err = spaces_for(&cfg, Some("nope"), &activity::DEFAULT_SPACES).unwrap_err();
        assert_eq!(err.code(), 2);
        assert!(err.to_string().starts_with("invalid space: 'nope'"));
        assert_eq!(
            spaces_for(&cfg, None, &context::DEFAULT_SPACES).unwrap(),
            vec![Space::Notes, Space::Tasks, Space::Memories]
        );
    }

    #[test]
    fn emit_json_matches_the_compact_line_shape() {
        let value = serde_json::json!({"a": 1});
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            out::json_line(&value).trim_end().to_string()
        );
    }
}
