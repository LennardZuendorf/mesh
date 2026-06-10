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

Tool → CLI mapping (exposed), one row per tool so the 1:1 mapping is verifiable:

| MCP tool | CLI equivalent |
|---|---|
| `brain_note_new` | `note new` |
| `brain_note_get` | `note get` |
| `brain_note_list` | `note list` |
| `brain_note_update` | `note update` |
| `brain_note_append` | `note append` |
| `brain_task_new` | `task new` |
| `brain_task_get` | `task get` |
| `brain_task_list` | `task list` |
| `brain_task_claim` | `task claim` |
| `brain_task_finish` | `task finish` |
| `brain_task_update` | `task update` |
| `brain_search` | `search` |

**Withheld from MCP:** `task cancel`, `task delete`, `note delete`, and the admin/local-only commands `daemon start|stop|status`, `reindex`, `status`. Rationale: destructive ops and infrastructure ops are kept off the agent surface; humans run those from a shell. `finish`/`claim` are safe, reversible coordination moves and stay exposed.

**Note on the inherited asymmetry:** the original design exposed `brain_note_delete` while withholding `task delete`/`cancel`. The recommended resolution (reflected above) is to withhold `note delete` too, so **no** destructive op is agent-callable — see Open Questions.

## Implementation Detail

- **Same client, second transport.** `mcp/server.py` calls the same `daemon/client.py` paths as the CLI, so behaviour, atomicity, and fallback are identical.
- **SessionStart hook.** A Claude Code `SessionStart` hook (not `PreToolUse`) runs once at session start; its stdout is injected as context. `SessionStart` is the correct event because `PreToolUse` fires before *every* tool call (dozens of times per session, not once), and the earlier draft's `$CONTEXT_SUMMARY` is not a real hook variable. The command uses `--meta-only --json` for a compact, structured injection and `--limit 5` for only the most relevant recent items. The query seeds from the repo name; teams can substitute any session-relevant string.

## Open Questions

1. **Destructive-op asymmetry.** The original design exposed `brain_note_delete` while withholding `task delete`/`cancel`. Should `note delete` also be withheld for consistency, so no destructive op is agent-callable? *Recommended default:* withhold `note delete` too (reflected in the table above); revisit only if agents genuinely need to delete notes.
