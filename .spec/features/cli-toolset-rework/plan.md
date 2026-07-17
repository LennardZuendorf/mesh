---
type: feature-plan
feature: cli-toolset-rework
sibling: tech.md
parent: ../../plan.md
updated: 2026-07-17
---

# Feature: CLI Toolset Rework — Implementation Plan

Scoping only on this branch — no `src/` changes yet. Five units: internal tidying, the
performance push, the two additive capabilities, the gaps punch list, and the deferred
task-graph design — sequenced so the additive capabilities and gaps build on tidied ground,
with the task graph gated last.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Independent of the deferred root `tasks-graph` row except for unit 5 below,
which **is** that row's design — root `plan.md` should treat `tasks-graph` as gated on
`cli-toolset-rework/1`–`4` DONE, not on any other feature.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Graph-query output](product.md#requirement-graph-query-output) | cli-toolset-rework/3 |
| R2 | [Projects as a supported convention](product.md#requirement-projects-as-a-supported-convention) | cli-toolset-rework/4 |

---

## Key Technical Decisions

1. **Keep and rework, not replace.** GBrain (Postgres/PGLite, no task primitive) and Beads
   (source of truth moved to Dolt/SQL) were both rejected. → tech.md § Constraints, product.md §
   Decision.
2. **Optimize the existing architecture, don't restructure.** Daemon + hybrid search stay;
   workstream A is behavior-preserving. → tech.md § Workstream A.
3. **Projects are a convention, not a verb.** Additive frontmatter + a scoped view; explicit
   evolution path if it earns a verb later. → tech.md § Workstream C.
4. **Task graph stops at blocks + ready + release for v1.** No parent-child hierarchy — the
   bug class Beads shipped. → tech.md § Workstream D.
5. **Rust rewrite is an open question, not a decision.** Runtime stays Python 3.11+ pending
   parallel performance research. → tech.md § Open Questions.

---

## Unit IDs

Units are `cli-toolset-rework/n`. Cite in commits (`refactor(search): cli-toolset-rework/1 ...`).

---

### cli-toolset-rework/1 — Internal tidying

**Goal:** Extract `core/search.py`, decompose `index/watch.py` into `warm.py`/`watcher.py`/`reconcile.py`, replace the module-level hook-registry global with a daemon-owned object, and group the read lenses into one composable layer — no behavior change.

**Requirements:** — (non-functional; enables R1's read-lens reuse)

**Dependencies:** —

**Files:**

```
src/shards/core/search.py       # new — drains cli/search.py + mcp/server.py's private reach-ins
src/shards/cli/search.py        # thins to a wrapper over core/search.py
src/shards/mcp/server.py        # stops importing cli/search.py private helpers
src/shards/index/watch.py       # removed
src/shards/index/warm.py        # new — in-memory index
src/shards/index/watcher.py     # new — watchdog adapter, guarded callbacks
src/shards/index/reconcile.py   # new — folder reconcile
src/shards/daemon/server.py     # owns the hook-registry object (was module-level global)
```

**Test scenarios:**

- Full existing suite (~591 tests) passes unmodified in behavior — only import paths change.
- No test imports `cli/search.py`'s private `_hit_dict`/`_query_search` from `mcp/server.py`.

**Verification:** `uv run pytest -q` green, `uv run mypy src/` clean, `uv run ruff check .` clean; `git grep` for the old private-helper cross-import returns nothing.

---

### cli-toolset-rework/2 — Performance push

**Goal:** Baseline CLI cold-start, extend lazy-import discipline to `pydantic`/`FastMCP`/`python-frontmatter`, trim dependencies, add a CI startup-time regression guard.

**Requirements:** — (non-functional; the "superfast and small" constraint)

**Dependencies:** —

**Files:**

```
src/shards/cli/__main__.py          # audit + lazy-import heavy deps
src/shards/schemas/*.py             # audit pydantic import placement
src/shards/mcp/server.py            # audit FastMCP import placement
.github/workflows/ci.yml            # startup-time guard (shared with unit 5)
```

**Test scenarios:**

- `shards --help` cold-start time recorded before and after; regression guard fails CI on regression past the recorded baseline.
- No behavior change to any command.

**Verification:** Baseline + post numbers recorded in this plan's Progress notes; CI guard job passes on this branch.

---

### cli-toolset-rework/3 — Graph-query output

**Goal:** Promote `build-context`'s BFS-over-`related` traversal to a first-class query surface with JSON + tree output.

**Requirements:** R1

**Dependencies:** cli-toolset-rework/1 (read-lens layer)

**Files:**

```
src/shards/core/context.py     # expose the BFS as a directly callable, first-class query
src/shards/cli/*.py            # new CLI surface for the graph query
src/shards/mcp/server.py       # corresponding shards_* tool, read-only annotation
```

**Test scenarios:**

- Query against a multi-hop `related` chain returns every reachable id, deduped for cycles/diamonds.
- JSON and tree outputs both derive from one traversal result; no hybrid-search dependency.

**Verification:** New tests under `tests/` mirroring `tests/memory/test_build_context.py`'s traversal cases; `uv run pytest -q` green.

---

### cli-toolset-rework/4 — Projects as a convention

**Goal:** `type: project` note, `project:` field on tasks, `task list --project <id>`, and a project-scoped read-lens.

**Requirements:** R2

**Dependencies:** cli-toolset-rework/1 (read-lens layer)

**Files:**

```
src/shards/schemas/note.py    # type enum gains "project"
src/shards/schemas/task.py    # optional project field (round-tripped)
src/shards/cli/task.py        # --project filter on task list
src/shards/core/*.py          # project-scoped read-lens
```

**Test scenarios:**

- A project note + tasks carrying its id in `project:` filter correctly via `task list --project <id>`.
- Tasks without `project:` are unaffected; frontmatter round-trips unknown/legacy project values.
- No new CLI verb is introduced.

**Verification:** New tests under `tests/tasks/`; `uv run pytest -q` green; `shards --help` shows no new verb, only a new flag/lens.

---

### cli-toolset-rework/5 — Gaps: CI, contract pinning, trust boundary, soft-delete

**Goal:** Add CI (pytest + mypy + ruff + startup guard + spec-test-count check), pin the `indexed` NDJSON contract with a shared schema + `search --health`, document the owner-identity trust boundary, evaluate optional soft-delete.

**Requirements:** — (non-functional hardening)

**Dependencies:** cli-toolset-rework/1, cli-toolset-rework/2

**Files:**

```
.github/workflows/ci.yml        # pytest + mypy + ruff + startup guard + spec-test-count check
src/shards/index/indexed_client.py  # shared NDJSON schema
src/shards/cli/search.py        # --health / status signal
docs or AGENTS.md               # owner-identity trust-boundary note
src/shards/storage/files.py     # soft-delete evaluation (may land as a follow-up, not required)
```

**Test scenarios:**

- CI runs on push/PR and fails on a lint/type/test/startup-time/spec-drift regression.
- `search --health` reports `indexed` reachability distinctly from a substring-fallback result.

**Verification:** CI green on this branch's own PR-equivalent run; `uv run pytest -q` covers the new health signal.

---

### cli-toolset-rework/6 — Deferred: task-graph (Phase 3) design → build

**Goal:** Activate `blocks`/`blocked_by`, compute `ready`, add `task ready [--claim]`, surface newly-ready tasks on `release`, cycle-check at write time. **Design is final (tech.md § Workstream D); build is gated.**

**Requirements:** — (this is root `plan.md`'s existing deferred `tasks-graph` row)

**Dependencies:** cli-toolset-rework/1, cli-toolset-rework/2, cli-toolset-rework/3, cli-toolset-rework/4, cli-toolset-rework/5 all DONE

**Files:** — (not started; see tech.md § Workstream D for the eventual shape)

**Test scenarios:** — (deferred)

**Verification:** — (deferred; do not start until the gate above is DONE)

---

## Dependencies

| Unit | Blocks | Blocked by |
|---|---|---|
| cli-toolset-rework/1 | 3, 4, 5, 6 | — |
| cli-toolset-rework/2 | 5, 6 | — |
| cli-toolset-rework/3 | 6 | cli-toolset-rework/1 |
| cli-toolset-rework/4 | 6 | cli-toolset-rework/1 |
| cli-toolset-rework/5 | 6 | cli-toolset-rework/1, cli-toolset-rework/2 |
| cli-toolset-rework/6 | — | cli-toolset-rework/1–5 |

---

## Progress

| Unit | Status |
|---|---|
| cli-toolset-rework/1 | NOT STARTED |
| cli-toolset-rework/2 | NOT STARTED |
| cli-toolset-rework/3 | NOT STARTED |
| cli-toolset-rework/4 | NOT STARTED |
| cli-toolset-rework/5 | NOT STARTED |
| cli-toolset-rework/6 | NOT STARTED (gated, deferred) |

---

## Open Questions

1. **Rust rewrite for CLI startup performance** — pending parallel performance research; not
   decided on this branch. See [tech.md](tech.md) § Open Questions. Unit 2's baseline numbers
   should inform, not preempt, that decision.
