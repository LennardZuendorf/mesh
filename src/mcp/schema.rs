//! The tool table: 37 tools, their descriptions, their JSON Schemas and their annotations.
//!
//! One `ToolDef` per registered tool, in `TOOL_NAMES` order. Every parameter carries a
//! non-empty description; every enum is generated from the domain's own vocabulary constants
//! rather than hand-typed, so a new note type or memory kind can never drift out of the
//! schema. Annotations are explicit (overrides.md O4): all three hints are always present and
//! `destructiveHint` is true for `mesh_task_cancel` alone.

use serde_json::{Map, Value as Json};

use crate::domain::TAG_SPEC_SEMANTICS;
use crate::model::memory::{MEMORY_KINDS, MEMORY_SCOPES};
use crate::model::note::NOTE_TYPES;
use crate::model::task::{TASK_PRIORITIES, TASK_STATUSES};

/// The `note_type` / `new_type` vocabulary.
const NOTE_TYPE_VALUES: &[&str] = &NOTE_TYPES;
/// The `priority` vocabulary.
const PRIORITY_VALUES: &[&str] = &TASK_PRIORITIES;
/// The `status` vocabulary `mesh_search` narrows to (`mesh_task_list.status` stays free text).
const STATUS_VALUES: &[&str] = &TASK_STATUSES;
/// The memory `kind` vocabulary.
const KIND_VALUES: &[&str] = &MEMORY_KINDS;
/// The memory `scope` vocabulary.
const SCOPE_VALUES: &[&str] = &MEMORY_SCOPES;
/// The `graph` traversal directions.
const DIRECTION_VALUES: &[&str] = &["out", "in", "both"];
/// The `--engine` vocabulary.
const ENGINE_VALUES: &[&str] = &["auto", "indexed", "builtin", "substring"];

/// How one parameter is typed, defaulted and rendered into JSON Schema.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// A required string.
    Str,
    /// A required array of strings.
    ReqList,
    /// Optional string, `null` by default.
    OptStr,
    /// String with a non-null default.
    StrDefault(&'static str),
    /// Optional enum string, `null` by default.
    OptEnum(&'static [&'static str]),
    /// Enum string with a non-null default.
    EnumDefault(&'static [&'static str], &'static str),
    /// Boolean, `false` by default.
    Bool,
    /// Integer with a non-null default.
    IntDefault(i64),
    /// Optional integer, `null` by default.
    OptInt,
    /// Optional number, `null` by default.
    OptNum,
    /// Optional array of strings, `null` by default.
    OptList,
}

impl Kind {
    /// Whether a parameter of this kind belongs in the schema's `required` list.
    pub fn is_required(self) -> bool {
        matches!(self, Kind::Str | Kind::ReqList)
    }

    fn json(self, description: &str) -> Json {
        let mut out = Map::new();
        let strings = |values: &[&str]| -> Json {
            Json::Array(
                values
                    .iter()
                    .map(|v| Json::String((*v).to_string()))
                    .collect(),
            )
        };
        match self {
            Kind::Str => {
                out.insert("type".into(), Json::String("string".into()));
            }
            Kind::ReqList => {
                out.insert("items".into(), serde_json::json!({"type": "string"}));
                out.insert("type".into(), Json::String("array".into()));
            }
            Kind::OptStr => {
                out.insert("type".into(), serde_json::json!(["string", "null"]));
            }
            Kind::StrDefault(value) => {
                out.insert("type".into(), Json::String("string".into()));
                out.insert("default".into(), Json::String(value.to_string()));
            }
            Kind::OptEnum(values) => {
                out.insert("enum".into(), strings(values));
                out.insert("type".into(), serde_json::json!(["string", "null"]));
            }
            Kind::EnumDefault(values, value) => {
                // The enum constrains the value set; a redundant `type` only costs bytes on a
                // payload every session pays for.
                out.insert("enum".into(), strings(values));
                out.insert("default".into(), Json::String(value.to_string()));
            }
            Kind::Bool => {
                // Absence is the default; a boolean parameter is false unless it is sent.
                out.insert("type".into(), Json::String("boolean".into()));
            }
            Kind::IntDefault(value) => {
                out.insert("type".into(), Json::String("integer".into()));
                out.insert("default".into(), Json::from(value));
            }
            Kind::OptInt => {
                out.insert("type".into(), serde_json::json!(["integer", "null"]));
            }
            Kind::OptNum => {
                out.insert("type".into(), serde_json::json!(["number", "null"]));
            }
            Kind::OptList => {
                out.insert("items".into(), serde_json::json!({"type": "string"}));
                out.insert("type".into(), serde_json::json!(["array", "null"]));
            }
        }
        out.insert("description".into(), Json::String(description.to_string()));
        Json::Object(out)
    }
}

/// One tool parameter.
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub kind: Kind,
    pub description: &'static str,
}

