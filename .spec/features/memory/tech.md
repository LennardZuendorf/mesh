---
type: feature-tech
feature: memory
sibling: product.md
parent: ../../tech.md
updated: 2026-06-10
---

# Feature: Memory — Architecture

The memory server is a FastMCP app and the same daemon client as the CLI: identical logic, identical atomic primitives, JSON always. Write tools map one-to-one to a CLI command; two read tools (`recent_activity`, `build_context`) are memory-specific shapes over the warm index; hard-delete and infrastructure commands are omitted so a model cannot tear down the daemon or irrecoverably destroy work.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/brain/mcp/server.py        # FastMCP server exposing the brain_* tools + annotations
src/brain/core/activity.py     # recent_activity: time-ordered changed notes/tasks from the index
src/brain/core/context.py      # build_context: bounded wikilink-neighborhood assembly
hooks/session_start.json       # Claude Code SessionStart warm-start hook (config artifact)
```

---

## Contract / API

Tool → CLI mapping, one row per tool so the mapping is verifiable. Each tool carries a behaviour annotation so an agent self-selects safe tools:

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
| `brain_task_cancel` | `task cancel` | destructive |
| `brain_task_update` | `task update` | idempotent |
| `brain_search` | `search` | read-only |
| `brain_recent_activity` | `recent-activity` | read-only |
| `brain_build_context` | `build-context` | read-only |

**Withheld from MCP:** `note delete`, `task delete`, and the admin/local-only commands `daemon start|stop|status`, `reindex`, `status`. Rationale: hard-delete (irrecoverable file removal) and infrastructure ops stay off the agent surface; humans run those from a shell. `task cancel` is exposed but annotated `destructive` — reversible coordination (moves the file to `done/`), not data loss. Task graph tools (`brain_task_release`, readiness) land with the tasks dependency-graph phase, not in v1.

## Implementation Detail

- **Same client, second transport.** `mcp/server.py` calls the same `daemon/client.py` paths as the CLI, so behaviour, atomicity, and fallback are identical.
- **`recent_activity`.** Reads the daemon's warm frontmatter index, filters by `updated >= now - since`, sorts newest-first, and returns `id/type/title/updated/path` (+ `status` for tasks). With the daemon down it falls back to a directory scan of `notes/` and `tasks/`. `--mine` restricts tasks to the caller's `owner`/`claimed_by`.
- **`build_context`.** Starts from a seed ID, reads its `related` array, and expands breadth-first to `--depth` (default 1), deduplicating; returns each node's frontmatter (+ preview) as compact JSON. Pure read over `core/wikilinks` resolution; daemon-independent.
- **SessionStart hook.** A Claude Code `SessionStart` hook (not `PreToolUse`, which fires before every tool call) runs once; its stdout is injected as context. It calls `recent-activity --mine --meta-only --json` so the agent resumes with the fleet's recent activity and its own open/claimed work.

## Open Questions

None. *Resolved (2026-06-10):* tool annotations, exposed `task cancel`, withheld hard-delete/admin (root [product.md](../../product.md) OQ #2); `recent_activity`/`build_context` are read shapes over the existing index with a no-daemon fallback, not a new store.
