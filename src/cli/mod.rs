//! The complete clap tree. Frozen after phase 0 — no implementation agent edits this file.
//!
//! Every command, subcommand, positional, flag, default and help string lives here; each verb
//! file only implements `run`. `--json`/`--quiet` are declared on the root *and* on every
//! non-admin subcommand, then merged by [`crate::ctx::Ctx::coalesce`] (the R6 flag contract).

pub mod admin;
pub mod asset;
pub mod globals;
pub mod lens;
pub mod memory;
pub mod note;
pub mod out;
pub mod scratch;
pub mod search;
pub mod task;
pub mod task_dep;
pub mod watch;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::ctx::Ctx;
use crate::error::Result;

/// The root help line. Unchanged from the Python CLI.
pub const ROOT_ABOUT: &str =
    "Three verbs, one folder, one mesh — notes + search = shared memory, tasks = coordination + handoff.";

const OWNER_WRITE_HELP: &str = "Owner identity (must be in [tasks].collections).";
const OWNER_FILTER_HELP: &str = "Filter by exact owner.";
const TAGS_CSV_HELP: &str = "Comma-separated tags.";
const TAGS_FILTER_HELP: &str = "Comma-separated tag filter (AND).";
const ANY_TAG_HELP: &str = "Switch --tags to OR semantics.";
const LIMIT_HELP: &str = "Cap the number of results.";
const SECTION_HELP: &str = "Append under this '## section', creating it when absent.";
const TIMESTAMP_HELP: &str = "Prefix the appended block with an attribution stamp.";
const FORCE_DELETE_HELP: &str = "Delete without the confirmation prompt.";
const SPACE_HELP: &str = "Comma-separated spaces to read (default: [search].spaces).";

/// `--json` / `--quiet`, redeclared on every non-admin subcommand.
#[derive(Args, Debug, Clone, Default)]
pub struct OutFlags {
    /// Machine-readable JSON output.
    #[arg(long, help = "Machine-readable JSON output.")]
    pub json: bool,
    /// IDs only; suppress stderr notes.
    #[arg(long, help = "IDs only; suppress stderr notes.")]
    pub quiet: bool,
}

/// `mesh` — the root command.
#[derive(Parser, Debug)]
#[command(
    name = "mesh",
    about = ROOT_ABOUT,
    disable_version_flag = true,
    disable_help_subcommand = true,
    subcommand_required = false,
    arg_required_else_help = false
)]
pub struct Cli {
    #[arg(long, help = "Show version and exit.")]
    pub version: bool,
    #[arg(long, help = "Machine-readable JSON output.")]
    pub json: bool,
    #[arg(long, help = "IDs only; suppress stderr notes.")]
    pub quiet: bool,
    #[arg(long, value_name = "TEXT", help = "Act as this agent identity.")]
    pub owner: Option<String>,
    #[arg(long, help = "Filter to owner or claimed_by == me.")]
    pub mine: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Config file to read (above $MESH_CONFIG_PATH)."
    )]
    pub config: Option<PathBuf>,
    #[arg(
        long,
        value_name = "PATH",
        help = "Vault folder to use (above $MESH_VAULT and [core].vault_path)."
    )]
    pub vault: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// The root command list, in fixed `--help` order.
#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(display_order = 1, about = "Capture knowledge as Markdown.")]
    Note(NoteArgs),
    #[command(display_order = 2, about = "Coordinate work as claimable task files.")]
    Task(TaskArgs),
    #[command(
        display_order = 3,
        about = "Recall across notes + tasks: ranked query, or an exact tag pull (--tags)."
    )]
    Search(SearchArgs),
    #[command(
        display_order = 4,
        about = "Remember what an agent learned about the operator."
    )]
    Memory(MemoryArgs),
    #[command(
        display_order = 5,
        about = "Keep this session's working state, per agent."
    )]
    Scratch(ScratchArgs),
    #[command(
        display_order = 6,
        about = "Store files beside the vault, content-addressed."
    )]
    Asset(AssetArgs),
    #[command(
        display_order = 7,
        about = "Write ~/.mesh/config.toml (or $MESH_CONFIG_PATH); --force to overwrite."
    )]
    Init(InitArgs),
    #[command(
        display_order = 8,
        about = "Report vault health (counts, freshness, links, locks)."
    )]
    Status(StatusArgs),
    #[command(
        display_order = 9,
        about = "Rebuild the search index (delegates to indexed)."
    )]
    Reindex(ReindexArgs),
    #[command(
        name = "recent-activity",
        display_order = 10,
        about = "List recent vault changes (newest first; --since, --mine)."
    )]
    RecentActivity(RecentActivityArgs),
    #[command(
        name = "build-context",
        display_order = 11,
        about = "Expand the related graph around a seed id (BFS to --depth)."
    )]
    BuildContext(BuildContextArgs),
    #[command(
        display_order = 12,
        about = "Query what's connected to a seed id (tree, or JSON nodes+edges)."
    )]
    Graph(GraphArgs),
    #[command(
        display_order = 13,
        about = "Show a project note and the tasks scoped to it (read-only lens)."
    )]
    Project(ProjectArgs),
    #[command(
        name = "session-start",
        display_order = 14,
        about = "Warm-start payload: my tasks + mentions of me + recent activity."
    )]
    SessionStart(SessionStartArgs),
    #[command(
        display_order = 15,
        about = "Watch the vault and keep the search index fresh (foreground)."
    )]
    Watch(WatchArgs),
    #[command(display_order = 16, about = "Inspect and edit the mesh config.")]
    Config(ConfigArgs),
    #[command(display_order = 17, about = "Print a shell completion script.")]
    Completions(CompletionsArgs),
    #[command(display_order = 18, about = "Run the stdio MCP server.")]
    Mcp(McpArgs),
    #[command(hide = true, about = "Removed — use 'mesh watch'.")]
    Daemon(DaemonArgs),
}

