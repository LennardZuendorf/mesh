---
type: entrypoint
scope: implementation
covers: feature sequence, build order, validation criteria, open decisions
children:
  - features/notes/plan.md
  - features/tasks/plan.md
  - features/daemon/plan.md
  - features/search/plan.md
  - features/memory/plan.md
updated: 2026-06-21
---

# Brain — Implementation Plan

Brain is at **spec stage** — no implementation has landed yet. Delivery is a single linear arc: the five features compose into one tool, built bottom-up so each is a closed, testable box before the next starts. The MVP (Phase 1) is `notes → tasks → daemon → search`; the agent surface (`memory`) follows in Phase 2; the task dependency graph is Phase 3. Build-vs-integrate is settled per cluster: Brain owns writes + the task core, **wraps `indexed`** for search, **coexists with Tolaria** for vault reads, and frames the agent surface as **memory**. Current focus is the first feature, `notes`.

**Parent specs:** [product.md](product.md), [tech.md](tech.md), [design.md](design.md)

**Feature plans (unit-level detail lives here, not duplicated below):**

| Feature | Product | Plan | Status |
|---|---|---|---|
| [notes](features/notes/product.md) | `.spec/` + feature | [plan.md](features/notes/plan.md) | planned |
| [tasks](features/tasks/product.md) | `.spec/` + feature | [plan.md](features/tasks/plan.md) | planned (v1) |
| [daemon](features/daemon/product.md) | `.spec/` + feature | [plan.md](features/daemon/plan.md) | planned |
| [search](features/search/product.md) | `.spec/` + feature | [plan.md](features/search/plan.md) | planned |
| [memory](features/memory/product.md) | `.spec/` + feature | [plan.md](features/memory/plan.md) | planned |

---

## Validation Summary

Nothing is built. All five features are spec-ahead-of-code; `src/brain/` does not exist yet. The spec is the contract the first implementation must satisfy.

## Feature Boundaries

```
notes   ── owns ──>  note schema, writes, direct reads, brain note CLI, wikilink resolution
tasks   ── owns ──>  task schema, v1 lifecycle (claim/finish/cancel/list), O_EXCL locks
daemon  ── owns ──>  socket server, watcher, warm frontmatter index, watcher hook, admin commands (reindex entry point)
search  ── owns ──>  indexed wrapper, tag-pull, substring fallback, indexed_client (incremental/full rebuild)
memory  ── owns ──>  FastMCP brain_* tools + annotations, recent_activity, build_context, SessionStart hook
```

| Layer | Owns | Does not own |
|---|---|---|
| **notes** | `core/notes.py`, `core/wikilinks.py`, `cli/note.py`, note frontmatter, writes + direct reads, config loading, global CLI shell | Task lifecycle, search ranking, daemon, the vault/git (Tolaria) |
| **tasks** | `core/tasks.py`, `storage/locks.py`, `cli/task.py`, v1 lifecycle/concurrency | Note CRUD, search ranking, daemon, the deferred dependency graph |
| **daemon** | `daemon/server.py`, `daemon/client.py`, `index/watch.py`, admin commands, `on_vault_change` hook registration | Ranking engine (`indexed`), `indexed_client` implementation (search), domain logic (core) |
| **search** | `index/indexed_client.py`, `index/tagpull.py`, `index/fallback.py`, `cli/search.py` | The ranking engine itself (`indexed`); watcher (daemon); schemas |
| **memory** | `mcp/server.py`, tool mapping + annotations, `core/activity.py`, `core/context.py`, SessionStart hook | The CLI, daemon internals, core write primitives, `indexed` |

Cross-cutting contracts (frontmatter schema, IDs, atomic writes, path sandbox, exit codes, config) live in root [tech.md](tech.md), not in any one feature.

**Freshness split:** the **daemon** watcher fires `on_vault_change(path)` on file events; the **search** feature's `indexed_client` registers that callback and implements `incremental_update` / `full_rebuild`. `brain reindex` is an admin command owned by the daemon CLI surface but delegates to `indexed_client.full_rebuild()`.

---

## Feature Sequence

Whole-feature delivery order with **binary** gates — a downstream feature starts only when its upstream is `DONE`. Units (`feature/n`) live in feature plans, never here.

| Order | Feature | Deliverable | Test | Status | Starts when |
|---:|---|---|---|---|---|
| 1 | notes | `brain note` surface, note schema, wikilinks, config, global CLI | `tests/notes/` | NOT STARTED | — |
| 2 | tasks | `brain task` v1 — atomic claim/finish, cancel, list (dependency graph deferred) | `tests/tasks/` | NOT STARTED | notes DONE |
| 3 | daemon | socket server, watcher, warm index, fallback shim, admin commands | `tests/daemon/` | NOT STARTED | tasks DONE |
| 4 | search | `indexed` wrapper + tag-pull + substring fallback + `indexed_client`, JSON output | `tests/search/` | NOT STARTED | daemon DONE |
| 5 | memory | `brain_*` MCP tools + annotations + `recent-activity`/`build-context` + SessionStart hook (Phase 2) | `tests/memory/` | NOT STARTED | search DONE |
| 6 | tasks-graph | Dependency graph: `ready`, `release`, strict gate, unblock-cascade (Phase 3) | `tests/tasks/` | NOT STARTED | memory DONE |

This single linear arc **is** the roadmap (small, single-goal repo). Cross-feature order is **only** here; feature plans declare same-feature unit deps only.

## Spec vs Implementation

| Gap | Feature / unit | Notes |
|---|---|---|
| No source code exists | all | `src/brain/` is unwritten; every feature is spec-only |

## Current Focus

The spec has passed review-and-hardening passes (build-vs-integrate, cross-layer reconciliation). Settled: **notes** own writes + config/global CLI + cheap direct reads; **tasks** v1 (claim/finish/cancel/list; graph deferred); **search** wraps **`indexed`** with a pinned contract and explicit degradation matrix; **memory** is Phase 2 with signed-off `recent-activity`/`build-context` adjuncts; **daemon** owns the socket/watcher/admin surface; **search** owns `indexed_client`. The next gate is human sign-off on the **notes** (feature 1) unit plan — see [features/notes/plan.md](features/notes/plan.md). No code lands until those units are approved; no work starts on `tasks` until `notes` is DONE.
