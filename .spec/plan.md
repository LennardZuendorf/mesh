---
type: entrypoint
scope: implementation
covers: feature sequence, build order, validation criteria
updated: 2026-09-01
---

# Mesh — Plan

**Status:** Phases 1–2 **delivered** and hardened. Nine feature arcs are complete; every
per-feature spec has been compounded into the root layer and deleted. Unit-level truth is
`src/mesh/` + `tests/` (1455 tests, branch coverage on, `ty` clean, `ruff` clean).

**Focus:** none in flight. The next scheduled work is Phase 3 (`tasks-graph`), which stays
**deferred by product decision** — its dependency gate is satisfied, it is simply not scheduled.

---

## Sequence

| # | Feature | Status | Live surface |
|---:|---|---|---|
| 1 | notes | ✅ DONE | `src/mesh/core/notes.py`, `tests/notes/` |
| 2 | tasks | ✅ DONE | `src/mesh/core/tasks.py`, `tests/tasks/` |
| 3 | daemon | ✅ DONE | `src/mesh/daemon/`, `src/mesh/index/`, `tests/daemon/` |
| 4 | search | ✅ DONE | `src/mesh/index/`, `tests/search/` |
| 5 | memory | ✅ DONE | `src/mesh/mcp/`, `src/mesh/core/lenses.py`, `tests/memory/` |
| 6 | tasks-graph | ⏳ deferred (Phase 3) | — |
| 7 | mesh-rebrand | ✅ DONE | tree-wide rename; no live spec surface |
| 8 | cli-toolset-rework | ✅ DONE (unit 6 = Phase 3, deferred) | msgspec schemas, `graph`/`project` lenses, CI startup guard |
| 9 | vault-agnostic | ✅ DONE | `[core].vault_path` + permanent aliases → [tech.md](tech.md) § Contracts |
| 10 | core-hardening | ✅ DONE | warm reads, `cli/_errors.py`, `storage/locks.py` CAS → [tech.md](tech.md) § Invariants |
| 11 | team-awareness | ✅ DONE | inbound mentions, `task append`/`release`, `session-start --team` → [tech.md](tech.md) § Implemented surfaces |
| 12 | agent-usability | ✅ DONE | MCP `instructions`, tool schemas, flag contract, `mesh init` → [tech.md](tech.md) § Implemented surfaces |

---

## Ownership

| Feature | Owns |
|---|---|
| **notes** | schema, writes, wikilinks, config, global CLI |
| **tasks** | lifecycle, `O_EXCL` claim, release |
| **daemon** | socket, watcher, warm index, admin, `ChangeHooks` |
| **search** | `indexed_client`, tag-pull, fallback, `mesh search` |
| **memory** | MCP, `recent-activity`, `build-context`, `graph`, `project`, `session-start` |

**Freshness:** the watcher fires `ChangeHooks` on a bounded worker thread (never inline, so
search re-indexing cannot stall index freshness) → `indexed_client.incremental_update`. A writer
additionally pokes `vault.touch` so its own next read is current. `mesh reindex` →
`full_rebuild()`.

Cross-cutting contracts: [tech.md](tech.md) § Implemented surfaces.

---

## Open reviews

- **Product-positioning review** (cross-cutting). Critique the "mesh for multi-agent collaboration
  over CLI + MCP" repositioning against the three-verbs-over-a-folder mechanic: substantive or
  buzzword; where the story overclaims; `mesh` name collisions; differentiation vs GBrain /
  Mem0 / Basic Memory; consistency across `product.md`, `tech.md`, `design.md`, `README.md`,
  `AGENTS.md`, CLI help. Run in a fresh thread, adversarial; may reopen root `product.md`.
  Findings → `file:line` + concrete fix.

---

## Resolved

- **Rust rewrite for CLI startup performance** — evaluated and shelved; runtime stays Python 3.11+,
  optimized via the pydantic→msgspec swap (~150–180ms floor). → [tech.md](tech.md) § Risks
- **Vault adaptivity** — mesh is vault-*agnostic* (no notes app required, any folder accepted) but
  not vault-*adaptive*: `notes/` and `tasks/{open,done}/` are the layout contract, and a pre-existing
  Markdown file outside them is coexisted with, never indexed. Deliberate, and the honest limit on
  "works over any Markdown folder". → [product.md](product.md) § Non-Goals