// ---------------------------------------------------------------------------------------
// note
// ---------------------------------------------------------------------------------------

/// `mesh note`.
#[derive(Args, Debug)]
pub struct NoteArgs {
    #[command(subcommand)]
    pub sub: Option<NoteSub>,
}

/// `mesh note …` in registration order.
#[derive(Subcommand, Debug)]
pub enum NoteSub {
    #[command(display_order = 1, about = "Create a note.")]
    New {
        #[arg(help = "Note title.")]
        title: String,
        #[arg(
            long = "type",
            default_value = "note",
            value_name = "TEXT",
            help = "Note type: note | log | decision | reference | project."
        )]
        note_type: String,
        #[arg(long, value_name = "TEXT", help = TAGS_CSV_HELP)]
        tags: Option<String>,
        #[arg(long, value_name = "TEXT", help = OWNER_WRITE_HELP)]
        owner: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Note body text.")]
        body: Option<String>,
        #[arg(long, value_name = "PATH", help = "Read the body from this file.")]
        file: Option<PathBuf>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 2, about = "Append a block to a note's body.")]
    Append {
        #[arg(help = "Note id or title slug.")]
        target: String,
        #[arg(help = "Text to append.")]
        text: String,
        #[arg(long, value_name = "TEXT", help = SECTION_HELP)]
        section: Option<String>,
        #[arg(long, help = TIMESTAMP_HELP)]
        timestamp: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 3, about = "Update a note's tags, type or title.")]
    Update {
        #[arg(help = "Note id or title slug.")]
        target: String,
        #[arg(long, value_name = "TEXT", help = crate::domain::TAG_SPEC_SEMANTICS)]
        tags: Option<String>,
        #[arg(
            long = "type",
            value_name = "TEXT",
            help = "Move the note to this type's folder: note | log | decision | reference | project."
        )]
        new_type: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Rewrite the note title.")]
        title: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 4, about = "Read one note.")]
    Get {
        #[arg(help = "Note id or title slug.")]
        target: String,
        #[arg(long, help = "Print the whole body instead of a preview.")]
        full: bool,
        #[arg(long = "meta-only", help = "Print the metadata block only.")]
        meta_only: bool,
        #[arg(long, help = "Print the related ids instead of the note.")]
        related: bool,
        #[arg(
            long,
            help = "Also resolve non-mesh Markdown by stem or relative path."
        )]
        foreign: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 5, about = "List notes.")]
    List {
        #[arg(long, value_name = "TEXT", help = TAGS_FILTER_HELP)]
        tags: Option<String>,
        #[arg(long = "any-tag", help = ANY_TAG_HELP)]
        any_tag: bool,
        #[arg(long, value_name = "TEXT", help = OWNER_FILTER_HELP)]
        owner: Option<String>,
        #[arg(long = "type", value_name = "TEXT", help = "Filter by note type.")]
        note_type: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Recency: 7d or an ISO date.")]
        since: Option<String>,
        #[arg(
            long,
            default_value = "updated",
            value_name = "TEXT",
            help = "updated | created | title."
        )]
        sort: String,
        #[arg(long, default_value_t = 20, value_name = "INT", allow_negative_numbers = true, help = LIMIT_HELP)]
        limit: i64,
        #[arg(
            long,
            help = "Also list non-mesh Markdown (id null, title from the first # H1)."
        )]
        foreign: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 6, about = "Delete a note.")]
    Delete {
        #[arg(help = "Note id or title slug.")]
        target: String,
        #[arg(long, help = FORCE_DELETE_HELP)]
        force: bool,
        #[command(flatten)]
        out: OutFlags,
    },
}

// ---------------------------------------------------------------------------------------
// task
// ---------------------------------------------------------------------------------------

/// `mesh task`.
#[derive(Args, Debug)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub sub: Option<TaskSub>,
}

