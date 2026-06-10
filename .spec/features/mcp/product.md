---
type: feature-product
feature: mcp
sibling: tech.md
parent: ../../product.md
updated: 2026-06-10
---

# Feature: MCP — Product

The MCP surface lets agents drive Brain natively, over the same daemon and the same logic as the CLI, always returning JSON. It also provides the Claude Code `SessionStart` hook that injects the most relevant recent notes and tasks so an agent resumes warm instead of cold. This is the Phase-2 agent surface — no new primitives, a second transport.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The FastMCP server and the `brain_*` tool mapping; the decision of which commands are exposed vs withheld; the `SessionStart` context-injection hook |
| **Does not own** | The CLI surface, the daemon internals, or the core domain logic — it is a thin transport over the same primitives |

---

## Requirements

### Requirement: Expose safe commands as MCP tools

The system SHALL expose the note/task/search commands as `brain_*` MCP tools backed by the same daemon, returning JSON; each tool MUST carry a behaviour annotation (`read-only` | `idempotent` | `destructive`) so an agent can self-select safe tools; and the surface MUST withhold hard-delete and infrastructure operations.

#### Scenario: Agent claims a task over MCP

- **Given** an agent connected to the MCP server
- **When** it calls `brain_task_claim`
- **Then** the same atomic claim runs as the CLI path and JSON is returned

#### Scenario: Tools are annotated and hard-deletes are withheld

- **Given** the MCP tool list
- **When** an agent inspects it
- **Then** every tool carries a `read-only`/`idempotent`/`destructive` annotation; `brain_task_cancel` is present (annotated `destructive`, but reversible coordination); and `note delete`, `task delete`, and admin commands (`daemon`, `reindex`, `status`) are absent

### Requirement: Warm-start context injection

The system SHALL provide a Claude Code `SessionStart` hook that runs a token-budgeted search and injects the top relevant note/task snippets once at session start.

#### Scenario: Resume warm

- **Given** the hook is configured
- **When** an agent session begins
- **Then** a compact `--meta-only --json` result of the top relevant items is injected as context

Reference requirements as R1, R2 in the feature plan's Requirements Trace.

## User Experience

```jsonc
// SessionStart hook — runs once at session start, stdout injected as context
{ "hooks": { "SessionStart": [ { "hooks": [ {
  "type": "command",
  "command": "brain search \"$(git rev-parse --show-toplevel | xargs basename)\" --limit 5 --meta-only --json"
} ] } ] } }
```

## Non-Goals

- No MCP-only capability — every tool maps to an existing CLI command.
- No exposure of hard-delete (`note delete`, `task delete`) or infrastructure operations (`daemon`, `reindex`, `status`) to agents. Reversible coordination (`task cancel`) is exposed but annotated `destructive`.
