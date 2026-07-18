---
type: feature-plan
feature: cli-toolset-rework
sibling: tech.md
parent: ../../plan.md
updated: 2026-07-18
---

# Feature: CLI Toolset Rework — Implementation Plan

Six units: internal tidying, the performance push, the two additive capabilities, the gaps
punch list, and the deferred task-graph design — sequenced so the additive capabilities and gaps
build on tidied ground, with the task graph gated last. Units 1–5 are **shipped** on this branch
(implemented, reviewed, end-to-end verified — see § Progress); unit 6 stays deferred by product
decision.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Independent of the deferred root `tasks-graph` row except for unit 6 below,
which **is** that row's design — root `plan.md` treats `tasks-graph` as gated on
`cli-toolset-rework/1`–`5` DONE, not on any other feature. That gate is now satisfied (see §
Progress); unit 6 stays deferred by product decision, not by the gate.

---

## Requirements Trace

| ID | Requirement | Units | Status |
|---|---|---|---|
| R1 | [Graph-query output](product.md#requirement-graph-query-output) | cli-toolset-rework/3 | ✅ DONE |
| R2 | [Projects as a supported convention](product.md#requirement-projects-as-a-supported-convention) | cli-toolset-rework/4 | ✅ DONE |

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
5. **Rust rewrite: resolved, shelved.** Runtime stays Python 3.11+, optimized. → tech.md §
   Decisions.
6. **pydantic v2 → msgspec: adopted**, folded into unit 2, gated on a round-trip-fidelity spike
   (Invariant 3 — unknown frontmatter keys must still round-trip). → tech.md § Decisions.
7. **Type checker: mypy → ty.** New dev toolchain: uv · ruff · ty · pytest. → root `tech.md` §
   Stack, `AGENTS.md` § 3–4.

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

**Verification:** `uv run pytest -q` green, `uv run ty check src/` clean, `uv run ruff check .` clean; `git grep` for the old private-helper cross-import returns nothing.

---

### cli-toolset-rework/2 — Performance push (incl. pydantic → msgspec swap)

**Goal:** Ship the measured performance plan (tech.md § Workstream B): stop wrapping hot
invocations in `uv run`; swap `schemas/` pydantic → msgspec — **gated on a round-trip-fidelity
spike** (tech.md § Decisions); decompose eager CLI sub-verb imports (hygiene, not perf); add a CI
startup-time regression guard. Target: ~150–180ms cold start (down from ~230–300ms).

**Requirements:** — (non-functional; the "superfast and small" constraint)

**Dependencies:** —

**Ordering note:** the msgspec swap in this unit is foundational to `schemas/`. Unit
`cli-toolset-rework/4` (projects) also edits `schemas/note.py` + `schemas/task.py` — it must land
**after** this unit's swap, not just after unit 1 (see Dependencies table below). The round-trip
spike gates the swap itself; if it fails, this unit ships only the `uv run` + hygiene + CI-guard
tactics and the schema swap reverts to pydantic.

**Files:**

```
src/shards/schemas/note.py           # pydantic BaseModel -> msgspec Struct
src/shards/schemas/task.py           # pydantic BaseModel -> msgspec Struct
src/shards/schemas/config.py         # CoreConfig: pydantic BaseModel -> msgspec Struct;
                                      # expanduser() field-validator -> msgspec equivalent
src/shards/schemas/search.py         # pydantic BaseModel -> msgspec Struct
src/shards/cli/note.py               # pydantic.ValidationError -> msgspec.ValidationError
src/shards/cli/task.py               # pydantic.ValidationError -> msgspec.ValidationError
src/shards/core/notes.py             # pydantic.ValidationError -> msgspec.ValidationError
src/shards/core/tasks.py             # pydantic.ValidationError -> msgspec.ValidationError
src/shards/cli/__main__.py           # decompose eager sub-verb imports (hygiene, not perf)
.github/workflows/ci.yml             # startup-time guard (shared with unit 5)
```

**Test scenarios:**

- **Gating spike:** a note/task file carrying unknown/foreign frontmatter keys round-trips
  byte-for-byte through msgspec `Struct`s exactly as it did under pydantic — proven **before** the
  full swap proceeds.
- `shards --help` / `note new` / `task claim` cold-start time recorded before and after; regression
  guard fails CI on regression past the recorded baseline.
- No behavior change to any command's inputs/outputs.

**Verification:** Baseline + post numbers recorded in this plan's Progress notes; round-trip
fidelity spike result recorded (pass → swap proceeds / fail → swap reverts) before merge; CI guard
job passes on this branch; `uv run ty check src/` clean.

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

**Dependencies:** cli-toolset-rework/1 (read-lens layer); cli-toolset-rework/2 (this unit edits
`schemas/note.py` + `schemas/task.py` — must land after the msgspec swap, not before)

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

**Goal:** Add CI (pytest + ruff + ty + startup guard + spec-test-count check), pin the `indexed` NDJSON contract with a shared schema + `search --health`, document the owner-identity trust boundary, evaluate optional soft-delete.

**Requirements:** — (non-functional hardening)

**Dependencies:** cli-toolset-rework/1, cli-toolset-rework/2

**Files:**

```
.github/workflows/ci.yml        # pytest + ruff + ty + startup guard + spec-test-count check
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
| cli-toolset-rework/2 | 4, 5, 6 | — |
| cli-toolset-rework/3 | 6 | cli-toolset-rework/1 |
| cli-toolset-rework/4 | 6 | cli-toolset-rework/1, cli-toolset-rework/2 |
| cli-toolset-rework/5 | 6 | cli-toolset-rework/1, cli-toolset-rework/2 |
| cli-toolset-rework/6 | — | cli-toolset-rework/1–5 |

---

## Progress

All of 1–5 are implemented, reviewed, and end-to-end verified on branch
`claude/cli-toolset-rework-xys6hl` — full suite green (678 tests), `ty check src/` clean, `ruff
check`/`ruff format --check` clean. Unit 6 stays deferred by product decision (§ above), not by
any unmet dependency.

| Unit | Status | Evidence |
|---|---|---|
| cli-toolset-rework/1 | ✅ DONE | Internal tidying landed — `5b172bc`; `core/search.py`, `index/{warm,watcher,reconcile}.py`, daemon-owned `ChangeHooks`, `core/lenses.py` all on this branch. |
| cli-toolset-rework/2 | ✅ DONE | Performance push landed — `c9fc813`, `e7d47ba`, `af1b3f6`, `a130cb8`. Round-trip-fidelity spike **passed**; msgspec swap shipped (not reverted). CI startup guard live (`tests/test_startup_guard.py`, `.github/workflows/ci.yml`). |
| cli-toolset-rework/3 | ✅ DONE | Graph-query landed — `91184c2`, `ca6d0c4`, `9bcd27b`. `shards graph <id>` CLI (`cli/session.py`) + `shards_graph` MCP tool (`mcp/server.py`, read-only), JSON + tree from one BFS. |
| cli-toolset-rework/4 | ✅ DONE | Projects convention landed — `f2ba754`, `2d1f0a4`. `type: project` note → `notes/projects/`; optional `project` task field; `--project` on `task new`/`update`/`list`; `shards project <id>` CLI + `shards_project` MCP tool. No new verb. |
| cli-toolset-rework/5 | ✅ DONE | Gaps closed — `e93d375`, `c546183`, `d2056ab`, `992db81`. `.github/workflows/ci.yml` (uv · ruff · ruff format · ty · pytest · startup guard); `indexed` NDJSON contract pinned (msgspec schema, `index/indexed_client.py`); `search --health`; owner-identity trust-boundary + delete-stance doc in `AGENTS.md` § 6. Soft-delete stays evaluated-and-deferred (see `AGENTS.md` § 6). |
| cli-toolset-rework/6 | NOT STARTED (gated, deferred) | Dependency gate (1–5 DONE) is now satisfied; build stays **deferred by product decision** — design final in tech.md § Workstream D, not scheduled. |