/// `mesh task …` in registration order.
#[derive(Subcommand, Debug)]
pub enum TaskSub {
    #[command(display_order = 1, about = "Create a task.")]
    New {
        #[arg(help = "Task title.")]
        title: String,
        #[arg(long, value_name = "TEXT", help = "Priority label, e.g. high.")]
        priority: Option<String>,
        #[arg(long, value_name = "TEXT", help = TAGS_CSV_HELP)]
        tags: Option<String>,
        #[arg(long, value_name = "TEXT", help = OWNER_WRITE_HELP)]
        owner: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Task body text.")]
        body: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Soft-link this task to a project note id."
        )]
        project: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Comma-separated task ids this blocks."
        )]
        blocks: Option<String>,
        #[arg(
            long = "blocked-by",
            value_name = "TEXT",
            help = "Comma-separated task ids blocking this."
        )]
        blocked_by: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 2, about = "Update a task's fields.")]
    Update {
        #[arg(help = "Task id.")]
        task_id: String,
        #[arg(long, value_name = "TEXT", help = "Set the priority label.")]
        priority: Option<String>,
        #[arg(long, value_name = "TEXT", help = crate::domain::TAG_SPEC_SEMANTICS)]
        tags: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Rewrite the task title.")]
        title: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Set the project soft link (a project note id)."
        )]
        project: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Reassign owner (must be in [tasks].collections)."
        )]
        owner: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Replace the blocks list (comma-separated)."
        )]
        blocks: Option<String>,
        #[arg(
            long = "blocked-by",
            value_name = "TEXT",
            help = "Replace the blocked_by list (comma-separated)."
        )]
        blocked_by: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 3, about = "Append a block to a task's body.")]
    Append {
        #[arg(help = "Task id.")]
        task_id: String,
        #[arg(help = "Text to append.")]
        text: String,
        #[arg(long, value_name = "TEXT", help = SECTION_HELP)]
        section: Option<String>,
        #[arg(long, help = TIMESTAMP_HELP)]
        timestamp: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 4, about = "Claim a task (atomic test-and-set).")]
    Claim {
        #[arg(help = "Task id.")]
        task_id: String,
        #[arg(
            long,
            help = "Refuse to claim a task with an unsatisfied blocker (exit 5)."
        )]
        strict: bool,
        #[arg(
            long = "no-strict",
            help = "Claim even when blocked, warning on stderr."
        )]
        no_strict: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 5, about = "Release a claim.")]
    Release {
        #[arg(help = "Task id.")]
        task_id: String,
        #[arg(
            long,
            help = "Break another agent's claim (cooperation override, not auth)."
        )]
        force: bool,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Append this text via task append (e.g. why you're releasing)."
        )]
        note: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 6, about = "Finish a task.")]
    Finish {
        #[arg(help = "Task id.")]
        task_id: String,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Outcome text recorded under the ## Outcome section."
        )]
        outcome: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 7, about = "Cancel a task.")]
    Cancel {
        #[arg(help = "Task id.")]
        task_id: String,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Reason recorded under the ## Cancelled section."
        )]
        reason: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 8, about = "Read one task.")]
    Get {
        #[arg(help = "Task id.")]
        task_id: String,
        #[arg(long, help = "Print the whole body instead of a preview.")]
        full: bool,
        #[arg(long = "meta-only", help = "Print the metadata block only.")]
        meta_only: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 9, about = "List tasks.")]
    List {
        #[arg(
            long,
            value_name = "TEXT",
            help = "Filter by status (comma-separated union, e.g. open,claimed): open | claimed | done | cancelled."
        )]
        status: Option<String>,
        #[arg(long, value_name = "TEXT", help = OWNER_FILTER_HELP)]
        owner: Option<String>,
        #[arg(long, help = "Only tasks I own or have claimed.")]
        mine: bool,
        #[arg(long, value_name = "TEXT", help = TAGS_FILTER_HELP)]
        tags: Option<String>,
        #[arg(long = "any-tag", help = ANY_TAG_HELP)]
        any_tag: bool,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Only tasks scoped to this project note id."
        )]
        project: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Recency floor: updated within <dur> (7d) or since an ISO date."
        )]
        since: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Recency ceiling: not updated within <dur> (2d) — the inverse of --since."
        )]
        stale: Option<String>,
        #[arg(
            long,
            help = "Only takeable work: status open and unclaimed (defaults --sort to priority)."
        )]
        available: bool,
        #[arg(
            long,
            help = "Only ready work: available and unblocked (defaults --sort to priority)."
        )]
        ready: bool,
        #[arg(long, help = "Only open or claimed tasks with an unsatisfied blocker.")]
        blocked: bool,
        #[arg(
            long,
            value_name = "TEXT",
            help = "updated | created | title | priority (default: updated, or priority with --available)."
        )]
        sort: Option<String>,
        #[arg(long, default_value_t = 20, value_name = "INT", allow_negative_numbers = true, help = LIMIT_HELP)]
        limit: i64,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 10, about = "Delete a task.")]
    Delete {
        #[arg(help = "Task id.")]
        task_id: String,
        #[arg(long, help = FORCE_DELETE_HELP)]
        force: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 11, about = "Add blocking edges to a task.")]
    Block {
        #[arg(help = "Task id to block.")]
        task_id: String,
        #[arg(long, value_name = "TEXT", help = "Comma-separated blocker task ids.")]
        on: String,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 12, about = "Remove blocking edges from a task.")]
    Unblock {
        #[arg(help = "Task id to unblock.")]
        task_id: String,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Comma-separated blocker task ids to drop."
        )]
        on: Option<String>,
        #[arg(long, help = "Drop every blocker.")]
        all: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(
        display_order = 13,
        about = "Pick the next ready task, optionally claiming it."
    )]
    Next {
        #[arg(long, help = "Claim the selected task in the same invocation.")]
        claim: bool,
        #[arg(
            long,
            help = "With --claim, exit 5 rather than skipping a blocked candidate."
        )]
        strict: bool,
        #[arg(long, help = "Only tasks I own or have claimed.")]
        mine: bool,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Only tasks scoped to this project note id."
        )]
        project: Option<String>,
        #[arg(long, value_name = "TEXT", help = TAGS_FILTER_HELP)]
        tags: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
}

// ---------------------------------------------------------------------------------------
// memory
// ---------------------------------------------------------------------------------------

/// `mesh memory`.
#[derive(Args, Debug)]
pub struct MemoryArgs {
    #[command(subcommand)]
    pub sub: Option<MemorySub>,
}

