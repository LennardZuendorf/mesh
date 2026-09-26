//! `mesh dashboard` — the operator's read-only screen.
//!
//! This module owns the **snapshot**: one read pass that composes the same domain surfaces
//! the CLI verbs already print — the `status` census, the `task list` slices, the
//! recent-activity lens and the search-health report — into a plain frame model. It adds no
//! second source of truth, holds no lock and writes nothing.
//!
//! The terminal lifecycle (tty guard, alternate screen, event loop, keys) and the render
//! mapping are the next units' work; they are the only places `ratatui` and `crossterm` may
//! be imported.

use std::time::Duration;

use serde_json::Value as Json;

use crate::config::Config;
use crate::domain::activity;
use crate::domain::select::{Filter, SortKey};
use crate::domain::tasks::{self, Availability};

/// How the dashboard is run.
#[derive(Clone, Debug)]
pub struct DashOpts {
    /// The tick interval: how long the loop waits before re-reading the folder.
    pub interval: Duration,
}

/// One agent's row in the agents pane: an identity plus the census the status report built.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentRow {
    /// The owner or claimer identity, as the census reports it.
    pub identity: String,
    /// Tasks this identity owns that are still open.
    pub owns_open: u64,
    /// Tasks this identity holds a claim on.
    pub claimed: u64,
    /// Claims past the stale window.
    pub stale_claims: u64,
    /// Notes this identity owns.
    pub notes_owned: u64,
    /// Notes this identity holds a claim on.
    pub notes_claimed: u64,
}

/// One task row in the tasks pane: the columns the pane prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    /// The claimer, for the claimed slice; `None` for ready and blocked rows.
    pub claimed_by: Option<String>,
}

/// The three slices of the tasks pane, in the order the pane renders them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TasksByState {
    /// Takeable work: open, unclaimed and unblocked.
    pub ready: Vec<TaskRow>,
    /// Open or claimed work with an unsatisfied blocker.
    pub blocked: Vec<TaskRow>,
    /// Work currently held: `status == claimed`.
    pub claimed: Vec<TaskRow>,
}

/// One row of the recent-activity pane, from the seven-key activity row.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityRow {
    pub id: String,
    /// The row's entity type (`note`, `task`, `memory`, …).
    pub kind: String,
    pub title: String,
    pub owner: Option<String>,
    pub claimed_by: Option<String>,
    pub path: String,
    /// The file mtime in epoch seconds — the pane's clock.
    pub mtime_seconds: f64,
}

/// The vault-health pane: the counts, the findings, and the watcher/index liveness.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HealthSummary {
    /// Mesh-native notes (id-bearing files, adopted ones included).
    pub notes_native: u64,
    /// Foreign Markdown: no mesh id, invisible to every lens.
    pub notes_foreign: u64,
    /// The dangling link targets the report lists (capped).
    pub dangling_links: Vec<String>,
    /// The real dangling total, beyond the report's cap.
    pub dangling_links_total: u64,
    /// Lock files past the staleness window.
    pub stale_locks: u64,
    pub watcher_running: bool,
    pub watcher_pid: Option<u32>,
    /// `indexed` when every search gate is open, `fallback` otherwise.
    pub search_mode: String,
    /// The first closed search gate's reason, when search is degraded.
    pub search_reason: Option<String>,
}

/// One frame: everything a single read pass produces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DashFrame {
    pub agents: Vec<AgentRow>,
    pub tasks: TasksByState,
    pub activity: Vec<ActivityRow>,
    pub health: HealthSummary,
}

/// Compose one frame from the domain reads the CLI already prints. Pure: one read pass, no
/// writes, no locks, no second source of truth.
///
/// * the **agents** pane is the `status` census (`lenses::status_report`), which already
///   carries each identity's task counts and note ownership/claims;
/// * the **tasks** pane is three `tasks::list` calls — the `--ready` and `--blocked`
///   availabilities and the `claimed` status slice;
/// * the **activity** pane is the recent-activity lens over its default notes+tasks corpus;
/// * the **health** pane takes the report's link, lock, watcher and note counts and joins the
///   search-health report for the index line.
///
/// `mine` narrows the agents, tasks and activity panes to the acting identity:
/// `cfg.agent()`, the `--owner`-else-`[core].agent` identity every `--mine` resolves — the
/// caller folds `--owner` in through `Config::with_agent`, exactly as `recent-activity` and
/// `session-start` do. With no identity configured, `mine` narrows to nothing, matching the
/// `--mine` discipline everywhere else. The health pane is vault-global and never narrows.
///
/// Fail-soft: every read either degrades on its own (the census and the search report are
/// total) or is collapsed here to an empty pane, so one unreadable or malformed file can
/// never fail the frame.
pub fn snapshot(cfg: &Config, mine: bool) -> DashFrame {
    let report = crate::domain::lenses::status_report(cfg);
    let me: Option<&str> = cfg.agent();
    DashFrame {
        agents: agent_rows(&report, mine, me),
        tasks: TasksByState {
            ready: task_rows(cfg, mine, me, Availability::Ready, None),
            blocked: task_rows(cfg, mine, me, Availability::Blocked, None),
            claimed: task_rows(cfg, mine, me, Availability::Any, Some("claimed")),
        },
        activity: activity_rows(cfg, mine),
        health: health_summary(&report, cfg),
    }
}