/// The behavioural hints a tool advertises. Exactly one is true per tool (final.md §10);
/// the other two are explicitly false (overrides.md O4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ann {
    /// Reads only; writes nothing.
    ReadOnly,
    /// A write whose repetition is a no-op.
    Idempotent,
    /// A write that removes work from the queue.
    Destructive,
    /// A write with none of the three hints set.
    Write,
}

impl Ann {
    fn json(self) -> Json {
        serde_json::json!({
            "readOnlyHint": self == Ann::ReadOnly,
            "idempotentHint": self == Ann::Idempotent,
            "destructiveHint": self == Ann::Destructive,
        })
    }
}

/// One registered tool.
#[derive(Clone, Copy, Debug)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [Param],
    pub ann: Ann,
}

impl ToolDef {
    /// The `tools/list` entry: name, description, `inputSchema`, `annotations`.
    pub fn json(&self) -> Json {
        let mut properties = Map::new();
        let mut required: Vec<Json> = Vec::new();
        for param in self.params {
            properties.insert(param.name.to_string(), param.kind.json(param.description));
            if param.kind.is_required() {
                required.push(Json::String(param.name.to_string()));
            }
        }
        let mut schema = Map::new();
        schema.insert("type".into(), Json::String("object".into()));
        schema.insert("properties".into(), Json::Object(properties));
        if !required.is_empty() {
            schema.insert("required".into(), Json::Array(required));
        }
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": Json::Object(schema),
            "annotations": self.ann.json(),
        })
    }
}

const fn p(name: &'static str, kind: Kind, description: &'static str) -> Param {
    Param {
        name,
        kind,
        description,
    }
}

// ---------------------------------------------------------------------------------------
// Shared parameter descriptions — one wording, every tool that takes the parameter.
// ---------------------------------------------------------------------------------------

const D_TAGS_AND: &str = "Keep only rows carrying every one of these tags (AND).";
const D_ANY_TAG: &str = "Match any tag in tags instead of requiring all of them.";
const D_OWNER_FILTER: &str =
    "Exact match on owner — trusted local input, not a verified caller identity.";
const D_OWNER_WRITE: &str = "Defaults to the configured agent. An explicit value must be in [tasks].collections when that roster is non-empty — a value check, not a verified identity.";
const D_SINCE: &str = "Recency floor on updated: duration shorthand ('7d', '12h', '2w') or an ISO-8601 date/datetime; omit for no floor.";
const D_LIMIT: &str = "Maximum rows returned.";
const D_TEXT: &str = "Text appended verbatim; stored, never interpreted.";
const D_SECTION: &str = "Append under this '## {section}' heading, creating it at the end of the body if absent. Omit to append at the very end of the body instead.";
const D_BODY: &str = "Initial Markdown body; [[wikilinks]] inside populate related.";
const D_MINE: &str = "Only rows owned by (or claimed by) the configured agent.";
const D_META_ONLY: &str = "Drop the snippet entirely (id/type/title/score/path only).";
const D_FULL: &str = "Return the whole body as the snippet instead of a short excerpt; ignored when meta_only is set.";
const D_PRIORITY: &str = "Sort weight; unset ranks last under sort='priority'.";
const D_MIN_IMPORTANCE: &str = "Keep only memories at or above this importance (1..5).";
const D_KIND_FILTER: &str = "Exact match on a memory's kind.";
const D_SECTION_SHORT: &str =
    "Append under this '## {section}' heading, creating it at the end when absent.";
const D_OWNER_WRITE_SHORT: &str = "Defaults to the configured agent; an explicit value must be in [tasks].collections when that roster is set.";

// ---------------------------------------------------------------------------------------
// The 37 tools, in registration order.
// ---------------------------------------------------------------------------------------