/// `mesh memory …` in registration order.
#[derive(Subcommand, Debug)]
pub enum MemorySub {
    #[command(display_order = 1, about = "Record a memory.")]
    New {
        #[arg(help = "Memory title.")]
        title: String,
        #[arg(
            long,
            default_value = "fact",
            value_name = "TEXT",
            help = "Memory kind: fact | preference | procedure | insight | episode."
        )]
        kind: String,
        #[arg(
            long,
            default_value = "shared",
            value_name = "TEXT",
            help = "Visibility: shared | private (a courtesy filter, never authorisation)."
        )]
        scope: String,
        #[arg(
            long,
            default_value_t = 3,
            value_name = "INT",
            help = "Importance 1..5."
        )]
        importance: i64,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Where this came from; free text, never interpreted."
        )]
        source: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Soft TTL: 7d, 12h, 2w or an ISO datetime."
        )]
        expires: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Mark this m- id superseded by the new memory."
        )]
        supersedes: Option<String>,
        #[arg(long, value_name = "TEXT", help = TAGS_CSV_HELP)]
        tags: Option<String>,
        #[arg(long, value_name = "TEXT", help = OWNER_WRITE_HELP)]
        owner: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Memory body text.")]
        body: Option<String>,
        #[arg(long, value_name = "PATH", help = "Read the body from this file.")]
        file: Option<PathBuf>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 2, about = "Append a block to a memory's body.")]
    Append {
        #[arg(help = "Memory id or title slug.")]
        target: String,
        #[arg(help = "Text to append.")]
        text: String,
        #[arg(long, value_name = "TEXT", help = SECTION_HELP)]
        section: Option<String>,
        #[arg(long, help = TIMESTAMP_HELP)]
        timestamp: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 3, about = "Update a memory's fields.")]
    Update {
        #[arg(help = "Memory id or title slug.")]
        target: String,
        #[arg(long, value_name = "TEXT", help = crate::domain::TAG_SPEC_SEMANTICS)]
        tags: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Rewrite the memory title.")]
        title: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Set the kind.")]
        kind: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Set the scope: shared | private.")]
        scope: Option<String>,
        #[arg(long, value_name = "INT", help = "Set importance 1..5.")]
        importance: Option<i64>,
        #[arg(long, value_name = "TEXT", help = "Set the source.")]
        source: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Set the soft TTL (7d, an ISO datetime, or 'none' to clear)."
        )]
        expires: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Reassign owner (must be in [tasks].collections)."
        )]
        owner: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 4, about = "Read one memory.")]
    Get {
        #[arg(help = "Memory id or title slug.")]
        target: String,
        #[arg(long, help = "Print the whole body instead of a preview.")]
        full: bool,
        #[arg(long = "meta-only", help = "Print the metadata block only.")]
        meta_only: bool,
        #[arg(long, help = "Print the related ids instead of the memory.")]
        related: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 5, about = "List memories.")]
    List {
        #[arg(long, value_name = "TEXT", help = "Filter by kind.")]
        kind: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Filter by scope.")]
        scope: Option<String>,
        #[arg(long, value_name = "TEXT", help = TAGS_FILTER_HELP)]
        tags: Option<String>,
        #[arg(long = "any-tag", help = ANY_TAG_HELP)]
        any_tag: bool,
        #[arg(long, value_name = "TEXT", help = OWNER_FILTER_HELP)]
        owner: Option<String>,
        #[arg(long, help = "Only memories I own.")]
        mine: bool,
        #[arg(
            long = "min-importance",
            value_name = "INT",
            help = "Keep importance >= N."
        )]
        min_importance: Option<i64>,
        #[arg(long, value_name = "TEXT", help = "Recency: 7d or an ISO date.")]
        since: Option<String>,
        #[arg(
            long = "include-expired",
            help = "Include memories past their soft TTL."
        )]
        include_expired: bool,
        #[arg(long = "include-superseded", help = "Include superseded memories.")]
        include_superseded: bool,
        #[arg(
            long,
            default_value = "updated",
            value_name = "TEXT",
            help = "updated | created | title | importance."
        )]
        sort: String,
        #[arg(long, default_value_t = 20, value_name = "INT", allow_negative_numbers = true, help = LIMIT_HELP)]
        limit: i64,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 6, about = "Recall memories relevant to a query.")]
    Recall {
        #[arg(help = "Query text.")]
        query: String,
        #[arg(long, value_name = "TEXT", help = "Filter by kind.")]
        kind: Option<String>,
        #[arg(long, value_name = "TEXT", help = TAGS_FILTER_HELP)]
        tags: Option<String>,
        #[arg(long, value_name = "TEXT", help = OWNER_FILTER_HELP)]
        owner: Option<String>,
        #[arg(long, help = "Only memories I own.")]
        mine: bool,
        #[arg(
            long = "min-importance",
            value_name = "INT",
            help = "Keep importance >= N."
        )]
        min_importance: Option<i64>,
        #[arg(
            long,
            default_value_t = 10,
            value_name = "INT",
            allow_negative_numbers = true,
            help = "Cap the number of hits."
        )]
        limit: i64,
        #[arg(long, value_name = "FLOAT", help = "Min score to keep.")]
        threshold: Option<f64>,
        #[arg(
            long = "no-decay",
            help = "Drop the recency term from the ranking (audits)."
        )]
        no_decay: bool,
        #[arg(
            long = "include-expired",
            help = "Include memories past their soft TTL."
        )]
        include_expired: bool,
        #[arg(long = "meta-only", help = "Omit snippets from hits.")]
        meta_only: bool,
        #[arg(long, help = "Include the full Markdown body per hit.")]
        full: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 7, about = "Delete a memory.")]
    Forget {
        #[arg(help = "Memory id or title slug.", default_value = "")]
        target: String,
        #[arg(long, help = FORCE_DELETE_HELP)]
        force: bool,
        #[arg(long, help = "Forget every expired memory instead of one target.")]
        expired: bool,
        #[command(flatten)]
        out: OutFlags,
    },
}

// ---------------------------------------------------------------------------------------
// scratch
// ---------------------------------------------------------------------------------------

/// `mesh scratch`.
#[derive(Args, Debug)]
pub struct ScratchArgs {
    #[command(subcommand)]
    pub sub: Option<ScratchSub>,
}

