---
type: feature-plan
feature: notes
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-21
---

# Notes — Plan

First feature — shared primitives + `brain note`, no daemon required.

**Gate:** feature 1. **DONE:** all units pass; note round-trips over vault.

## Requirements Trace

| ID | Units |
|---|---|
| R1 Create | 1, 2 |
| R2 Amend | 3 |
| R3 Read/list | 4 |
| R4 Wikilinks | 5 |
| R5 Delete | 6 |

**Order:** `1 → 2 → {3,4,6}` → `5`

---

### notes/1 — Scaffold + storage
Config (`schemas/config.py`, `BRAIN_CONFIG_PATH`), global CLI flags, `Note` schema, ids, atomic files, sandbox, locks, `tests/conftest.py`.  
**Verify:** `pytest tests/notes/test_schema.py tests/notes/test_storage.py`

### notes/2 — `note new`
CLI create; headless body rules; `$BRAIN_AGENT` default owner.  
**Verify:** `pytest tests/notes/test_new.py`

### notes/3 — append / update
Section append, tag mutation, locked concurrent edits.  
**Verify:** `pytest tests/notes/test_append_update.py`

### notes/4 — get / list
Preview modes, slug resolve, filters/sort.  
**Verify:** `pytest tests/notes/test_get_list.py`

### notes/5 — wikilinks
`related` resolution, dangling detection.  
**Verify:** `pytest tests/notes/test_wikilinks.py`

### notes/6 — delete
Guarded hard delete.  
**Verify:** `pytest tests/notes/test_delete.py`

## Progress

| Unit | Status |
|---|---|
| 1–6 | NOT STARTED |
