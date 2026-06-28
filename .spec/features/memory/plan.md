---
type: feature-plan
feature: memory
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-21
---

# Memory — Plan

**Gate:** search DONE. **DONE:** MCP mirrors CLI; `session-start` includes stale claimed tasks.

## Requirements Trace

| ID | Units |
|---|---|
| R1 | 1 |
| R2 | 2 |
| R3 | 3 |
| R4 | 4 |

---

### memory/1 — MCP + annotations
Tool table, withhold delete/admin. → `test_tools.py`

### memory/2 — recent-activity
CLI + tool; `--mine`; daemon-down scan. → `test_recent_activity.py`

### memory/3 — build-context
BFS wikilinks; dedupe cycles. → `test_build_context.py`

### memory/4 — session-start + hook
Two-source compose; hook config. → `test_session_hook.py`

## Progress

| Unit | Status |
|---|---|
| 1–4 | NOT STARTED |
