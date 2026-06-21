---
type: feature-plan
feature: daemon
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-21
---

# Daemon — Plan

**Gate:** tasks DONE. **DONE:** warm RPC + fallback; watcher updates index; `status` reports health.

## Requirements Trace

| ID | Units |
|---|---|
| R1 Warm | 1 |
| R2 Fresh | 2 |
| R3 Degrade | 1, 3 |
| R4 Admin | 3 |

---

### daemon/1 — Socket + fallback
NDJSON server/client; `0600` socket. → `test_socket_fallback.py`

### daemon/2 — Watcher + hook
Warm index reconcile; `on_vault_change`. → `test_watch.py`

### daemon/3 — Admin
`daemon *`, `status`, `reindex` delegate. → `test_admin_status.py`

## Progress

| Unit | Status |
|---|---|
| 1–3 | NOT STARTED |
