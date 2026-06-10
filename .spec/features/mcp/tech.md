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

Tool → CLI mapping (exposed):

| MCP tool | CLI equivalent |
|---|---|
| `brain_note_new` / `brain_note_get` / `brain_note_list` / `brain_note_update` / `brain_note_append` | `note new` / `get` / `list` / `update` / `append` |
| `brain_task_new` / `brain_task_get` / `brain_task_list` / `brain_task_claim` / `brain_task_finish` / `brain_task_update` | `task new` / `get` / `list` / `claim` / `finish` / `update` |
| `brain_search` | `search` |

**Withheld from MCP:** `task cancel`, `task delete`, and the admin/local-only commands `daemon start|stop|status`, `reindex`, `status`. Rationale: destructive task ops and infrastructure ops are kept off the agent surface; humans run those from a shell. `finish`/`claim` are safe, reversible coordination moves and stay exposed.

## Implementation Detail

- **Same client, second transport.** `mcp/server.py` calls the same `daemon/client.py` paths as the CLI, so behaviour, atomicity, and fallback are identical.
- **SessionStart hook.** A Claude Code `SessionStart` hook (not `PreToolUse`) runs once at session start; its stdout is injected as context. The command uses `--meta-only --json` for a compact, structured injection and `--limit 5` for only the most relevant recent items. The query seeds from the repo name; teams can substitute any session-relevant string.

## Open Questions

1. **Destructive-op asymmetry.** Whether to expose `brain_note_delete` at all. *Recommended default:* withhold all destructive ops from MCP for consistency; revisit if agents genuinely need to delete notes.
