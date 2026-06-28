---
type: entrypoint
scope: implementation
covers: feature sequence, build order, validation criteria
children:
  - features/notes/plan.md
  - features/tasks/plan.md
  - features/daemon/plan.md
  - features/search/plan.md
  - features/memory/plan.md
updated: 2026-06-21
---

# Brain — Plan

**Status:** spec-only — `src/brain/` does not exist. Build bottom-up; binary gates between features.

**Focus:** human sign-off on [notes/plan.md](features/notes/plan.md), then `notes/1`.

---

## Sequence

| # | Feature | Gate | Tests |
|---:|---|---|---|
| 1 | notes | — | `tests/notes/` |
| 2 | tasks | notes DONE | `tests/tasks/` |
| 3 | daemon | tasks DONE | `tests/daemon/` |
| 4 | search | daemon DONE | `tests/search/` |
| 5 | memory | search DONE | `tests/memory/` |
| 6 | tasks-graph | memory DONE | `tests/tasks/` |

---

## Ownership

| Feature | Owns |
|---|---|
| **notes** | schema, writes, wikilinks, config, global CLI |
| **tasks** | lifecycle, `O_EXCL` claim (v1) |
| **daemon** | socket, watcher, warm index, admin, `on_vault_change` hook |
| **search** | `indexed_client`, tag-pull, fallback, `brain search` |
| **memory** | MCP, `recent-activity`, `build-context`, `session-start` |

**Freshness:** daemon watcher fires hook → search `indexed_client.incremental_update`. `brain reindex` → `full_rebuild()`.

Cross-cutting contracts: [tech.md](tech.md). Unit detail: `features/<name>/plan.md`.