/// The registered tool table, in `TOOL_NAMES` order.
pub const TOOLS: [ToolDef; 37] = [
    ToolDef {
        name: "mesh_note_get",
        description: "Read one note by id or title slug: frontmatter, body, and path.",
        ann: Ann::ReadOnly,
        params: &[p(
            "id",
            Kind::Str,
            "Note id (n-...) or a title slug (case/whitespace-normalized match). A slug matching more than one note raises an error naming the candidates.",
        )],
    },
    ToolDef {
        name: "mesh_note_list",
        description: "List mesh notes with tag/owner/type/recency filters, sorted and capped.",
        ann: Ann::ReadOnly,
        params: &[
            p("tags", Kind::OptList, "Keep only notes carrying every one of these tags (AND)."),
            p("any_tag", Kind::Bool, D_ANY_TAG),
            p("owner", Kind::OptStr, "Exact match on the note's owner field — trusted local input, not a verified caller identity."),
            p("note_type", Kind::OptEnum(NOTE_TYPE_VALUES), "Restrict to one note type; omit to list every type."),
            p("since", Kind::OptStr, "Recency floor on updated: duration shorthand ('7d', '12h', '2w') or an ISO-8601 date/datetime; omit for no floor."),
            p("sort", Kind::StrDefault("updated"), "'updated'/'created' (newest first) or 'title' (A-Z); an unrecognised value is rejected."),
            p("limit", Kind::IntDefault(20), D_LIMIT),
        ],
    },
    ToolDef {
        name: "mesh_task_get",
        description: "Read one task by id: frontmatter, body, path, and derived readiness.",
        ann: Ann::ReadOnly,
        params: &[p(
            "id",
            Kind::Str,
            "Task id (t-...) — id-only; unlike notes, no title-slug match.",
        )],
    },
    ToolDef {
        name: "mesh_task_list",
        description: "List mesh tasks (open and done) with status/owner/mine/project filters, sorted.",
        ann: Ann::ReadOnly,
        params: &[
            p("status", Kind::OptStr, "Comma-separated status filter, e.g. 'open,claimed' (union match). Each token must be one of open/claimed/done/cancelled."),
            p("owner", Kind::OptStr, "Exact match on the task's owner field (who it is accountable to) — trusted local input, not a verified caller identity."),
            p("mine", Kind::Bool, "Restrict to tasks where owner or claimed_by equals the configured agent."),
            p("tags", Kind::OptList, "Keep only tasks carrying every one of these tags (AND)."),
            p("any_tag", Kind::Bool, D_ANY_TAG),
            p("project", Kind::OptStr, "Exact match on the task's project soft link — an unvalidated note id, never checked for existence."),
            p("since", Kind::OptStr, "Recency floor on updated: duration shorthand ('7d', '12h', '2w') or an ISO-8601 date/datetime; keeps updated >= since."),
            p("stale", Kind::OptStr, "Recency ceiling on updated — the inverse of since: keeps updated < stale. Conjunctive with since when both are given."),
            p("available", Kind::Bool, "Takeable work only: status == 'open' and claimed_by is unset."),
            p("ready", Kind::Bool, "Takeable work whose blockers are all satisfied; sorts by priority like available."),
            p("blocked", Kind::Bool, "Open or claimed tasks with an unsatisfied blocker."),
            p("sort", Kind::OptStr, "'updated'/'created' (newest first), 'title' (A-Z), or 'priority' (high, then normal, then low, then unprioritized). Omit to default to 'priority' under available=True and 'updated' otherwise."),
            p("limit", Kind::IntDefault(20), D_LIMIT),
        ],
    },
    ToolDef {
        name: "mesh_search",
        description: "Recall across notes + tasks: tag pull (no query) or scored match (query).",
        ann: Ann::ReadOnly,
        params: &[
            p("query", Kind::OptStr, "Search text, scored and ranked. Omit for a tag-only pull (unscored, meta_only by nature) instead of a search."),
            p("type_filter", Kind::OptStr, "Exact match on frontmatter type: a note type (note/log/decision/reference/project) or 'task'."),
            p("tags", Kind::OptList, D_TAGS_AND),
            p("owner", Kind::OptStr, D_OWNER_FILTER),
            p("status", Kind::OptEnum(STATUS_VALUES), "Exact task-status filter. Notes carry no status field, so this excludes every note hit whenever it is set."),
            p("kind", Kind::OptEnum(KIND_VALUES), "Exact memory-kind filter; excludes non-memory hits when set."),
            p("spaces", Kind::OptList, "Spaces to search: notes, tasks, memories, scratch, assets. Omit for the configured default."),
            p("engine", Kind::OptEnum(ENGINE_VALUES), "Force a path: indexed recall, builtin ranking, or substring tiers. Omit for auto."),
            p("limit", Kind::IntDefault(10), "Maximum hits returned."),
            p("threshold", Kind::OptNum, "Minimum score (0-1) a hit must clear to be kept. Unset defers to [search].threshold, or the substring fallback's own floor when neither is set."),
            p("meta_only", Kind::Bool, D_META_ONLY),
            p("full", Kind::Bool, D_FULL),
        ],
    },
    ToolDef {
        name: "mesh_health",
        description: "Report which path a search would take — indexed recall or the substring fallback — and the gate that decided it.",
        ann: Ann::ReadOnly,
        params: &[],
    },
    ToolDef {
        name: "mesh_recent_activity",
        description: "Recent vault changes (newest first), each row carrying identity: ``{id, type, title, path, mtime, owner, claimed_by}``.",
        ann: Ann::ReadOnly,
        params: &[
            p("since", Kind::OptStr, "Recency floor: duration shorthand ('7d', '12h', '2w') or an ISO-8601 date/datetime; omit for no floor."),
            p("owner", Kind::OptStr, "Restrict to rows owned by this agent (exact match) — trusted local input, not a verified caller identity."),
            p("mine", Kind::Bool, "Restrict to rows owned by (or, for tasks, claimed by) the configured agent."),
            p("limit", Kind::IntDefault(20), D_LIMIT),
        ],
    },
    ToolDef {
        name: "mesh_build_context",
        description: "Expand the ``related`` graph around a seed id (BFS to depth, seed first).",
        ann: Ann::ReadOnly,
        params: &[
            p("seed_id", Kind::Str, "Seed note id (n-...) or task id (t-...) to expand from; must resolve or the call errors."),
            p("depth", Kind::IntDefault(1), "BFS hops from the seed (0 = seed only, 1 = seed plus its direct related entries). Each extra hop reads every newly discovered node off disk, so cost grows with the graph's branching factor — keep this small."),
        ],
    },
    ToolDef {
        name: "mesh_graph",
        description: "Query what's connected to a seed id: ``{seed, nodes, edges}`` (BFS to depth). 'out' follows related, 'in' finds backlinks, 'both' does each.",
        ann: Ann::ReadOnly,
        params: &[
            p("seed_id", Kind::Str, "Seed note id (n-...) or task id (t-...) to query from; must resolve or the call errors."),
            p("depth", Kind::IntDefault(1), "BFS hops from the seed (0 = seed only). Each extra hop reads every newly discovered node off disk, so cost grows with the graph's branching factor — keep this small."),
            p("direction", Kind::EnumDefault(DIRECTION_VALUES, "out"), "'out' (forward related links) — 'in' (who links to this node — backlinks/notify) and 'both' additionally scan the whole vault once to build the backlink index."),
        ],
    },
    ToolDef {
        name: "mesh_project",
        description: "Show a project note and the tasks scoped to it: ``{project, tasks}``.",
        ann: Ann::ReadOnly,
        params: &[p(
            "project_id",
            Kind::Str,
            "Project note id (n-...) or title slug. Every task whose project field points at it is returned, regardless of status — project is a soft link, never validated against type: project.",
        )],
    },
    ToolDef {
        name: "mesh_session_start",
        description: "Warm-start payload: my open/claimed tasks + mentions of me + recent activity.",
        ann: Ann::ReadOnly,
        params: &[
            p("owner", Kind::OptStr, "Show this agent's warm start instead of the caller's own — substitutes the effective identity for every source (tasks, mentions, activity)."),
            p("team", Kind::Bool, "Widen the recent-activity section to the whole team instead of just the effective agent's rows. The task queue and mentions always stay scoped to the effective agent."),
            p("meta_only", Kind::Bool, "Drop task bodies from the task section. Mentions and activity rows never carry a body regardless."),
            p("no_memories", Kind::Bool, "Leave the memory section out."),
            p("budget", Kind::IntDefault(0), "Character budget: trims bodies, then entries, adding a truncated marker. 0 is unbounded."),
        ],
    },
    ToolDef {
        name: "mesh_note_new",
        description: "Create a note (routed by type) and return its frontmatter plus any warnings.",
        ann: Ann::Write,
        params: &[
            p("title", Kind::Str, "Note title. A slug-normalized duplicate against an existing note returns a non-blocking warning, not an error."),
            p("note_type", Kind::EnumDefault(NOTE_TYPE_VALUES, "note"), "Note kind — also selects the storage folder: notes/ (note) or notes/{logs,decisions,references,projects}/ for the others."),
            p("tags", Kind::OptList, "Initial tag list."),
            p("owner", Kind::OptStr, D_OWNER_WRITE),
            p("body", Kind::StrDefault(""), D_BODY),
        ],
    },
    ToolDef {
        name: "mesh_note_append",
        description: "Append text to a note's body (optionally under a section / timestamped).",
        ann: Ann::Write,
        params: &[
            p("target", Kind::Str, "Note id (n-...) or title slug to append to."),
            p("text", Kind::Str, D_TEXT),
            p("section", Kind::OptStr, D_SECTION),
            p("timestamp", Kind::Bool, "Prefix the appended block with '<iso> — <agent>', naming the agent making this call — not the note's owner."),
        ],
    },
    ToolDef {
        name: "mesh_task_new",
        description: "Create a task in tasks/open/ (status open, unclaimed) and return it plus any warnings.",
        ann: Ann::Write,
        params: &[
            p("title", Kind::Str, "Task title. A slug-normalized duplicate against an existing task returns a non-blocking warning, not an error."),
            p("priority", Kind::OptEnum(PRIORITY_VALUES), D_PRIORITY),
            p("tags", Kind::OptList, "Initial tag list."),
            p("owner", Kind::OptStr, D_OWNER_WRITE),
            p("body", Kind::StrDefault(""), D_BODY),
            p("project", Kind::OptStr, "Optional soft link to a project note's id — a plain string, never validated or checked for existence."),
            p("blocks", Kind::OptList, "Task ids this task blocks; mirrored onto them and cycle-checked."),
            p("blocked_by", Kind::OptList, "Task ids blocking this task; it is not ready until they finish."),
        ],
    },
    ToolDef {
        name: "mesh_task_append",
        description: "Append text to a task's body (no status/folder change; mirrors note append).",
        ann: Ann::Write,
        params: &[
            p("task_id", Kind::Str, "Task id (t-...) — id-only."),
            p("text", Kind::Str, D_TEXT),
            p("section", Kind::OptStr, D_SECTION),
            p("timestamp", Kind::Bool, "Prefix the appended block with '<iso> — <agent>', naming the agent making this call — not the task's owner."),
        ],
    },
    ToolDef {
        name: "mesh_note_update",
        description: "Update a note's fields (tags, type — moving its folder) and bump updated.",
        ann: Ann::Idempotent,
        params: &[
            p("target", Kind::Str, "Note id (n-...) or title slug."),
            p("tags", Kind::OptStr, TAG_SPEC_SEMANTICS),
            p("new_type", Kind::OptEnum(NOTE_TYPE_VALUES), "Moves the file into the matching folder; omit to leave the type unchanged."),
        ],
    },
    ToolDef {
        name: "mesh_task_claim",
        description: "Claim a task for an agent (atomic test-and-set; same-owner reclaim is a no-op).",
        ann: Ann::Idempotent,
        params: &[
            p("task_id", Kind::Str, "Task id (t-...) to claim."),
            p("claimer", Kind::OptStr, "Acting agent identity; defaults to [core].agent. A same-agent reclaim is a no-op; a different agent already holding it raises a conflict; claiming a terminal (done/cancelled) task is also a no-op."),
            p("strict", Kind::Bool, "Refuse the claim while a blocker is unsatisfied."),
        ],
    },
    ToolDef {
        name: "mesh_task_release",
        description: "Release a claim, returning the task to open (atomic compare-and-clear; idempotent).",
        ann: Ann::Idempotent,
        params: &[
            p("task_id", Kind::Str, "Task id (t-...) to release."),
            p("owner", Kind::OptStr, "Acting agent identity; defaults to [core].agent. Releasing your own claim, an already-open task, or a terminal task are all idempotent no-ops. Releasing someone else's live claim raises a conflict — this surface carries no force override."),
        ],
    },
    ToolDef {
        name: "mesh_task_finish",
        description: "Finish a task: append an outcome and move it to tasks/done/ (idempotent).",
        ann: Ann::Idempotent,
        params: &[
            p("task_id", Kind::Str, "Task id (t-...) to finish."),
            p("outcome", Kind::OptStr, "Optional outcome text appended under a new '## Outcome' section before the task moves to tasks/done/. Idempotent: a re-finish never adds a second section."),
        ],
    },
    ToolDef {
        name: "mesh_task_update",
        description: "Update a task's fields (priority, tags, title, project, owner, blocks/blocked_by).",
        ann: Ann::Idempotent,
        params: &[
            p("task_id", Kind::Str, "Task id (t-...) to update."),
            p("priority", Kind::OptEnum(PRIORITY_VALUES), D_PRIORITY),
            p("tags", Kind::OptStr, TAG_SPEC_SEMANTICS),
            p("title", Kind::OptStr, "New title. Only renames the task; the id never changes."),
            p("project", Kind::OptStr, "New soft link to a project note's id — a plain string, never validated or checked for existence."),
            p("owner", Kind::OptStr, "Reassigns accountability; must be in [tasks].collections when that roster is non-empty. Never touches claimed_by — use task_claim/task_release for the execution handle."),
            p("blocks", Kind::OptList, "Task ids this task blocks; replaces the list, retracting dropped mirrors."),
            p("blocked_by", Kind::OptList, "Task ids blocking this task; replaces the list, cycle-checked."),
        ],
    },
    ToolDef {
        name: "mesh_task_cancel",
        description: "Cancel a task: append a reason and move it to tasks/done/ (idempotent).",
        ann: Ann::Destructive,
        params: &[
            p("task_id", Kind::Str, "Task id (t-...) to cancel."),
            p("reason", Kind::OptStr, "Optional reason text appended under a new '## Cancelled' section before the task moves to tasks/done/. Idempotent: a re-cancel never adds a second section."),
        ],
    },
    ToolDef {
        name: "mesh_memory_new",
        description: "Create a memory — an agent's durable belief — and return its frontmatter plus any warnings.",
        ann: Ann::Write,
        params: &[
            p("title", Kind::Str, "Memory title. A slug-normalized duplicate returns a warning, not an error."),
            p("kind", Kind::EnumDefault(KIND_VALUES, "fact"), "What sort of belief this is; a recall filter."),
            p("scope", Kind::EnumDefault(SCOPE_VALUES, "shared"), "shared is visible to every agent; private only to its owner."),
            p("importance", Kind::OptInt, "1..5, default 3; weights recall ranking."),
            p("source", Kind::OptStr, "Free-text provenance."),
            p("expires", Kind::OptStr, "When it stops being recalled: '7d'/'2w' from now, or ISO-8601. Nothing is auto-deleted."),
            p("supersedes", Kind::OptStr, "Memory id (m-...) this one replaces; the old one drops out of recall."),
            p("tags", Kind::OptList, "Initial tag list."),
            p("owner", Kind::OptStr, D_OWNER_WRITE_SHORT),
            p("body", Kind::StrDefault(""), D_BODY),
        ],
    },
    ToolDef {
        name: "mesh_memory_append",
        description: "Append text to a memory's body (optionally under a section / timestamped).",
        ann: Ann::Write,
        params: &[
            p("target", Kind::Str, "Memory id (m-...) or title slug to append to."),
            p("text", Kind::Str, D_TEXT),
            p("section", Kind::OptStr, D_SECTION_SHORT),
            p("timestamp", Kind::Bool, "Prefix the block with '<iso> — <agent>', naming this caller — not the memory's owner."),
        ],
    },
    ToolDef {
        name: "mesh_memory_update",
        description: "Update a memory's fields (tags, title, kind, scope, importance, source, expires, owner).",
        ann: Ann::Idempotent,
        params: &[
            p("target", Kind::Str, "Memory id (m-...) or title slug."),
            p("tags", Kind::OptStr, TAG_SPEC_SEMANTICS),
            p("title", Kind::OptStr, "New title; the id never changes."),
            p("kind", Kind::OptEnum(KIND_VALUES), "New kind; omit to keep it."),
            p("scope", Kind::OptEnum(SCOPE_VALUES), "New scope; omit to keep it."),
            p("importance", Kind::OptInt, "New importance, 1..5; omit to keep it."),
            p("source", Kind::OptStr, "New provenance; omit to keep it."),
            p("expires", Kind::OptStr, "New expiry ('7d', '2w' or ISO-8601); the literal 'none' clears it."),
            p("owner", Kind::OptStr, "Reassigns the owner; must be in [tasks].collections when that roster is set."),
        ],
    },
    ToolDef {
        name: "mesh_memory_get",
        description: "Read one memory by id or title slug: frontmatter, body, and path.",
        ann: Ann::ReadOnly,
        params: &[p(
            "target",
            Kind::Str,
            "Memory id (m-...) or a title slug; an ambiguous slug errors with the candidates.",
        )],
    },
    ToolDef {
        name: "mesh_memory_list",
        description: "List memories with kind/scope/importance/recency filters; expired and superseded are excluded.",
        ann: Ann::ReadOnly,
        params: &[
            p("kind", Kind::OptEnum(KIND_VALUES), D_KIND_FILTER),
            p("scope", Kind::OptEnum(SCOPE_VALUES), "Exact match on scope; another agent's private memories stay hidden."),
            p("tags", Kind::OptList, D_TAGS_AND),
            p("any_tag", Kind::Bool, D_ANY_TAG),
            p("owner", Kind::OptStr, D_OWNER_FILTER),
            p("mine", Kind::Bool, D_MINE),
            p("min_importance", Kind::OptInt, D_MIN_IMPORTANCE),
            p("since", Kind::OptStr, D_SINCE),
            p("include_expired", Kind::Bool, "Also return expired memories."),
            p("include_superseded", Kind::Bool, "Also return superseded memories."),
            p("sort", Kind::StrDefault("updated"), "'updated'/'created' (newest first), 'title' (A-Z) or 'importance' (5 first)."),
            p("limit", Kind::IntDefault(20), D_LIMIT),
        ],
    },
    ToolDef {
        name: "mesh_memory_recall",
        description: "Recall memories for a query, ranked by match score, importance and recency decay.",
        ann: Ann::ReadOnly,
        params: &[
            p("query", Kind::Str, "Search text, scored over memories and re-ranked by importance and age."),
            p("kind", Kind::OptEnum(KIND_VALUES), D_KIND_FILTER),
            p("tags", Kind::OptList, D_TAGS_AND),
            p("owner", Kind::OptStr, D_OWNER_FILTER),
            p("mine", Kind::Bool, D_MINE),
            p("min_importance", Kind::OptInt, D_MIN_IMPORTANCE),
            p("limit", Kind::IntDefault(10), "Maximum hits returned."),
            p("threshold", Kind::OptNum, "Minimum final score (0-1), applied after importance and decay weighting."),
            p("no_decay", Kind::Bool, "Rank on match score and importance alone."),
            p("include_expired", Kind::Bool, "Also recall expired memories."),
            p("meta_only", Kind::Bool, D_META_ONLY),
            p("full", Kind::Bool, "Return the whole body as the snippet; ignored under meta_only."),
        ],
    },
    ToolDef {
        name: "mesh_scratch_set",
        description: "Write a named scratch note, replacing its body (idempotent).",
        ann: Ann::Idempotent,
        params: &[
            p("name", Kind::Str, "Scratch name, slugified; scratch carries no id."),
            p("body", Kind::Str, "The whole new body; it replaces what was there."),
            p("agent", Kind::OptStr, "Whose namespace to write; defaults to the configured agent."),
        ],
    },
    ToolDef {
        name: "mesh_scratch_append",
        description: "Append to a named scratch note (optionally under a section / timestamped).",
        ann: Ann::Write,
        params: &[
            p("name", Kind::Str, "Scratch name; it must already exist."),
            p("text", Kind::Str, D_TEXT),
            p("section", Kind::OptStr, D_SECTION_SHORT),
            p("timestamp", Kind::Bool, "Prefix the block with '<iso> — <agent>', naming this caller."),
            p("agent", Kind::OptStr, "Whose namespace to write; defaults to the configured agent."),
        ],
    },
    ToolDef {
        name: "mesh_scratch_get",
        description: "Read one scratch note by name: frontmatter, body, and path.",
        ann: Ann::ReadOnly,
        params: &[
            p("name", Kind::Str, "Scratch name to read."),
            p("agent", Kind::OptStr, "Whose namespace to read; defaults to the configured agent."),
        ],
    },
    ToolDef {
        name: "mesh_scratch_list",
        description: "List scratch notes for one agent, or all of them with all_agents.",
        ann: Ann::ReadOnly,
        params: &[
            p("agent", Kind::OptStr, "Whose namespace to list; defaults to the configured agent."),
            p("all_agents", Kind::Bool, "List every agent's scratch; wins over agent when both are given."),
            p("since", Kind::OptStr, D_SINCE),
        ],
    },
    ToolDef {
        name: "mesh_asset_get",
        description: "Read one asset sidecar by id: frontmatter, caption, and path.",
        ann: Ann::ReadOnly,
        params: &[p(
            "asset_id",
            Kind::Str,
            "Asset id (a-...) — the content address of the blob.",
        )],
    },
    ToolDef {
        name: "mesh_asset_list",
        description: "List assets with tag/owner/media-type/recency filters, sorted and capped.",
        ann: Ann::ReadOnly,
        params: &[
            p("tags", Kind::OptList, D_TAGS_AND),
            p("any_tag", Kind::Bool, D_ANY_TAG),
            p("owner", Kind::OptStr, D_OWNER_FILTER),
            p("mine", Kind::Bool, D_MINE),
            p("media_type", Kind::OptStr, "Exact match on media_type, e.g. 'image/png'."),
            p("since", Kind::OptStr, D_SINCE),
            p("sort", Kind::StrDefault("updated"), "'updated'/'created' (newest first), 'title' (A-Z) or 'bytes' (largest)."),
            p("limit", Kind::IntDefault(20), D_LIMIT),
        ],
    },
    ToolDef {
        name: "mesh_asset_attach",
        description: "Link an asset into a note, task or memory body, populating related both ways.",
        ann: Ann::Idempotent,
        params: &[
            p("asset_id", Kind::Str, "Asset id (a-...) to attach."),
            p("target", Kind::Str, "Note, task or memory id to attach it to, routed by prefix."),
            p("section", Kind::OptStr, D_SECTION_SHORT),
        ],
    },
    ToolDef {
        name: "mesh_task_block",
        description: "Add blocked_by edges to a task (additive, cycle-checked, mirrored).",
        ann: Ann::Idempotent,
        params: &[
            p("task_id", Kind::Str, "Task id (t-...) that becomes blocked."),
            p("on", Kind::ReqList, "Task ids that block it; a repeat edge is a no-op and a cycle is refused."),
        ],
    },
    ToolDef {
        name: "mesh_task_unblock",
        description: "Remove blocked_by edges from a task, or all of them; idempotent.",
        ann: Ann::Idempotent,
        params: &[
            p("task_id", Kind::Str, "Task id (t-...) to unblock."),
            p("on", Kind::OptList, "Task ids to drop; an absent edge is a no-op. Use all=true for every blocker."),
            p("all", Kind::Bool, "Drop every blocker, mirrored edges included."),
        ],
    },
    ToolDef {
        name: "mesh_task_next",
        description: "Pick the highest-priority ready task, optionally claiming it.",
        ann: Ann::Write,
        params: &[
            p("claim", Kind::Bool, "Claim the picked task, re-selecting up to three times on a lost race."),
            p("strict", Kind::Bool, "Refuse a claim whose blockers are unsatisfied."),
            p("mine", Kind::Bool, "Only tasks owned or claimed by the configured agent."),
            p("project", Kind::OptStr, "Only tasks whose project soft link matches exactly."),
            p("tags", Kind::OptList, D_TAGS_AND),
        ],
    },
];