/// `mesh scratch …` in registration order.
#[derive(Subcommand, Debug)]
pub enum ScratchSub {
    #[command(
        display_order = 1,
        about = "Write a scratch file (whole-body overwrite)."
    )]
    Set {
        #[arg(help = "Scratch name (slugified; it is the address).")]
        name: String,
        #[arg(
            value_name = "-",
            allow_hyphen_values = true,
            help = "Pass '-' to read the body from stdin."
        )]
        source: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Scratch body text.")]
        body: Option<String>,
        #[arg(long, value_name = "PATH", help = "Read the body from this file.")]
        file: Option<PathBuf>,
        #[arg(long, value_name = "TEXT", help = "Address this agent's namespace.")]
        agent: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 2, about = "Append a block to a scratch file.")]
    Append {
        #[arg(help = "Scratch name.")]
        name: String,
        #[arg(help = "Text to append.")]
        text: String,
        #[arg(long, value_name = "TEXT", help = SECTION_HELP)]
        section: Option<String>,
        #[arg(long, help = TIMESTAMP_HELP)]
        timestamp: bool,
        #[arg(long, value_name = "TEXT", help = "Address this agent's namespace.")]
        agent: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 3, about = "Print a scratch body verbatim.")]
    Get {
        #[arg(help = "Scratch name.")]
        name: String,
        #[arg(long, value_name = "TEXT", help = "Address this agent's namespace.")]
        agent: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 4, about = "List scratch files.")]
    List {
        #[arg(long, value_name = "TEXT", help = "Address this agent's namespace.")]
        agent: Option<String>,
        #[arg(long = "all-agents", help = "List every agent's scratch files.")]
        all_agents: bool,
        #[arg(long, value_name = "TEXT", help = "Recency: 7d or an ISO date.")]
        since: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 5, about = "Delete a scratch file.")]
    Clear {
        #[arg(help = "Scratch name.")]
        name: String,
        #[arg(long, value_name = "TEXT", help = "Address this agent's namespace.")]
        agent: Option<String>,
        #[arg(long, help = FORCE_DELETE_HELP)]
        force: bool,
        #[command(flatten)]
        out: OutFlags,
    },
}

// ---------------------------------------------------------------------------------------
// asset
// ---------------------------------------------------------------------------------------

/// `mesh asset`.
#[derive(Args, Debug)]
pub struct AssetArgs {
    #[command(subcommand)]
    pub sub: Option<AssetSub>,
}

/// `mesh asset …` in registration order.
#[derive(Subcommand, Debug)]
pub enum AssetSub {
    #[command(
        display_order = 1,
        about = "Store a file, content-addressed, with a sidecar."
    )]
    Add {
        #[arg(help = "Path to the file to copy in.")]
        path: PathBuf,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Title (default: the source basename)."
        )]
        title: Option<String>,
        #[arg(long, value_name = "TEXT", help = TAGS_CSV_HELP)]
        tags: Option<String>,
        #[arg(long, value_name = "TEXT", help = OWNER_WRITE_HELP)]
        owner: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Sidecar body text.")]
        caption: Option<String>,
        #[arg(
            long,
            value_name = "TEXT",
            help = "Attach to this note, task or memory id."
        )]
        attach: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 2, about = "Read an asset sidecar.")]
    Get {
        #[arg(help = "Asset id.")]
        asset_id: String,
        #[arg(long = "meta-only", help = "Print the metadata block only.")]
        meta_only: bool,
        #[arg(long, help = "Print the whole caption instead of a preview.")]
        full: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 3, about = "Print the absolute blob path.")]
    Path {
        #[arg(help = "Asset id.")]
        asset_id: String,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 4, about = "List assets.")]
    List {
        #[arg(long, value_name = "TEXT", help = TAGS_FILTER_HELP)]
        tags: Option<String>,
        #[arg(long = "any-tag", help = ANY_TAG_HELP)]
        any_tag: bool,
        #[arg(long, value_name = "TEXT", help = OWNER_FILTER_HELP)]
        owner: Option<String>,
        #[arg(long, help = "Only assets I own.")]
        mine: bool,
        #[arg(
            long = "media-type",
            value_name = "TEXT",
            help = "Filter by media type."
        )]
        media_type: Option<String>,
        #[arg(long, value_name = "TEXT", help = "Recency: 7d or an ISO date.")]
        since: Option<String>,
        #[arg(
            long,
            default_value = "updated",
            value_name = "TEXT",
            help = "updated | created | title | bytes."
        )]
        sort: String,
        #[arg(long, default_value_t = 20, value_name = "INT", allow_negative_numbers = true, help = LIMIT_HELP)]
        limit: i64,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(
        display_order = 5,
        about = "Attach an asset to a note, task or memory."
    )]
    Attach {
        #[arg(help = "Asset id.")]
        asset_id: String,
        #[arg(help = "Target note, task or memory id.")]
        target: String,
        #[arg(long, value_name = "TEXT", help = SECTION_HELP)]
        section: Option<String>,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 6, about = "Detach an asset from an entity.")]
    Detach {
        #[arg(help = "Asset id.")]
        asset_id: String,
        #[arg(help = "Target note, task or memory id.")]
        target: String,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 7, about = "Delete an asset and its blob.")]
    Remove {
        #[arg(help = "Asset id.")]
        asset_id: String,
        #[arg(long, help = "Delete even when other entities still reference it.")]
        force: bool,
        #[command(flatten)]
        out: OutFlags,
    },
    #[command(display_order = 8, about = "Report orphan blobs and sidecars.")]
    Gc {
        #[arg(long, help = "Actually remove what the report lists.")]
        apply: bool,
        #[command(flatten)]
        out: OutFlags,
    },
}

// ---------------------------------------------------------------------------------------
// search and the lenses
// ---------------------------------------------------------------------------------------

