---
type: feature-plan
feature: search
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-21
---

# Search — Plan

Completes Phase 1 MVP. **Gate:** daemon DONE. **DONE:** hybrid + fallback + tag-pull; hook registered.

## Requirements Trace

| ID | Units |
|---|---|
| R1 | 2, 3 |
| R2 | 1 |
| R3 | 1, 2 |
| R4 | 1, 2 |

---

### search/1 — Tag-pull + fallback
Schema, substring scoring, stderr notice, `--threshold`. → `test_tagpull_fallback.py`

### search/2 — indexed wrapper
Map hits to Brain JSON; mock `indexed`; foreign files `id: null`. → `test_indexed_client.py`

### search/3 — Freshness
`incremental_update` + `full_rebuild`; register on watcher hook. → `test_freshness.py`

## Progress

| Unit | Status |
|---|---|
| 1–3 | NOT STARTED |
