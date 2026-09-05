---
type: entrypoint
scope: product
children:
  - tech.md
  - design.md
  - plan.md
  - features/rust-rewrite/product.md
updated: 2026-09-05
---

# Mesh — Product

A mesh for multi-agent collaboration over one shared Markdown folder. The folder is divided into
named **spaces** — notes, tasks, memories, scratch, assets — and each space gets its own verb
family behind one instant Rust binary, plus `search`, read-only lenses and an MCP server. No
database, no background process: every command reads the folder directly.

**One-liner:** One folder, one binary, one mesh — notes and memories are recall, tasks are
coordination and handoff, scratch and assets are the working surface.

**Idea:** Many agents (e.g. flights-agent, notes-agent) plus a human operator share one vault as a
collaboration mesh. Sessions stop starting cold; knowledge is searchable; work is claimable task
files with live dependencies, not chat paste — agents coordinate through files over CLI and MCP,
not a bespoke protocol. Contrast [GBrain](https://github.com/garrytan/gbrain) (Postgres, graph,
job queue) and memory-only tools (Mem0, Basic Memory) that don't coordinate.

---

## Requirements

1. **One verb family per space.** `note`, `task`, `memory`, `scratch`, `asset` — plus `search`
   over any subset of them, read-only lenses (`recent-activity`, `build-context`, `graph`,
   `project`, `session-start`) and human-only admin (`init`, `status`, `reindex`, `watch`,
   `config`, `completions`). No sixth space and no lens becomes a space without a spec change.
   → [tech.md](tech.md) § Implemented surfaces
2. **Spaces are configuration, not layout.** Each space is a folder relative to the vault root,
   an absolute folder, the vault root itself, or disabled. The notes space may *be* the vault, so
   an existing Markdown vault is exposed as-is; foreign files stay readable and searchable and
   are never mutated. → [features/rust-rewrite/product.md](features/rust-rewrite/product.md)
3. **Markdown is truth.** Schema-valid frontmatter, unknown keys round-trip, clean bodies. Mesh
   owns the interface, the operator owns the vault.
4. **Tasks are handoff with a live graph.** `owner` / `claimed_by` / `claim` / `release` /
   `finish` / `cancel` / `list`, plus `blocks` / `blocked_by` readiness derived at read time,
   `block` / `unblock`, a strict claim gate and `task next`. Phase 3 is delivered.
5. **Memories are recall, not a memory subsystem.** Note-shaped Markdown files with a kind,
   scope, importance, optional source, soft expiry and supersession; recall ranks by match,
   importance and recency. Nothing is ever deleted automatically.
6. **Hybrid recall via `indexed`.** Ranked search over the configured spaces; a built-in ranked
   engine when `indexed` is absent, and a substring mode that reproduces the legacy scoring.
7. **`$MESH_AGENT` identity.** Defaults `--owner`, drives `--mine`, validated against
   `[tasks].collections` across every space. A spelling convention, never an authorisation
   boundary.
8. **No background process.** Every command works on its own; `mesh watch` is optional and only
   keeps the external index and folder routing fresh.
9. **Sandboxed vault.** Every path stays inside the union of the enabled space roots; agent
   content is data, never shell input.

---

## Phases

| Phase | Delivers | Done when | Status |
|---|---|---|---|
| **1 MVP** | note, task, search | Three CLI verbs; task claim/finish/cancel/list; hybrid search + fallback | ✅ delivered |
| **2 Agent** | MCP + SessionStart | Annotated `mesh_*` tools; `session-start` warm hook | ✅ delivered |
| **3 Graph** | task dependency graph | `ready`, strict gate, unblock report, `task next` | 🚧 delivered inside `rust-rewrite` |
| **4 Rust** | Rust binary + spaces | One binary, five spaces, memory/scratch/asset families, daemon removed | 🚧 in flight — [features/rust-rewrite/](features/rust-rewrite/product.md) |

---

## Features

| Feature | Role |
|---|---|
| notes | `note` — writes, wikilinks; coexists with any other writer on the folder |
| tasks | `task` — atomic claim, lifecycle, live dependency graph |
| search | `search` — `indexed` wrapper, ranked built-in engine, tag-pull |
| memory | MCP tools, session lenses, warm start |
| **rust-rewrite** | The Rust binary, the spaces model, the memory/scratch/asset families, the daemon's removal → [features/rust-rewrite/](features/rust-rewrite/product.md) |

Everything but `rust-rewrite` is implemented and hardened; each landed arc's cross-cutting
decisions are compounded into this root layer and its per-feature spec deleted. The source of
truth for delivered behaviour is the implementation plus [tech.md](tech.md) § Implemented
surfaces.

---

- **Read-your-writes (2026-08):** the daemon's warm index was watcher-driven, so a create followed
  immediately by a list could return a list without the agent's own new entity. Resolved
  permanently by the rewrite: there is no warm index and every command reads disk, so a write is
  visible to the next read by construction. → [tech.md](tech.md) § Invariants

## Non-Goals

No handoff primitive, DB, external task backend, enrichment loop, synthesis layer, job queue,
knowledge graph, dashboard/RBAC, sequential IDs, or git sync (versioning is the vault owner's
job). No daemon. No memory store that is not Markdown, and no automatic deletion of memories.

---

## Resolved

- **Rust rewrite (2026-09):** reversed. Mesh is re-implemented as a single Rust binary with a
  granular multi-space surface. The earlier decision to shelve the rewrite weighed only cold
  start against a Python floor of ~150–180 ms; what changed is the shape of the product — five
  spaces, a live dependency graph and agent hot loops that call mesh many times per turn, where
  that floor is paid per call and the warm daemon that hid it becomes a liability. The rewrite
  also deletes the daemon outright. **Supersedes** the "Rust rewrite shelved" decision recorded
  in [plan.md](plan.md) § Resolved. → [features/rust-rewrite/](features/rust-rewrite/product.md)
- **MCP annotations:** `read-only` / `idempotent` / `write` / `destructive`, now set explicitly on
  every tool. Cancel and release exposed; every removal verb and all admin withheld.
  → [tech.md](tech.md) § Implemented surfaces
- **`[tasks].collections`:** valid agent identities for `--owner` validation, enforced across
  every space; not folder splits. → [tech.md](tech.md)
- **Toolset direction (2026-07):** evaluated replacing mesh with GBrain (rejected — mandates
  Postgres/PGLite, no task primitive) and Beads (rejected — moved its source of truth to Dolt, a
  SQL DB, contradicting Markdown-as-truth). Decision: keep mesh, rework internals + performance +
  two additive capabilities (graph-query output, projects convention).
- **Vault coupling (2026-08):** naming the vault after a specific notes tool was dropped — it was
  never a code dependency, only a name and a set of spec claims. `[core].vault_path` is canonical,
  with two legacy input spellings kept as permanent aliases; versioning and backup are the vault
  owner's job, which is what justifies hard delete.
- **Package/CLI rename to `mesh` (2026-09):** package, CLI command, MCP server (`mesh-mcp`), MCP
  tool prefix (`mesh_*`), env vars, default config path and runtime paths were renamed from
  `shards` to `mesh`, aligning the code with the repo. Clean break, no compat aliases. Reference
  doc: [`docs/concepts.md`](../docs/concepts.md).