/// `mesh search`.
#[derive(Args, Debug)]
pub struct SearchArgs {
    #[arg(help = "Query text (omit with --tags for a tag pull).")]
    pub query: Option<String>,
    #[arg(long = "type", value_name = "TEXT", help = "Filter by note/task type.")]
    pub type_filter: Option<String>,
    #[arg(
        long,
        value_name = "TEXT",
        action = clap::ArgAction::Append,
        help = "Require all these tags (AND); repeatable."
    )]
    pub tags: Vec<String>,
    #[arg(long, value_name = "TEXT", help = OWNER_FILTER_HELP)]
    pub owner: Option<String>,
    #[arg(long, value_name = "TEXT", help = "Filter by task status.")]
    pub status: Option<String>,
    #[arg(long, value_name = "TEXT", help = "Filter by memory kind.")]
    pub kind: Option<String>,
    #[arg(long, value_name = "TEXT", help = SPACE_HELP)]
    pub space: Option<String>,
    #[arg(
        long,
        default_value = "auto",
        value_name = "TEXT",
        help = "Ranking engine: auto | indexed | builtin | substring."
    )]
    pub engine: String,
    #[arg(
        long,
        default_value_t = 10,
        value_name = "INT",
        allow_negative_numbers = true,
        help = "Cap the number of hits."
    )]
    pub limit: i64,
    #[arg(
        long,
        value_name = "FLOAT",
        help = "Min score to keep (unset: [search].threshold if explicit, else the engine's own floor)."
    )]
    pub threshold: Option<f64>,
    #[arg(long = "meta-only", help = "Omit body/snippet from hits.")]
    pub meta_only: bool,
    #[arg(long, help = "Include the full Markdown body per hit.")]
    pub full: bool,
    #[arg(
        long,
        help = "Report indexed reachability vs. the built-in engine (JSON), then exit."
    )]
    pub health: bool,
    #[command(flatten)]
    pub out: OutFlags,
}

/// `mesh recent-activity`.
#[derive(Args, Debug)]
pub struct RecentActivityArgs {
    #[arg(long, value_name = "TEXT", help = "Recency window: 7d, 12h, or ISO.")]
    pub since: Option<String>,
    #[arg(long, value_name = "TEXT", help = OWNER_FILTER_HELP)]
    pub owner: Option<String>,
    #[arg(long, help = "Filter to owner or claimed_by == me.")]
    pub mine: bool,
    #[arg(
        long,
        default_value_t = 20,
        value_name = "INT",
        allow_negative_numbers = true,
        help = "Cap the number of rows."
    )]
    pub limit: i64,
    #[arg(long, value_name = "TEXT", help = SPACE_HELP)]
    pub space: Option<String>,
    #[command(flatten)]
    pub out: OutFlags,
}

/// `mesh build-context`.
#[derive(Args, Debug)]
pub struct BuildContextArgs {
    #[arg(help = "Seed note/task id (n-… or t-…) to expand from.")]
    pub seed_id: String,
    #[arg(
        long,
        default_value_t = 1,
        value_name = "INT",
        help = "Hops to walk (0 = seed only; 1 = direct)."
    )]
    pub depth: i64,
    #[arg(long, value_name = "TEXT", help = SPACE_HELP)]
    pub space: Option<String>,
    #[command(flatten)]
    pub out: OutFlags,
}

/// `mesh graph`.
#[derive(Args, Debug)]
pub struct GraphArgs {
    #[arg(help = "Seed note/task id (n-… or t-…) to expand from.")]
    pub seed_id: String,
    #[arg(
        long,
        default_value_t = 1,
        value_name = "INT",
        help = "Hops to walk (0 = seed only; 1 = direct)."
    )]
    pub depth: i64,
    #[arg(
        long,
        default_value = "out",
        value_name = "TEXT",
        help = "Edge direction to walk: out (related, default), in (backlinks), both."
    )]
    pub direction: String,
    #[arg(long, value_name = "TEXT", help = SPACE_HELP)]
    pub space: Option<String>,
    #[command(flatten)]
    pub out: OutFlags,
}

/// `mesh project`.
#[derive(Args, Debug)]
pub struct ProjectArgs {
    #[arg(help = "Project note id (n-…) to scope to.")]
    pub project_id: String,
    #[arg(long, value_name = "TEXT", help = SPACE_HELP)]
    pub space: Option<String>,
    #[command(flatten)]
    pub out: OutFlags,
}

/// `mesh session-start`.
#[derive(Args, Debug)]
pub struct SessionStartArgs {
    #[arg(
        long,
        value_name = "TEXT",
        help = "Show this agent's warm start instead of mine."
    )]
    pub owner: Option<String>,
    #[arg(
        long,
        help = "Widen the activity half to every agent (task half stays mine)."
    )]
    pub team: bool,
    #[arg(
        long = "meta-only",
        help = "Omit note/task bodies (token-budget path)."
    )]
    pub meta_only: bool,
    #[arg(
        long = "no-memories",
        help = "Leave recalled memories out of the payload."
    )]
    pub no_memories: bool,
    #[arg(
        long,
        default_value_t = 0,
        value_name = "INT",
        help = "Character budget (0 = unbounded); trims bodies, then entries."
    )]
    pub budget: i64,
    #[arg(long, value_name = "TEXT", help = SPACE_HELP)]
    pub space: Option<String>,
    #[command(flatten)]
    pub out: OutFlags,
}

// ---------------------------------------------------------------------------------------
// admin
// ---------------------------------------------------------------------------------------

/// `mesh init`.
#[derive(Args, Debug)]
pub struct InitArgs {
    #[arg(
        long,
        value_name = "TEXT",
        help = "Vault folder ([core].vault_path). Defaults to ~/.mesh/vault."
    )]
    pub path: Option<String>,
    #[arg(
        long,
        value_name = "TEXT",
        help = "This agent's identity ([core].agent). Defaults to $MESH_AGENT, else 'agent'."
    )]
    pub agent: Option<String>,
    #[arg(
        long,
        value_name = "TEXT",
        help = "Comma-separated roster of valid --owner identities ([tasks].collections). Default: empty — an open roster, any owner string accepted."
    )]
    pub collections: Option<String>,
    #[arg(
        long = "search-collection",
        value_name = "TEXT",
        help = "indexed collection name ([search].collection). Default: unset."
    )]
    pub search_collection: Option<String>,
    #[arg(
        long,
        help = "Hybrid lexical+vector search via indexed ([search].hybrid). Default: on."
    )]
    pub hybrid: bool,
    #[arg(
        long = "no-hybrid",
        help = "Turn hybrid search off ([search].hybrid = false)."
    )]
    pub no_hybrid: bool,
    #[arg(
        long,
        value_name = "FLOAT",
        help = "Score floor ([search].threshold). Default: unset — the key is omitted so the engine keeps its own floor."
    )]
    pub threshold: Option<f64>,
    #[arg(
        long,
        default_value = "auto",
        value_name = "TEXT",
        help = "Default ranking engine ([search].engine): auto | indexed | builtin | substring."
    )]
    pub engine: String,
    #[arg(long, help = "Write the [spaces] block. Default: on.")]
    pub spaces: bool,
    #[arg(
        long = "no-spaces",
        help = "Leave the [spaces] block out of the config."
    )]
    pub no_spaces: bool,
    #[arg(long, help = "Expose an existing Obsidian vault: notes = \".\".")]
    pub obsidian: bool,
    #[arg(long, help = "Overwrite an existing config. Default: refuse.")]
    pub force: bool,
}

