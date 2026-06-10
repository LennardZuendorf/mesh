---
type: entrypoint
scope: implementation
covers: feature sequence, build order, validation criteria, open decisions
children: []
updated: 2026-06-10
---

# Brain — Implementation Plan

Brain is at **spec stage** — no implementation has landed yet. Delivery is a single linear arc: the five features compose into one tool, built bottom-up so each is a closed, testable box before the next starts. The MVP (Phase 1) is `notes → tasks → daemon → search`; the agent surface (`mcp`) follows in Phase 2. Current focus is the first feature, `notes`.

**Parent specs:** [product.md](product.md), [tech.md](tech.md), [design.md](design.md)

**Feature plans (unit-level detail lives here, not duplicated below):**

| Feature | Product | Plan | Status |
|---|---|---|---|
| [notes](features/notes/product.md) | `.spec/` + feature | [plan.md](features/notes/plan.md) | planned |
| [tasks](features/tasks/product.md) | `.spec/` + feature | [plan.md](features/tasks/plan.md) | planned |
| [daemon](features/daemon/product.md) | `.spec/` + feature | [plan.md](features/daemon/plan.md) | planned |
| [search](features/search/product.md) | `.spec/` + feature | [plan.md](features/search/plan.md) | planned |
| [mcp](features/mcp/product.md) | `.spec/` + feature | [plan.md](features/mcp/plan.md) | planned |

---

## Validation Summary

Nothing is built. All five features are spec-ahead-of-code; `src/brain/` does not exist yet. The spec is the contract the first implementation must satisfy.

## Feature Boundaries

```
notes   ── owns ──>  note schema, brain note CLI, wikilink resolution
tasks   ── owns ──>  task schema, lifecycle, claim/finish/blocks, locks
daemon  ── owns ──>  socket server, watcher, incremental index, fallback shim
search  ── owns ──>  BM25, embedder adapter, RRF fusion, tag-pull, brain search
mcp     ── owns ──>  FastMCP brain_* tools, SessionStart hook
```

| Layer | Owns | Does not own |
|---|---|---|
| **notes** | `core/notes.py`, `core/wikilinks.py`, `cli/note.py`, note frontmatter | Task lifecycle, search ranking, daemon |
| **tasks** | `core/tasks.py`, `storage/locks.py`, `cli/task.py`, task lifecycle/concurrency | Note CRUD, search ranking, daemon |
| **daemon** | `daemon/server.py`, `daemon/client.py`, `index/watch.py`, admin commands | Ranking algorithm (search), domain logic (core) |
| **search** | `index/bm25.py`, `index/embedder.py`, `index/fusion.py`, `cli/search.py` | The watcher/index storage (daemon), schemas |
| **mcp** | `mcp/server.py`, tool mapping, SessionStart hook | The CLI, daemon internals, core primitives |

Cross-cutting contracts (frontmatter schema, IDs, atomic writes, path sandbox, exit codes, config) live in root [tech.md](tech.md), not in any one feature.

---

## Feature Sequence

Whole-feature delivery order with **binary** gates — a downstream feature starts only when its upstream is `DONE`. Units (`feature/n`) live in feature plans, never here.

| Order | Feature | Deliverable | Test | Status | Starts when |
|---:|---|---|---|---|---|
| 1 | notes | `brain note` surface, note schema, wikilinks over the vault | `tests/notes/` | NOT STARTED | — |
| 2 | tasks | `brain task` surface, lifecycle, atomic claim/finish, blocks | `tests/tasks/` | NOT STARTED | notes DONE |
| 3 | daemon | socket server, watcher, incremental index, daemon-down fallback | `tests/daemon/` | NOT STARTED | tasks DONE |
| 4 | search | hybrid BM25 + embeddings + RRF, tag-pull, JSON output | `tests/search/` | NOT STARTED | daemon DONE |
| 5 | mcp | `brain_*` MCP tools + SessionStart hook (Phase 2) | `tests/mcp/` | NOT STARTED | search DONE |

This single linear arc **is** the roadmap (small, single-goal repo). Cross-feature order is **only** here; feature plans declare same-feature unit deps only.

## Spec vs Implementation

| Gap | Feature / unit | Notes |
|---|---|---|
| No source code exists | all | `src/brain/` is unwritten; every feature is spec-only |

## Current Focus

The spec has passed a review-and-hardening pass (internal consistency, persona walk-throughs, and a competitive read against GBrain and the agent-memory field): the note `type → folder` map, exit-code ownership, config keys, `task release`, cancel-unblocks-dependents, minimal `task list` status/ownership visibility, MCP tool annotations + exposed `task cancel`, and daemon-independent wikilink resolution are all now specified, and every feature's Open Questions are closed. The next gate is human sign-off on the **notes** (feature 1) unit plan — see [features/notes/plan.md](features/notes/plan.md). No code lands until those units are approved; no work starts on `tasks` until `notes` is DONE.
