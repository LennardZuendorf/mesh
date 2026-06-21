---
type: feature-product
feature: memory
sibling: tech.md
parent: ../../product.md
updated: 2026-06-21
---

# Feature: Memory — Product

The memory cluster is Brain's agent-facing surface: the same three verbs, plus signed-off read-only memory-lens commands and a warm-start hook, exposed as MCP tools and framed as *memory* — capture, recall, and "pick up where you left off." It is a **lens on the existing notes + search + daemon**, not a new store and not a flat 1:1 mirror of the CLI. This is the Phase-2 agent surface.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The FastMCP server and the `brain_*` tool mapping; per-tool behaviour annotations (`read-only`/`idempotent`/`write`/`destructive`); the memory-lens read commands `recent-activity` and `build-context` (and their MCP tools); the `SessionStart` warm-start hook |
| **Does not own** | The CLI surface for the three verbs (notes/tasks/search features), the daemon internals, the core domain logic, or the search engine (`indexed`) — it is a thin transport and warm-start lens over the same primitives |

---

## Requirements

### Requirement: Expose the verbs as annotated memory tools

The system SHALL expose the note/task/search commands as `brain_*` MCP tools backed by the same daemon, returning JSON; each tool MUST carry a behaviour annotation (`read-only` | `idempotent` | `write` | `destructive`) so an agent self-selects safe tools; and the surface MUST withhold hard-delete and infrastructure operations.

#### Scenario: Agent claims a task over MCP

- **Given** an agent connected to the MCP server
- **When** it calls `brain_task_claim` with task id `t-c7d1`
- **Then** the same atomic claim runs as the CLI path and JSON is returned

#### Scenario: Tools are annotated and hard-deletes are withheld

- **Given** the MCP tool list
- **When** an agent inspects it
- **Then** every tool carries a `read-only`/`idempotent`/`write`/`destructive` annotation; `brain_task_cancel` is present (annotated `destructive`, reversible coordination); and `note delete`, `task delete`, and admin commands (`daemon`, `reindex`, `status`) are absent

### Requirement: Recent-activity feed

The system SHALL provide a `recent_activity` tool (and matching `recent-activity` CLI command) that returns notes and tasks changed within a time window, time-ordered (newest first) — a "what changed since last session" feed distinct from a relevance query, answered from the daemon's warm index.

#### Scenario: What changed today

- **Given** notes and tasks were edited in the last day
- **When** an agent calls `brain_recent_activity` with `since: "24h"`
- **Then** the changed notes and tasks are returned newest-first with `id`, `type`, `title`, `updated`, and `path`

#### Scenario: Recent activity with daemon down

- **Given** the daemon is stopped
- **When** `brain recent-activity --since 7d` runs
- **Then** results come from a directory scan (slower but complete)

### Requirement: Context assembly over wikilinks

The system SHALL provide a `build_context` tool (and matching `build-context` CLI command) that, given a seed note/task ID, follows its `related` wikilinks to assemble a bounded neighborhood of connected notes/tasks — graph traversal, not flat search — so an agent can pull a relevant cluster on demand.

#### Scenario: Pull the neighborhood of a decision

- **Given** a decision note `n-a3f2` whose `related` links to two notes and the task it came from
- **When** an agent calls `brain_build_context` with seed `n-a3f2` and `depth: 1`
- **Then** the seed plus its directly-linked notes/tasks are returned as compact JSON

### Requirement: Warm-start context injection

The system SHALL provide a Claude Code `SessionStart` hook that injects a compact continuity preamble once at session start, composed from **(a)** recent activity within a window and **(b)** the caller's own open/claimed tasks regardless of recency — so an agent resumes warm instead of cold.

#### Scenario: Resume warm

- **Given** the hook is configured
- **When** an agent session begins
- **Then** a token-budgeted `--meta-only --json` digest includes items from `recent-activity --since 7d --mine` **and** all `task list --mine --status open,claimed` results (deduplicated by `id`)

Reference requirements as R1–R4 in the feature plan's Requirements Trace.

## User Experience

```jsonc
// SessionStart hook — runs once at session start, stdout injected as context
{ "hooks": { "SessionStart": [ { "hooks": [ {
  "type": "command",
  "command": "brain session-start --meta-only --json"
} ] } ] } }
```

`brain session-start` (memory feature) composes recent activity + open/claimed tasks internally — agents configure one command.

## Prior Art & Inspiration

**Anchor — [basic-memory](https://github.com/basicmachines-co/basic-memory):** a memory MCP over a Markdown vault whose source of truth is the files and whose index is derivative — architecturally identical to Brain.

- **Borrow:** the *capability* framing of memory (not the transport); `recent_activity` as a time-ordered feed; `build_context` traversing wiki-links to assemble a neighborhood; per-tool read-only/destructive annotations; warm-start "pick up where you left off."
- **Differ:** Brain stays at three verbs for capture/coordination/recall and adds **no** new store — `recent-activity`/`build-context` are signed-off read-only memory-lens commands over the existing notes + daemon index. Contrast [mcp-memory-keeper](https://github.com/mkreyman/mcp-memory-keeper): 38 tools over a dedicated SQLite context DB — the exact "second memory store" Brain refuses.

## Non-Goals

- No MCP-only write capability — every write tool maps 1:1 to a CLI command.
- No new memory store — `recent_activity` and `build_context` read the same files and warm index everything else uses.
- No exposure of hard-delete (`note delete`, `task delete`) or infrastructure operations to agents. Reversible coordination (`task cancel`) is exposed but annotated `destructive`.