/// `mesh status`.
#[derive(Args, Debug)]
pub struct StatusArgs {}

/// `mesh reindex`.
#[derive(Args, Debug)]
pub struct ReindexArgs {
    #[arg(long, value_name = "TEXT", help = "Comma-separated spaces to reindex.")]
    pub space: Option<String>,
}

/// `mesh watch`.
#[derive(Args, Debug)]
pub struct WatchArgs {
    #[arg(long, help = "Do one reconcile + index sweep and exit.")]
    pub once: bool,
    #[arg(long = "no-index", help = "Reconcile only; never drive indexed.")]
    pub no_index: bool,
    #[arg(
        long = "no-reconcile",
        help = "Index only; never move a misfiled file."
    )]
    pub no_reconcile: bool,
    #[arg(
        long,
        default_value_t = 250,
        value_name = "INT",
        help = "Debounce window in milliseconds."
    )]
    pub debounce: u64,
    #[arg(long, value_name = "TEXT", help = SPACE_HELP)]
    pub space: Option<String>,
    #[arg(long, help = "Emit an NDJSON event log on stdout.")]
    pub json: bool,
}

/// `mesh config`.
#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub sub: Option<ConfigSub>,
}

/// `mesh config …`.
#[derive(Subcommand, Debug)]
pub enum ConfigSub {
    #[command(display_order = 1, about = "Print the resolved config path.")]
    Path,
    #[command(
        display_order = 2,
        about = "Print the effective config, spaces and sandbox roots."
    )]
    Show {
        #[arg(long, help = "Machine-readable JSON output.")]
        json: bool,
    },
    #[command(display_order = 3, about = "Print one config value.")]
    Get {
        #[arg(help = "Dotted key, e.g. core.agent.")]
        key: String,
    },
    #[command(display_order = 4, about = "Set one config value in place.")]
    Set {
        #[arg(help = "Dotted key, e.g. core.agent.")]
        key: String,
        #[arg(help = "New value.")]
        value: String,
    },
}

/// `mesh completions`.
#[derive(Args, Debug)]
pub struct CompletionsArgs {
    #[arg(value_enum, help = "Shell to generate a completion script for.")]
    pub shell: clap_complete::Shell,
}

/// `mesh mcp`.
#[derive(Args, Debug)]
pub struct McpArgs {}

/// `mesh daemon` — the hidden, non-spawning shim.
#[derive(Args, Debug)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub sub: Option<DaemonSub>,
}

/// `mesh daemon …`.
#[derive(Subcommand, Debug)]
pub enum DaemonSub {
    #[command(about = "Removed — never spawns; reports the watcher instead.")]
    Start,
    #[command(about = "Stop a running watcher, if there is one.")]
    Stop,
    #[command(about = "Report watcher liveness and the watch lock path.")]
    Status,
}

/// Print a command's long help to **stdout** and report exit code 2 (deviation 18).
pub fn help_to_stdout(path: &[&str]) -> i32 {
    let mut command = <Cli as clap::CommandFactory>::command();
    for name in path {
        let next = command.find_subcommand(name).cloned();
        match next {
            Some(sub) => command = sub,
            None => break,
        }
    }
    let text = command.render_long_help().to_string();
    out::line(text.trim_end());
    2
}

/// Route a parsed command to its verb file.
pub fn dispatch(ctx: &mut Ctx, command: Command) -> Result<i32> {
    match command {
        Command::Note(args) => match args.sub {
            None => Ok(help_to_stdout(&["note"])),
            Some(sub) => note::run(ctx, sub).map(|()| 0),
        },
        Command::Task(args) => match args.sub {
            None => Ok(help_to_stdout(&["task"])),
            Some(sub) if is_dep_verb(&sub) => task_dep::run(ctx, sub).map(|()| 0),
            Some(sub) => task::run(ctx, sub).map(|()| 0),
        },
        Command::Search(args) => search::run(ctx, args).map(|()| 0),
        Command::Memory(args) => match args.sub {
            None => Ok(help_to_stdout(&["memory"])),
            Some(sub) => memory::run(ctx, sub).map(|()| 0),
        },
        Command::Scratch(args) => match args.sub {
            None => Ok(help_to_stdout(&["scratch"])),
            Some(sub) => scratch::run(ctx, sub).map(|()| 0),
        },
        Command::Asset(args) => match args.sub {
            None => Ok(help_to_stdout(&["asset"])),
            Some(sub) => asset::run(ctx, sub).map(|()| 0),
        },
        Command::Init(args) => admin::init(ctx, args).map(|()| 0),
        Command::Status(args) => admin::status(ctx, args).map(|()| 0),
        Command::Reindex(args) => admin::reindex(ctx, args).map(|()| 0),
        Command::RecentActivity(args) => lens::recent_activity(ctx, args).map(|()| 0),
        Command::BuildContext(args) => lens::build_context(ctx, args).map(|()| 0),
        Command::Graph(args) => lens::graph(ctx, args).map(|()| 0),
        Command::Project(args) => lens::project(ctx, args).map(|()| 0),
        Command::SessionStart(args) => lens::session_start(ctx, args).map(|()| 0),
        Command::Watch(args) => watch::run(ctx, args).map(|()| 0),
        Command::Config(args) => match args.sub {
            None => Ok(help_to_stdout(&["config"])),
            Some(sub) => admin::config(ctx, sub).map(|()| 0),
        },
        Command::Completions(args) => admin::completions(ctx, args).map(|()| 0),
        Command::Mcp(_) => Ok(crate::mcp::serve_stdio_code()),
        Command::Daemon(args) => match args.sub {
            None => Ok(help_to_stdout(&["daemon"])),
            Some(sub) => admin::daemon(ctx, sub).map(|()| 0),
        },
    }
}

