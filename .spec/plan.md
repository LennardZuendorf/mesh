---
type: entrypoint
scope: implementation
covers: feature sequence, build order, validation criteria
updated: 2026-07-05
---

# Shards — Plan

**Status:** Phase 1–2 **delivered** — all five features implemented and tested (578 tests, mypy strict, ruff clean) on branch `feat/phase-1-mvp`. Built bottom-up behind binary whole-feature gates.

**Focus:** Phase 3 (tasks-graph) — `ready` / `release`, strict `--strict` gate, unblock-cascade. **Deferred, not started.**

---

## Sequence

| # | Feature | Status | Tests | Commit |
|---:|---|---|---|---|
| 1 | notes | ✅ DONE | `tests/notes/` | `feat(notes)` |
| 2 | tasks | ✅ DONE | `tests/tasks/` | `feat(tasks)` |
| 3 | daemon | ✅ DONE | `tests/daemon/` | `feat(daemon)` |
| 4 | search | ✅ DONE | `tests/search/` | `feat(search)` |
| 5 | memory | ✅ DONE | `tests/memory/` | `feat(memory)` |
| 6 | tasks-graph | ⏳ deferred (Phase 3) | `tests/tasks/` | — |
| 7 | [shards-rebrand](features/shards-rebrand/plan.md) | 🛠 in progress | full suite | `chore(rebrand)` |

---

## Ownership

| Feature | Owns |
|---|---|
| **notes** | schema, writes, wikilinks, config, global CLI |
| **tasks** | lifecycle, `O_EXCL` claim (v1) |
| **daemon** | socket, watcher, warm index, admin, `on_vault_change` hook |
| **search** | `indexed_client`, tag-pull, fallback, `shards search` |
| **memory** | MCP, `recent-activity`, `build-context`, `session-start` |

**Freshness:** daemon watcher fires hook → search `indexed_client.incremental_update`. `shards reindex` → `full_rebuild()`.

Cross-cutting contracts: [tech.md](tech.md) § Implemented surfaces. Per-feature unit plans were compounded here and removed — unit-level truth is now `tests/` + `src/shards/`.
