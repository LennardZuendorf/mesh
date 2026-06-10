---
type: feature-tech
feature: mcp
sibling: product.md
parent: ../../tech.md
updated: 2026-06-10
---

# Feature: MCP — Architecture

The MCP server is a FastMCP app that is the same daemon client as the CLI: identical logic, identical atomic primitives, JSON always. Each tool maps one-to-one to a CLI command; destructive and infrastructure commands are deliberately omitted so a model cannot tear down the daemon or irrecoverably destroy work.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/brain/mcp/server.py      # FastMCP server exposing the brain_* tools
```

---

## Contract / API

Tool → CLI mapping (exposed), one row per tool so the 1:1 mapping is verifiable. Each tool carries a behaviour annotation so an agent can self-select safe tools:

| MCP tool | CLI equivalent | Annotation |
|---|---|---|
| `brain_note_new` | `note new` | write |
| `brain_note_get` | `note get` | read-only |
| `brain_note_list` | `note list` | read-only |
| `brain_note_update` | `note update` | idempotent |
| `brain_note_append` | `note append` | write |
| `brain_task_new` | `task new` | write |
| `brain_task_get` | `task get` | read-only |
| `brain_task_list` | `task list` | read-only |
| `brain_task_claim` | `task claim` | idempotent |
| `brain_task_finish` | `task finish` | idempotent |
| `brain_task_release` | `task release` | idempotent |
| `brain_task_cancel` | `task cancel` | destructive |
| `brain_task_update` | `task update` | idempotent |
| `brain_search` | `search` | read-only |

**Withheld from MCP:** `note delete`, `task delete`, and the admin/local-only commands `daemon start|stop|status`, `reindex`, `status`. Rationale: hard-delete (irrecoverable file removal) and infrastructure ops stay off the agent surface; humans run those from a shell. `task cancel` is exposed but annotated `destructive` — it is reversible coordination (moves the file to `done/`, unblocks dependents), not data loss, and the PM-style `tolaria-agent` needs it to de-scope stale work.

## Implementation Detail

- **Same client, second transport.** `mcp/server.py` calls the same `daemon/client.py` paths as the CLI, so behaviour, atomicity, and fallback are identical.
- **SessionStart hook.** A Claude Code `SessionStart` hook (not `PreToolUse`) runs once at session start; its stdout is injected as context. `SessionStart` is the correct event because `PreToolUse` fires before *every* tool call (dozens of times per session, not once), and the earlier draft's `$CONTEXT_SUMMARY` is not a real hook variable. The command uses `--meta-only --json` for a compact, structured injection and `--limit 5` for only the most relevant recent items. The query seeds from the repo name; teams can substitute any session-relevant string.

## Open Questions

None. *Resolved (2026-06-10):* every tool carries a `read-only`/`idempotent`/`write`/`destructive` annotation; `task cancel` is exposed (annotated `destructive`); hard `note delete`/`task delete` and admin ops stay human-only. See root [product.md](../../product.md) Open Question #2.