fn is_dep_verb(sub: &TaskSub) -> bool {
    matches!(
        sub,
        TaskSub::Block { .. } | TaskSub::Unblock { .. } | TaskSub::Next { .. }
    )
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
    use clap::CommandFactory;

    #[test]
    fn the_clap_tree_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn root_help_order_is_fixed_not_alphabetical() {
        let command = Cli::command();
        let names: Vec<&str> = command
            .get_subcommands()
            .filter(|c| !c.is_hide_set())
            .map(clap::Command::get_name)
            .collect();
        assert_eq!(
            names,
            [
                "note",
                "task",
                "search",
                "memory",
                "scratch",
                "asset",
                "init",
                "status",
                "reindex",
                "recent-activity",
                "build-context",
                "graph",
                "project",
                "session-start",
                "watch",
                "config",
                "completions",
                "mcp"
            ]
        );
    }

    #[test]
    fn daemon_is_registered_but_hidden() {
        let command = Cli::command();
        let daemon = command.find_subcommand("daemon").unwrap();
        assert!(daemon.is_hide_set());
    }

    fn sub_names(command: &clap::Command, name: &str) -> Vec<String> {
        command
            .find_subcommand(name)
            .unwrap()
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect()
    }

    #[test]
    fn subcommand_registration_order_is_preserved() {
        let command = Cli::command();
        assert_eq!(
            sub_names(&command, "note"),
            ["new", "append", "update", "get", "list", "delete"]
        );
        assert_eq!(
            sub_names(&command, "task"),
            [
                "new", "update", "append", "claim", "release", "finish", "cancel", "get", "list",
                "delete", "block", "unblock", "next"
            ]
        );
        assert_eq!(
            sub_names(&command, "memory"),
            ["new", "append", "update", "get", "list", "recall", "forget"]
        );
        assert_eq!(
            sub_names(&command, "scratch"),
            ["set", "append", "get", "list", "clear"]
        );
        assert_eq!(
            sub_names(&command, "asset"),
            ["add", "get", "path", "list", "attach", "detach", "remove", "gc"]
        );
        assert_eq!(
            sub_names(&command, "config"),
            ["path", "show", "get", "set"]
        );
        assert_eq!(sub_names(&command, "daemon"), ["start", "stop", "status"]);
    }

    #[test]
    fn admin_commands_declare_no_local_output_flags() {
        let command = Cli::command();
        for name in ["init", "status", "reindex", "completions", "mcp"] {
            let sub = command.find_subcommand(name).unwrap();
            let has = sub.get_arguments().any(|a| a.get_id() == "json");
            assert!(!has, "{name} must not declare a local --json");
        }
    }

    #[test]
    fn non_admin_commands_declare_local_output_flags() {
        let command = Cli::command();
        let note_new = command
            .find_subcommand("note")
            .unwrap()
            .find_subcommand("new")
            .unwrap();
        assert!(note_new.get_arguments().any(|a| a.get_id() == "json"));
        assert!(note_new.get_arguments().any(|a| a.get_id() == "quiet"));
        let search = command.find_subcommand("search").unwrap();
        assert!(search.get_arguments().any(|a| a.get_id() == "json"));
    }

    #[test]
    fn flags_parse_on_either_side_of_the_command_name() {
        let left = Cli::try_parse_from(["mesh", "--json", "note", "list"]).unwrap();
        assert!(left.json);
        let right = Cli::try_parse_from(["mesh", "note", "list", "--json"]).unwrap();
        assert!(!right.json);
        match right.command {
            Some(Command::Note(args)) => match args.sub {
                Some(NoteSub::List { out, .. }) => assert!(out.json),
                other => panic!("wrong subcommand: {other:?}"),
            },
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn defaults_match_the_surface_contract() {
        let parsed = Cli::try_parse_from(["mesh", "note", "list"]).unwrap();
        match parsed.command {
            Some(Command::Note(args)) => match args.sub {
                Some(NoteSub::List { limit, sort, .. }) => {
                    assert_eq!(limit, 20);
                    assert_eq!(sort, "updated");
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        let parsed = Cli::try_parse_from(["mesh", "search", "q"]).unwrap();
        match parsed.command {
            Some(Command::Search(args)) => {
                assert_eq!(args.limit, 10);
                assert_eq!(args.engine, "auto");
                assert_eq!(args.query.as_deref(), Some("q"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_sub_app_with_no_subcommand_parses_to_none() {
        let parsed = Cli::try_parse_from(["mesh", "note"]).unwrap();
        match parsed.command {
            Some(Command::Note(args)) => assert!(args.sub.is_none()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn repeatable_search_tags_are_anded() {
        let parsed = Cli::try_parse_from(["mesh", "search", "--tags", "a", "--tags", "b"]).unwrap();
        match parsed.command {
            Some(Command::Search(args)) => assert_eq!(args.tags, ["a", "b"]),
            other => panic!("{other:?}"),
        }
    }
}
