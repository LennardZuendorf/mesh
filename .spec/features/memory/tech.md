---
type: feature-tech
feature: memory
sibling: product.md
parent: ../../tech.md
updated: 2026-06-21
---

# Memory — Tech

FastMCP over same `daemon/client.py` as CLI. MCP params = typed fields, not CLI flag strings.

**Links:** [product](product.md) · [plan](plan.md)

## Files

`mcp/server.py` · `core/activity.py` · `core/context.py` · `cli/session.py` · `hooks/session_start.json`

## Tools

| MCP | CLI | Ann. |
|---|---|---|
| `brain_note_{new,append}` | same | write |
| `brain_note_{get,list}` | same | read-only |
| `brain_note_update` | `note update` | idempotent |
| `brain_task_{new}` | same | write |
| `brain_task_{get,list}` | same | read-only |
| `brain_task_{claim,finish,update}` | same | idempotent |
| `brain_task_cancel` | `task cancel` | destructive |
| `brain_search` | `search` | read-only |
| `brain_recent_activity` | `recent-activity` | read-only |
| `brain_build_context` | `build-context` | read-only |

**Withheld:** delete, `daemon`, `reindex`, `status`. Phase 3: `brain_task_release`.

**session-start:** merge `recent_activity(7d, mine)` + `task_list(mine, open|claimed)`; dedupe by id; tasks first, then by `updated`.