/// The agents pane: one row per identity the census reports, in the census's own
/// identity-ascending order. With `mine` the pane keeps only the acting identity — and keeps
/// nothing at all when no identity is configured.
fn agent_rows(report: &Json, mine: bool, me: Option<&str>) -> Vec<AgentRow> {
    let Some(agents) = report.get("agents").and_then(Json::as_object) else {
        return Vec::new();
    };
    agents
        .iter()
        .filter(|(identity, _)| !mine || me.is_some_and(|want| want == identity.as_str()))
        .map(|(identity, counts)| AgentRow {
            identity: identity.clone(),
            owns_open: count_of(counts, "owns_open"),
            claimed: count_of(counts, "claimed"),
            stale_claims: count_of(counts, "stale_claims"),
            notes_owned: count_of(counts, "notes_owned"),
            notes_claimed: count_of(counts, "notes_claimed"),
        })
        .collect()
}

/// One tasks-pane slice: `tasks::list` under the slice's availability, plus the `claimed`
/// status predicate. The sort key mirrors `task list`'s computed default — priority for the
/// ready and blocked slices, updated otherwise — so the dashboard and the CLI agree on the
/// row order. The slice is unbounded; the pane scrolls.
fn task_rows(
    cfg: &Config,
    mine: bool,
    me: Option<&str>,
    availability: Availability,
    status: Option<&str>,
) -> Vec<TaskRow> {
    let filter = Filter {
        mine,
        me: me.map(str::to_string),
        sort: if status.is_some() {
            SortKey::Updated
        } else {
            SortKey::Priority
        },
        limit: None,
        ..Filter::default()
    }
    .with_extra("status", status);
    tasks::list(cfg, &filter, availability)
        .unwrap_or_default()
        .into_iter()
        .map(|view| TaskRow {
            id: view.item.id,
            title: view.item.title,
            claimed_by: view.item.claimed_by,
        })
        .collect()
}

/// The activity pane: the recent-activity lens, capped by its own default limit. `mine` is
/// handed to the lens, which resolves the acting identity itself.
fn activity_rows(cfg: &Config, mine: bool) -> Vec<ActivityRow> {
    activity::recent_activity_in(
        cfg,
        None,
        None,
        mine,
        activity::DEFAULT_LIMIT,
        &activity::DEFAULT_SPACES,
    )
    .unwrap_or_default()
    .iter()
    .map(to_activity_row)
    .collect()
}

/// One seven-key activity row, read by column.
fn to_activity_row(entry: &Json) -> ActivityRow {
    ActivityRow {
        id: text_of(entry, "id"),
        kind: text_of(entry, "type"),
        title: text_of(entry, "title"),
        owner: option_text_of(entry, "owner"),
        claimed_by: option_text_of(entry, "claimed_by"),
        path: text_of(entry, "path"),
        mtime_seconds: entry.get("mtime").and_then(Json::as_f64).unwrap_or(0.0),
    }
}

/// The vault-health pane: the report's counts and findings plus the search-health line.
fn health_summary(report: &Json, cfg: &Config) -> HealthSummary {
    let search = crate::search::health::payload(cfg);
    let watcher = report.get("watcher");
    HealthSummary {
        notes_native: count_of(report, "notes"),
        notes_foreign: count_of(report, "notes_foreign"),
        dangling_links: string_list(report.get("dangling_links")),
        dangling_links_total: count_of(report, "dangling_links_total"),
        stale_locks: report
            .get("stale_locks")
            .and_then(Json::as_array)
            .map_or(0, |locks| locks.len() as u64),
        watcher_running: watcher
            .and_then(|value| value.get("running"))
            .and_then(Json::as_bool)
            .unwrap_or(false),
        watcher_pid: watcher
            .and_then(|value| value.get("pid"))
            .and_then(Json::as_u64)
            .and_then(|pid| u32::try_from(pid).ok()),
        search_mode: search
            .get("mode")
            .and_then(Json::as_str)
            .unwrap_or("fallback")
            .to_string(),
        search_reason: search
            .get("reason")
            .and_then(Json::as_str)
            .map(str::to_string),
    }
}