/// The tool with this name, if it is registered.
pub fn find(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|t| t.name == name)
}

/// The `tools/list` payload: every tool, in registration order.
pub fn tools_list() -> Json {
    Json::Array(TOOLS.iter().map(ToolDef::json).collect())
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
    use crate::mcp::{DESTRUCTIVE_TOOLS, TOOL_NAMES};

    #[test]
    fn the_table_matches_the_registration_order() {
        let names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        assert_eq!(names.as_slice(), TOOL_NAMES.as_slice());
    }

    #[test]
    fn every_parameter_has_a_description() {
        for tool in TOOLS {
            for param in tool.params {
                assert!(
                    !param.description.is_empty(),
                    "{}.{}",
                    tool.name,
                    param.name
                );
                assert!(!param.name.starts_with('-'), "{}", param.name);
            }
        }
    }

    #[test]
    fn every_tool_but_health_takes_a_parameter() {
        for tool in TOOLS {
            if tool.name == "mesh_health" {
                assert!(tool.params.is_empty());
            } else {
                assert!(!tool.params.is_empty(), "{}", tool.name);
            }
        }
    }

    #[test]
    fn every_tool_description_is_non_empty_and_new_ones_are_capped() {
        let legacy = 21;
        for (i, tool) in TOOLS.iter().enumerate() {
            assert!(!tool.description.is_empty(), "{}", tool.name);
            if i >= legacy {
                assert!(
                    tool.description.chars().count() <= 200,
                    "{} description is {} chars",
                    tool.name,
                    tool.description.chars().count()
                );
            }
        }
    }

    #[test]
    fn annotations_are_explicit_and_destructive_is_only_cancel() {
        let mut destructive: Vec<&str> = Vec::new();
        for tool in TOOLS {
            let ann = tool.ann.json();
            for hint in ["readOnlyHint", "idempotentHint", "destructiveHint"] {
                assert!(ann.get(hint).and_then(Json::as_bool).is_some(), "{hint}");
            }
            if ann["destructiveHint"] == Json::Bool(true) {
                destructive.push(tool.name);
            }
        }
        assert_eq!(destructive.as_slice(), DESTRUCTIVE_TOOLS.as_slice());
    }

    #[test]
    fn the_enums_come_from_the_domain_vocabularies() {
        let schema = find("mesh_note_new").unwrap().json();
        let values = &schema["inputSchema"]["properties"]["note_type"]["enum"];
        assert_eq!(values, &serde_json::json!(NOTE_TYPES.to_vec()));
        let schema = find("mesh_task_new").unwrap().json();
        let values = &schema["inputSchema"]["properties"]["priority"]["enum"];
        assert_eq!(values, &serde_json::json!(TASK_PRIORITIES.to_vec()));
        let schema = find("mesh_memory_new").unwrap().json();
        assert_eq!(
            &schema["inputSchema"]["properties"]["kind"]["enum"],
            &serde_json::json!(MEMORY_KINDS.to_vec())
        );
    }

    #[test]
    fn task_list_status_is_a_free_string() {
        let schema = find("mesh_task_list").unwrap().json();
        let status = &schema["inputSchema"]["properties"]["status"];
        assert!(status.get("enum").is_none());
        assert!(serde_json::to_string(status).unwrap().contains("string"));
        assert!(!serde_json::to_string(status).unwrap().contains("enum"));
    }

    #[test]
    fn the_tag_spec_sentence_is_byte_identical_in_three_schemas() {
        for name in ["mesh_note_update", "mesh_task_update", "mesh_memory_update"] {
            let schema = find(name).unwrap().json();
            assert_eq!(
                schema["inputSchema"]["properties"]["tags"]["description"],
                Json::String(TAG_SPEC_SEMANTICS.to_string()),
                "{name}"
            );
        }
    }

    #[test]
    fn release_never_grows_a_force_parameter() {
        let names: Vec<&str> = find("mesh_task_release")
            .unwrap()
            .params
            .iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["task_id", "owner"]);
    }

    #[test]
    fn required_lists_only_the_required_parameters() {
        let schema = find("mesh_note_append").unwrap().json();
        assert_eq!(
            schema["inputSchema"]["required"],
            serde_json::json!(["target", "text"])
        );
        let health = find("mesh_health").unwrap().json();
        assert!(health["inputSchema"].get("required").is_none());
        assert_eq!(health["inputSchema"]["properties"], serde_json::json!({}));
    }

    #[test]
    fn the_serialised_tool_list_fits_the_budget() {
        let text = serde_json::to_string(&tools_list()).unwrap();
        assert!(
            text.len() <= 32 * 1024,
            "tools/list is {} bytes",
            text.len()
        );
        // Print the figure so a description edit that eats the headroom is visible.
        println!("tools/list: {} bytes", text.len());
    }
}
