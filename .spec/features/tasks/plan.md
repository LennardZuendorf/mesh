---
type: feature-plan
feature: tasks
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-21
---

# Tasks — Plan

v1 coordination core. **Gate:** notes DONE. **DONE:** create→claim→finish/cancel; concurrent claim test passes.

## Requirements Trace

| ID | Units |
|---|---|
| R1 | 1, 2 |
| R2 | 3 |
| R3 | 4 |
| R4 | 5 |
| R5 | 5, 6 |

---

### tasks/1 — Schema + routing
`Task` model, `tasks/open|done/` routing. → `test_schema.py`

### tasks/2 — new / update
CLI; inert `--blocked-by`. → `test_new_update.py`

### tasks/3 — claim
Concurrent `O_EXCL`; exit `4`. → `test_claim.py`

### tasks/4 — finish
Idempotent move to `done/`. → `test_finish.py`

### tasks/5 — list / get / cancel
`--mine` filters; cancel idempotent. → `test_list_cancel.py`

### tasks/6 — delete
Guarded removal. → `test_delete.py`

Phase 3 graph → root plan feature 6.

## Progress

| Unit | Status |
|---|---|
| 1–6 | NOT STARTED |
