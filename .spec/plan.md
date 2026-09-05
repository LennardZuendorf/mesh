---
type: entrypoint
scope: implementation
covers: feature sequence, build order, validation criteria
children:
  - features/rust-rewrite/plan.md
updated: 2026-09-05
---

# Mesh — Plan

**Status:** Phases 1–2 delivered in Python and hardened across nine feature arcs, all compounded
into the root layer. The implementation is now being replaced: `rust-rewrite` re-delivers the
whole surface as a single Rust binary over five configurable spaces, ships the deferred
dependency graph, and removes the daemon.

**Focus:** `rust-rewrite` — in flight. Units and verification:
[features/rust-rewrite/plan.md](features/rust-rewrite/plan.md).

---

## Sequence

| # | Feature | Status | Live surface |
|---:|---|---|---|
| 1 | notes | ✅ DONE | `note` verb family, its tests |
| 2 | tasks | ✅ DONE | `task` verb family, its tests |
| 3 | daemon | ✅ DONE, then **removed** by `rust-rewrite` | replaced by direct reads + optional `mesh watch` |
| 4 | search | ✅ DONE | `search` verb, `indexed` wrapper, built-in engine |
| 5 | memory | ✅ DONE | MCP tools, session lenses |
| 6 | tasks-graph | ✅ delivered inside `rust-rewrite` | derived readiness, `block`/`unblock`, strict claim, `task next` → [features/rust-rewrite/plan.md](features/rust-rewrite/plan.md) |
| 7 | mesh-rebrand | ✅ DONE | tree-wide rename; no live spec surface |
| 8 | cli-toolset-rework | ✅ DONE | `graph`/`project` lenses, CI startup guard |
| 9 | vault-agnostic | ✅ DONE | `[core].vault_path` + permanent aliases → [tech.md](tech.md) § Contracts |
| 10 | core-hardening | ✅ DONE | lock compare-and-swaps, structured errors → [tech.md](tech.md) § Invariants |
| 11 | team-awareness | ✅ DONE | inbound mentions, `task append`/`release`, `session-start --team` |
| 12 | agent-usability | ✅ DONE | MCP instructions, tool schemas, flag contract, `mesh init` |
| 13 | **rust-rewrite** | 🚧 IN PROGRESS | [features/rust-rewrite/plan.md](features/rust-rewrite/plan.md) |

---

## Ownership

| Feature | Owns |
|---|---|
| **rust-rewrite** | the binary and its whole surface, the spaces model, the memory/scratch/asset families, the task graph, the daemon's removal, packaging, CI, and the deletion of the Python tree |

Everything above row 13 is delivered; its cross-cutting contracts live in
[tech.md](tech.md) § Implemented surfaces, not in a feature folder.

---

## Open reviews

- **Product-positioning review** (cross-cutting). Critique the "mesh for multi-agent collaboration
  over CLI + MCP" repositioning against the one-folder mechanic, now that the surface is five
  spaces rather than three verbs: substantive or buzzword; where the story overclaims; `mesh`
  name collisions; differentiation vs GBrain / Mem0 / Basic Memory; consistency across
  `product.md`, `tech.md`, `design.md`, `README.md`, `AGENTS.md`, CLI help. Run in a fresh
  thread, adversarial; may reopen root `product.md`. Findings → `file:line` + concrete fix.

---

## Resolved

- **Rust rewrite for CLI startup performance** — shelved in 2026-08, **reversed 2026-09**. The
  original trade weighed a three-verb CLI's cold start alone; the granular multi-space surface and
  agent hot loops changed it, and the rewrite also deletes the daemon. See `rust-rewrite`
  → [features/rust-rewrite/product.md](features/rust-rewrite/product.md) and
  [tech.md](tech.md) § Performance.
- **Vault adaptivity** — mesh was vault-*agnostic* but not vault-*adaptive*. Resolved by the
  spaces model: the notes space may be the vault root itself, and foreign Markdown is readable
  and searchable (never mutated), so an existing vault is exposed as-is rather than coexisted
  with. → [features/rust-rewrite/product.md](features/rust-rewrite/product.md)
