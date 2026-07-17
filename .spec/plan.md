---
type: entrypoint
scope: implementation
covers: feature sequence, build order, validation criteria
children:
  - features/shards-rebrand/plan.md
  - features/cli-toolset-rework/plan.md
updated: 2026-07-17
---

# Shards — Plan

**Status:** Phase 1–2 **delivered** — all five features implemented and tested (591 tests, mypy strict, ruff clean) on branch `feat/phase-1-mvp`. Built bottom-up behind binary whole-feature gates.

**Focus:** [cli-toolset-rework](features/cli-toolset-rework/plan.md) — keep-and-rework decision (GBrain/Beads rejected), internal tidying, performance push, graph-query output, projects convention. Phase 3 (tasks-graph) — `ready` / `release`, `blocks`/`blocked_by`, cycle-check, no parent-child hierarchy — is designed inside that feature (unit `cli-toolset-rework/6`, tech.md § Workstream D) but stays **deferred, gated on `cli-toolset-rework/1`–`5` DONE.**

---

## Sequence

| # | Feature | Status | Tests | Commit |
|---:|---|---|---|---|
| 1 | notes | ✅ DONE | `tests/notes/` | `feat(notes)` |
| 2 | tasks | ✅ DONE | `tests/tasks/` | `feat(tasks)` |
| 3 | daemon | ✅ DONE | `tests/daemon/` | `feat(daemon)` |
| 4 | search | ✅ DONE | `tests/search/` | `feat(search)` |
| 5 | memory | ✅ DONE | `tests/memory/` | `feat(memory)` |
| 6 | tasks-graph | ⏳ deferred (Phase 3; design in `cli-toolset-rework/6`) | `tests/tasks/` | — |
| 7 | [shards-rebrand](features/shards-rebrand/plan.md) | 🛠 in progress | full suite | `chore(rebrand)` |
| 8 | [cli-toolset-rework](features/cli-toolset-rework/plan.md) | 📝 scoped, units not started | full suite + new | — |

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

---

## Open reviews

- **Product-positioning review** (post-rebrand, cross-cutting — not rebrand-scoped). Critique the
  "mesh for multi-agent collaboration over CLI + MCP" repositioning against the three-verbs-over-a-folder
  mechanic: is it substantive or buzzword; does the story match the code / where does it overclaim;
  name `shards` collisions vs `brain`; differentiation vs GBrain / Mem0 / Basic Memory; internal
  consistency across `product.md`, `tech.md`, `design.md`, `README.md`, `AGENTS.md`, CLI help; what
  to cut or tone down. Run in a fresh thread, adversarial; may reopen root `product.md`. Findings →
  `file:line` + concrete fix. (Rebrand-correctness audit lives in
  [features/shards-rebrand/plan.md](features/shards-rebrand/plan.md) § Follow-ups — that one is the
  feature's own pre-merge gate.)

---

## Open questions

- **Rust rewrite for CLI startup performance** — under evaluation via parallel performance
  research; not decided. Runtime stays Python 3.11+ for now. →
  [features/cli-toolset-rework/tech.md](features/cli-toolset-rework/tech.md) § Open Questions
