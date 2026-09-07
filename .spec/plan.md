---
type: entrypoint
scope: implementation
covers: feature sequence, build order, validation criteria
children: []
updated: 2026-09-07
---

# Mesh — Plan

**Status:** every phase delivered. Phases 1–2 shipped in Python across nine hardening arcs;
`rust-rewrite` then re-delivered the whole surface as a single Rust binary over five configurable
spaces, shipped the deferred dependency graph (phase 3), and removed the daemon. All arcs are
compounded into this root layer and their feature folders deleted.

**Focus:** none in flight. The next arc starts with `/spec feature <name>`. The one open item is
the product-positioning review below.

---

## Sequence

| # | Feature | Status | Live surface |
|---:|---|---|---|
| 1 | notes | ✅ DONE | `note` verb family, its tests |
| 2 | tasks | ✅ DONE | `task` verb family, its tests |
| 3 | daemon | ✅ DONE, then **removed** by `rust-rewrite` | replaced by direct reads + optional `mesh watch` |
| 4 | search | ✅ DONE | `search` verb, `indexed` wrapper, built-in engine |
| 5 | memory | ✅ DONE | MCP tools, session lenses |
| 6 | tasks-graph | ✅ DONE | derived readiness, `block`/`unblock`, strict claim, `task next` → `src/domain/deps.rs`, `tests/task_dep_cli.rs` |
| 7 | mesh-rebrand | ✅ DONE | tree-wide rename; no live spec surface |
| 8 | cli-toolset-rework | ✅ DONE | `graph`/`project` lenses, CI startup guard |
| 9 | vault-agnostic | ✅ DONE | `[core].vault_path` + permanent aliases → [tech.md](tech.md) § Contracts |
| 10 | core-hardening | ✅ DONE | lock compare-and-swaps, structured errors → [tech.md](tech.md) § Invariants |
| 11 | team-awareness | ✅ DONE | inbound mentions, `task append`/`release`, `session-start --team` |
| 12 | agent-usability | ✅ DONE | MCP instructions, tool schemas, flag contract, `mesh init` |
| 13 | rust-rewrite | ✅ DONE | the whole `src/` tree and `tests/`; five spaces (`src/spaces.rs`), the 37-tool MCP surface (`src/mcp/`), no daemon → [tech.md](tech.md) |

---

## Ownership

Every row is delivered. Cross-cutting contracts live in [tech.md](tech.md) § Implemented surfaces
and § Invariants, not in a feature folder — there are none.

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
  agent hot loops changed it, and the rewrite also deleted the daemon.
  → [tech.md](tech.md) § Performance.
- **Vault adaptivity** — mesh was vault-*agnostic* but not vault-*adaptive*. Resolved by the
  spaces model: the notes space may be the vault root itself, and foreign Markdown is readable
  and searchable (never mutated), so an existing vault is exposed as-is rather than coexisted
  with. → [tech.md](tech.md) § Invariants
