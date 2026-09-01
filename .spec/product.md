---
type: entrypoint
scope: product
children:
  - tech.md
  - design.md
  - plan.md
updated: 2026-08-23
---

# Shards — Product

A mesh for multi-agent collaboration over one shared Markdown folder. Three verbs — `note`, `task`, `search` — give a fleet of agents and their human operator a shared substrate to capture knowledge and coordinate work through low-level tools: a CLI and an MCP server. No database; the daemon accelerates but never gates.

**One-liner:** Three verbs, one folder, one mesh — notes + search = shared memory, tasks = coordination + handoff.

**Idea:** Many agents (e.g. flights-agent, notes-agent) plus a human operator share one vault as a collaboration mesh. Sessions stop starting cold; knowledge is searchable; work is claimable task files, not chat paste — agents coordinate through files over CLI and MCP, not a bespoke protocol. Contrast [GBrain](https://github.com/garrytan/gbrain) (Postgres, graph, job queue) and memory-only tools (Mem0, Basic Memory) that don't coordinate.

---

## Requirements

1. **Three verbs.** `note`, `task`, `search` cover capture, coordination, recall. Read-only lenses, not a fourth verb — all shipped: `recent-activity`, `build-context`, `session-start`, `graph` (`--direction in|out|both`), and **projects** as a convention (`type: project` note, `project` task field, scoped view). Admin (`init`, `daemon`, `status`, `reindex`) is human-only. → [tech.md](tech.md) § Implemented surfaces
2. **Markdown is truth.** Schema-valid frontmatter; Shards owns the interface, the operator owns the vault.
3. **Tasks are handoff.** Model: `owner`, `claimed_by`, `claim`/`release`/`finish`/`cancel`, later `blocks`/`blocked_by`. **v1:** claim/release/append/finish/cancel/list, with inbound mentions and a team-wide `session-start`; graph edges recordable but inert until Phase 3.
4. **Hybrid recall via `indexed`.** Ranked search over `notes/` + `tasks/`; JSON with `path`; substring fallback when `indexed`/daemon unavailable or `[search].hybrid=false`.
5. **`$SHARDS_AGENT` identity.** Default `--owner`, `--mine` filter; unknown owners rejected via `[tasks].collections`.
6. **Graceful degradation.** All writes work daemon-down.
7. **Sandboxed vault.** Agent content is data, never shell input.

---

## Phases

| Phase | Delivers | Done when | Status |
|---|---|---|---|
| **1 MVP** | note, task, search, daemon | Three CLI verbs; task claim/finish/cancel/list; hybrid search + substring fallback | ✅ delivered |
| **2 Agent** | MCP + SessionStart | Annotated `shards_*` tools; `session-start` warm hook | ✅ delivered |
| **3 Graph** | task dependency graph | `ready`, strict gate, unblock-cascade | deferred (`release` shipped ahead of it — it is the missing half of `claim`, not a graph feature) |

---

## Features

| Feature | Role |
|---|---|
| notes | `note` — writes, wikilinks; coexists with any other writer on the folder |
| tasks | `task` — atomic claim, lifecycle (graph deferred) |
| daemon | Watcher, warm index, socket, fallback |
| search | `search` — `indexed` wrapper, tag-pull, fallback |
| memory | MCP tools, memory lenses, warm-start |

All five are implemented (Phase 1–2), then hardened and extended by four further tracks —
core-hardening, team-awareness, agent-usability and vault-agnostic — whose cross-cutting
decisions are compounded into this root layer. Every per-feature spec has been removed: the
source of truth is `src/shards/`, `tests/`, and [tech.md](tech.md) § Implemented surfaces.

---

- **Read-your-writes (2026-08):** the daemon's warm index is watcher-driven, so a create followed
  immediately by a list — the commonest agent sequence there is — could return a list without the
  agent's own new entity. A writer now best-effort-notifies the daemon after the file is durable.
  The invariant narrows from "writes bypass the socket" to the one that carries the actual promise:
  **the daemon never gates a write.** → [tech.md](tech.md) § Invariants

## Non-Goals

No memory primitive, handoff primitive, DB, external task backend, enrichment loop, synthesis layer, job queue, knowledge graph, dashboard/RBAC, sequential IDs, or git sync (versioning is the vault owner's job).

---

## Resolved

- **MCP annotations:** `read-only` / `idempotent` / `write` / `destructive`. Cancel and release exposed; hard-delete and admin (including `init`) withheld. → [tech.md](tech.md) § Implemented surfaces
- **`[tasks].collections`:** valid agent identities for `--owner` validation; not folder splits. → [tech.md](tech.md)
- **Toolset direction (2026-07):** evaluated replacing shards with GBrain (rejected — mandates Postgres/PGLite, no task primitive) and Beads (rejected — moved its source of truth to Dolt, a SQL DB, contradicting Markdown-as-truth). Decision: keep shards, rework internals + performance + two additive capabilities (graph-query output, projects convention); task-graph (Phase 3) designed but stays deferred.
- **Vault coupling (2026-08):** naming the vault after a specific notes tool was dropped — it was
  never a code dependency (no package, import, subprocess or MCP call), only a name and a set of
  spec claims. `[core].vault_path` is canonical, with two legacy input spellings kept as permanent
  aliases; versioning and backup are the vault owner's job, which is now what justifies hard
  delete. Supersedes the earlier rebrand decision to keep that coupling unchanged.
