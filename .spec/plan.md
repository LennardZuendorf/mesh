---
type: entrypoint
scope: implementation
covers: feature sequence, build order, validation criteria
children: []
updated: 2026-09-08
---

# Mesh — Plan

**Status:** every phase delivered. Phases 1–2 shipped in Python across nine hardening arcs;
`rust-rewrite` then re-delivered the whole surface as a single Rust binary over five configurable
spaces, shipped the deferred dependency graph (phase 3), and removed the daemon. All arcs are
compounded into this root layer and their feature folders deleted.

**Focus:** none in flight. The next arc starts with `/spec feature <name>`. Two adversarial
review sweeps over the landed Rust surface are closed and compounded into
[tech.md](tech.md) § Invariants and § Implemented surfaces; the open items are the
product-positioning review and the two known gaps below.

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
  **One finding is already banked:** the clap `about` strings still describe the Python-era
  three-verb surface — the root command says "Three verbs, one folder" and `search` says "Recall
  across notes + tasks", where the live surface is five spaces and the default corpus is notes,
  tasks, memories and assets. They are user-visible copy in `src/cli/mod.rs`, so the fix belongs
  to this review, not to a docs pass.

## Known gaps

- **`mesh watch` leaks its lock on SIGINT.** `run` holds the singleton `LockGuard` and a signal
  skips `Drop`, so an interrupted watcher leaves the lock behind and never emits its closing
  `stop` event. The lock self-heals on its TTL, so this is a protocol and hygiene gap, not a
  wedge. Closing it needs a signal handler setting the existing `stop` flag, which needs a crate
  (`#![forbid(unsafe_code)]` rules out a hand-written one) — a **dependency change**, so it waits
  on a decision rather than being fixed in passing. → [tech.md](tech.md) § Stack

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