/// A count column: `0` when the key is absent or not a number.
fn count_of(value: &Json, key: &str) -> u64 {
    value.get(key).and_then(Json::as_u64).unwrap_or(0)
}

/// A string column: `""` when the key is absent or null.
fn text_of(entry: &Json, key: &str) -> String {
    entry
        .get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An identity column: `None` when absent, null or empty.
fn option_text_of(entry: &Json, key: &str) -> Option<String> {
    entry
        .get(key)
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// A string array, or an empty list when the key is absent or not an array.
fn string_list(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
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
    use std::fs;
    use std::path::Path;

    /// A fresh `updated` stamp, so no fixture task reads as stale against the status window.
    fn stamp() -> String {
        crate::timefmt::iso_z(&crate::timefmt::now_utc())
    }

    /// One note file, optionally owned or claimed.
    fn note(dir: &Path, rel: &str, id: &str, title: &str, owner: Option<&str>, body: &str) {
        let owner = owner.map_or("null".to_string(), str::to_string);
        let text = format!(
            "---\nid: {id}\ntype: note\ntitle: {title}\ntags: []\nowner: {owner}\n\
             created: 2026-01-01T00:00:00Z\nupdated: {}\nrelated: []\n---\n\n{body}\n",
            stamp()
        );
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// One open task file, optionally claimed and blocked.
    fn task(
        dir: &Path,
        id: &str,
        title: &str,
        status: &str,
        owner: &str,
        claimed_by: Option<&str>,
        blocked_by: &[&str],
    ) {
        let claimed = claimed_by.map_or("null".to_string(), str::to_string);
        let blockers: String = blocked_by
            .iter()
            .map(|blocker| format!("\"{blocker}\""))
            .collect::<Vec<String>>()
            .join(", ");
        let text = format!(
            "---\nid: {id}\ntype: task\ntitle: {title}\ntags: []\nowner: {owner}\n\
             created: 2026-01-01T00:00:00Z\nupdated: {}\nrelated: []\nstatus: {status}\n\
             priority: null\nclaimed_by: {claimed}\nproject: null\nblocks: []\n\
             blocked_by: [{blockers}]\n---\n\nbody\n",
            stamp()
        );
        let path = dir.join("tasks").join("open").join(format!("{id}.md"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// The scenario vault: two agents, a claimed, a blocked and a ready task, a dangling
    /// link, an adopted note (foreign stem, mesh id) and a foreign note (no id at all).
    fn fixture(dir: &Path) {
        note(
            dir,
            "notes/team-sol.md",
            "n-adopt",
            "Team solution",
            Some("alice"),
            "Context on [[Missing Page]].",
        );
        note(
            dir,
            "notes/n-bob.md",
            "n-bob",
            "Bob's native note",
            Some("bob"),
            "Bob's words.",
        );
        fs::write(
            dir.join("notes/loose.md"),
            "# Loose Heading\n\nno frontmatter here\n",
        )
        .unwrap();
        task(
            dir,
            "t-claimed",
            "Claimed work",
            "claimed",
            "alice",
            Some("alice"),
            &[],
        );
        task(
            dir,
            "t-blocker",
            "Blocking work",
            "open",
            "alice",
            None,
            &[],
        );
        task(
            dir,
            "t-blocked",
            "Blocked work",
            "open",
            "alice",
            None,
            &["t-blocker"],
        );
        task(dir, "t-ready", "Ready work", "open", "bob", None, &[]);
    }

    fn sorted_task_ids(rows: &[TaskRow]) -> Vec<String> {
        let mut ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
        ids.sort();
        ids
    }

    fn sorted_activity_ids(rows: &[ActivityRow]) -> Vec<String> {
        let mut ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
        ids.sort();
        ids
    }

    #[test]
    fn every_pane_reads_one_pass_of_the_domain_surfaces() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        fixture(dir.path());
        let frame = snapshot(&cfg, false);

        assert_eq!(
            frame.agents,
            vec![
                AgentRow {
                    identity: "alice".to_string(),
                    owns_open: 2,
                    claimed: 1,
                    stale_claims: 0,
                    notes_owned: 1,
                    notes_claimed: 0,
                },
                AgentRow {
                    identity: "bob".to_string(),
                    owns_open: 1,
                    claimed: 0,
                    stale_claims: 0,
                    notes_owned: 1,
                    notes_claimed: 0,
                },
            ]
        );

        assert_eq!(
            sorted_task_ids(&frame.tasks.ready),
            ["t-blocker", "t-ready"]
        );
        assert_eq!(sorted_task_ids(&frame.tasks.blocked), ["t-blocked"]);
        assert_eq!(sorted_task_ids(&frame.tasks.claimed), ["t-claimed"]);
        assert_eq!(frame.tasks.claimed[0].title, "Claimed work");
        assert_eq!(frame.tasks.claimed[0].claimed_by.as_deref(), Some("alice"));

        // The feed is the notes+tasks corpus: two notes and four tasks; the foreign note
        // carries no id and never appears.
        assert_eq!(
            sorted_activity_ids(&frame.activity),
            [
                "n-adopt",
                "n-bob",
                "t-blocked",
                "t-blocker",
                "t-claimed",
                "t-ready"
            ]
        );
        let claimed_row = frame
            .activity
            .iter()
            .find(|row| row.id == "t-claimed")
            .unwrap();
        assert_eq!(claimed_row.kind, "task");
        assert_eq!(claimed_row.claimed_by.as_deref(), Some("alice"));
        assert_eq!(claimed_row.owner.as_deref(), Some("alice"));
        assert!(!claimed_row.path.is_empty());

        assert_eq!(frame.health.notes_native, 2);
        assert_eq!(frame.health.notes_foreign, 1);
        assert_eq!(frame.health.dangling_links, ["Missing Page"]);
        assert_eq!(frame.health.dangling_links_total, 1);
        assert_eq!(frame.health.stale_locks, 0);
        assert!(!frame.health.watcher_running);
        assert_eq!(frame.health.watcher_pid, None);
        assert_eq!(frame.health.search_mode, "fallback");
        assert_eq!(
            frame.health.search_reason.as_deref(),
            Some(crate::search::health::REASON_COLLECTION)
        );
    }

    #[test]
    fn mine_narrows_the_agents_tasks_and_activity_panes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path()).with_agent(Some("alice"));
        fixture(dir.path());
        let frame = snapshot(&cfg, true);

        assert_eq!(frame.agents.len(), 1);
        assert_eq!(frame.agents[0].identity, "alice");
        assert_eq!(frame.agents[0].notes_owned, 1);

        assert_eq!(sorted_task_ids(&frame.tasks.ready), ["t-blocker"]);
        assert_eq!(sorted_task_ids(&frame.tasks.blocked), ["t-blocked"]);
        assert_eq!(sorted_task_ids(&frame.tasks.claimed), ["t-claimed"]);

        assert_eq!(
            sorted_activity_ids(&frame.activity),
            ["n-adopt", "t-blocked", "t-blocker", "t-claimed"]
        );

        // Vault health is global: the mine filter never narrows it.
        assert_eq!(frame.health.notes_native, 2);
        assert_eq!(frame.health.notes_foreign, 1);
    }

    #[test]
    fn mine_without_an_identity_narrows_to_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.core.agent = None;
        fixture(dir.path());
        let frame = snapshot(&cfg, true);

        // `--mine` with no identity configured matches nothing, the discipline every other
        // `--mine` keeps — never "every unclaimed row".
        assert!(frame.agents.is_empty());
        assert_eq!(frame.tasks, TasksByState::default());
        assert!(frame.activity.is_empty());
        // Without the filter the same vault still reports everything.
        let all = snapshot(&cfg, false);
        assert_eq!(all.agents.len(), 2);
        assert_eq!(all.tasks.claimed.len(), 1);
        assert_eq!(all.activity.len(), 6);
    }

    #[test]
    fn an_empty_vault_renders_a_calm_empty_frame() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let frame = snapshot(&cfg, false);

        assert!(frame.agents.is_empty());
        assert_eq!(frame.tasks, TasksByState::default());
        assert!(frame.activity.is_empty());
        assert_eq!(frame.health.notes_native, 0);
        assert_eq!(frame.health.notes_foreign, 0);
        assert!(frame.health.dangling_links.is_empty());
        assert_eq!(frame.health.dangling_links_total, 0);
        assert_eq!(frame.health.stale_locks, 0);
        assert!(!frame.health.watcher_running);
        assert_eq!(frame.health.search_mode, "fallback");
    }

    #[test]
    fn one_corrupt_file_degrades_its_own_pane_and_not_the_frame() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        fixture(dir.path());
        let before = snapshot(&cfg, false);
        // The base frame is real before the corrupt files land, so the equality below is
        // not vacuously true.
        assert_eq!(before.agents.len(), 2);
        assert_eq!(before.tasks.claimed.len(), 1);
        assert_eq!(before.activity.len(), 6);

        fs::write(
            dir.path().join("notes/n-bad.md"),
            "---\ntitle: [oops\n---\n\nx",
        )
        .unwrap();
        fs::write(
            dir.path().join("tasks/open/t-bad.md"),
            "---\nid: t-bad\nstatus: [oops\n---\n\nx",
        )
        .unwrap();
        let after = snapshot(&cfg, false);

        assert_eq!(
            after, before,
            "a corrupt file must not move any other number"
        );
    }
}
