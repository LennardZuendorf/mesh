---
type: entrypoint
scope: product
children:
  - tech.md
  - design.md
  - plan.md
updated: 2026-07-04
---

# Brain — Product

Thin coordination over one Tolaria Markdown folder. Three verbs — `note`, `task`, `search` — give a human and their agents shared memory and handoff. No database; the daemon accelerates but never gates.

**One-liner:** Three verbs, one daemon, one folder — notes + search = memory, tasks = handoff.

**Idea:** A human operator plus agents (e.g. flights-agent, tolaria-agent) share one vault. Sessions stop starting cold; knowledge is searchable; work is claimable task files, not chat paste. Contrast [GBrain](https://github.com/garrytan/gbrain) (Postgres, graph, job queue) and memory-only tools (Mem0, Basic Memory) that don't coordinate.

---

## Requirements

1. **Three verbs.** `note`, `task`, `search` cover capture, coordination, recall. Phase 2 adjuncts (signed off): `recent-activity`, `build-context`, `session-start` — read-only lenses, not a fourth verb. Admin (`daemon`, `status`, `reindex`) is human-only.
2. **Markdown is truth.** Schema-valid frontmatter; Brain owns the interface, Tolaria owns the vault/Git.
3. **Tasks are handoff.** Model: `owner`, `claimed_by`, `claim`/`finish`/`cancel`, later `blocks`/`blocked_by`. **v1:** claim/finish/cancel/list; graph edges recordable but inert until Phase 3.
4. **Hybrid recall via `indexed`.** Ranked search over `notes/` + `tasks/`; JSON with `path`; substring fallback when `indexed`/daemon unavailable or `[search].hybrid=false`.
5. **`$BRAIN_AGENT` identity.** Default `--owner`, `--mine` filter; unknown owners rejected via `[tasks].collections`.
6. **Graceful degradation.** All writes work daemon-down.
7. **Sandboxed vault.** Agent content is data, never shell input.

---

## Phases

| Phase | Delivers | Done when | Status |
|---|---|---|---|
| **1 MVP** | note, task, search, daemon | Three CLI verbs; task claim/finish/cancel/list; hybrid search + substring fallback | ✅ delivered |
| **2 Agent** | MCP + SessionStart | Annotated `brain_*` tools; `session-start` warm hook | ✅ delivered |
| **3 Graph** | task dependency graph | `ready`, `release`, strict gate, unblock-cascade | deferred |

---

## Features

| Feature | Role |
|---|---|
| notes | `note` — writes, wikilinks; coexists with Tolaria |
| tasks | `task` — atomic claim, lifecycle (graph deferred) |
| daemon | Watcher, warm index, socket, fallback |
| search | `search` — `indexed` wrapper, tag-pull, fallback |
| memory | MCP tools, memory lenses, warm-start |

All five are implemented (Phase 1–2). Per-feature specs have been compounded into this
root layer and removed — the source of truth is `src/brain/`, `tests/`, and
[tech.md](tech.md) § Implemented surfaces.

---

## Non-Goals

No memory primitive, handoff primitive, DB, external task backend, enrichment loop, synthesis layer, job queue, knowledge graph, dashboard/RBAC, sequential IDs, git sync, or replacing Tolaria.

---

## Resolved

- **MCP annotations:** `read-only` / `idempotent` / `write` / `destructive`. Cancel exposed; hard-delete and admin withheld. → [tech.md](tech.md) § Implemented surfaces
- **`[tasks].collections`:** valid agent identities for `--owner` validation; not folder splits. → [tech.md](tech.md)
