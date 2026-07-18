---
type: feature-tech
feature: cli-toolset-rework
sibling: product.md
parent: ../../tech.md
updated: 2026-07-18
---

# Feature: CLI Toolset Rework — Architecture

Four workstreams over the existing architecture (daemon + hybrid search stay; optimize, don't
restructure): **A** behavior-preserving internal tidying, **B** a dedicated performance push,
**C** two small additive capabilities, **D** a deferred, gated task-graph design. Plus a punch
list of gaps (CI, `indexed` contract, positioning review, trust boundary, soft-delete) closed
opportunistically.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Constraints

| Constraint | Source |
|---|---|
| Markdown/git is the source of truth | owner |
| No database to operate | owner |
| CLI **and** MCP both stay supported | owner |
| "Superfast and small" | owner |
| Keep the current architecture (daemon + hybrid search); optimize, don't restructure | owner |

---

## Decisions

### Rust rewrite — RESOLVED: shelved

No rewrite; runtime stays Python 3.11+, optimized (Workstream B below). Parallel performance
research measured Rust's cold start at ~2–10ms vs Python's measured ~260–300ms today — a real
gap, but below human-perceptibility for how shards is actually invoked (agent tool calls + human
CLI, not a hot loop). Cost side: re-implementing ~5.4k LOC + 678 tests, losing
`pydantic`/`FastMCP`/`python-frontmatter`, and betting the MCP surface on `rmcp` (the official
Rust MCP SDK, **pre-1.0**). Not worth it.

**Only future fallback (not scheduled, not this branch):** a thin Rust *client* over the existing
daemon NDJSON socket protocol, if a genuine high-frequency/hot-loop use ever emerges. → root
`tech.md` § Risks.

### pydantic v2 → msgspec — ADOPTED, SHIPPED

**Gating spike — passed (read first):** root `tech.md` Invariant 3 — "unknown frontmatter keys
round-trip" — is load-bearing (also see `lessons.md` § "Foreign-file tolerance must be symmetric
across every reader"). msgspec `Struct`s reject unknown fields by default; pydantic's
`extra="allow"` does not. The migration had to preserve byte-for-byte round-trip fidelity of
unknown/foreign keys. That round-trip-fidelity spike (unit `cli-toolset-rework/2`) **passed** —
proved via a `_Frontmatter` stash on each `Struct` that round-trips unknown keys, locked by
`tests/schemas/` (including a foreign-temporal round-trip regression test) — so the full swap
proceeded and shipped; it was not reverted.

**Why:** pydantic v2 pays a one-time ~78–110ms schema-compile tax the moment the first
`BaseModel` subclass (`schemas/config.py`'s `CoreConfig`) is defined — unavoidable on any real
command, since lazy-importing pydantic doesn't help (the class *definition*, not the import,
pays the cost). Measured: msgspec import+define ~33ms vs pydantic ~120–130ms. Swapping `schemas/`
to msgspec takes cold start from ~230–250ms to **~150–180ms**.

**Scope:** `src/shards/schemas/*.py` (note, task, config, search) + every
`pydantic.ValidationError` call site (`cli/note.py`, `cli/task.py`, `core/notes.py`,
`core/tasks.py`). Field-validator behavior (e.g. `expanduser()` on config paths — `lessons.md` §
"Resolve user paths") and the `ValidationError` catches must be reimplemented with msgspec
equivalents (`msgspec.ValidationError`, `__post_init__` / `msgspec.convert` hooks). Keep
`python-frontmatter` for the frontmatter split — msgspec only replaces schema validation +
serialization.

---

## Workstream A — Internal tidying (behavior-preserving)

No behavior change; all ~591 tests stay green throughout.

| Change | Files | Why |
|---|---|---|
| Extract `core/search.py` | new `src/shards/core/search.py`; drains logic out of `cli/search.py` | Today `mcp/server.py` reaches into `cli/search.py`'s private `_hit_dict`/`_query_search` — a cross-layer coupling `core` exists to prevent everywhere else |
| Decompose `index/watch.py` (445 LOC) | new `index/warm.py` (in-memory index), `index/watcher.py` (watchdog adapter, guarded callbacks — keep the existing "never let an exception escape a watchdog callback" guard), `index/reconcile.py` (folder reconcile) | One 445-line file mixes three concerns; splitting them is the same behavior, clearer ownership |
| Replace the module-level mutable hook-registry global | daemon owns an object instance instead | Global mutable registries are a hidden-coupling smell; the daemon already owns the watcher lifecycle, so it should own the registry too |
| Group read lenses into one composable layer | `recent-activity` / `build-context` / `session-start` (`core/activity.py`, `core/context.py`, `cli/session.py`) | These three already share the warm-index-or-fallback shape (tech.md § Implemented surfaces); today they're wired ad hoc per caller — one composable layer is the natural home, and workstream C's graph-query output plugs into it |

---

## Workstream B — Superfast & small (performance)

<!-- merge -->
Goal and principle are cross-cutting and belong in root `tech.md` (see root § Performance);
this section holds the feature-scoped execution plan only.
<!-- /merge -->

Measured plan, in order:

1. **Free win — stop wrapping in `uv run`.** For agent/MCP/scripted invocations, call the
   installed console script or `python -m shards.cli.__main__` directly instead of
   `uv run shards ...` (~25–30ms measured).
2. **Big lever — pydantic → msgspec.** See § Decisions above. ~90ms saved, gated on the
   round-trip-fidelity spike. This is the path to the ~150–180ms floor.
3. **Hygiene, not perf.** Decomposing the eager sub-verb imports in `cli/__main__.py` saves only
   ~0–20ms — every verb hits the same schema floor regardless of import order — so this is
   future-proofing, not a speed fix.
4. **Guard.** Baseline cold-start numbers recorded (`shards --help`, `note new`, `task claim`); a
   startup-time regression guard added to CI (workstream gaps, below) so a re-introduced eager
   import fails the build, not just a future profiling pass.

**Measured non-levers — no action:**

- `FastMCP` is already off the CLI hot path (separate `shards-mcp` console script).
- `python-frontmatter` already uses PyYAML's C loader (`CSafeLoader`).
- typer's `rich` help rendering is already lazy (help/error path only).
- `PYDANTIC_DISABLE_PLUGINS` had no measurable effect.

---

## Workstream C — Two additive capabilities

### C1 — Graph-query output

Promotes `build-context`'s existing BFS over the `related` wikilink graph (`core/context.py`,
cycle/diamond-deduped, seed-first — see root `tech.md` § Implemented surfaces) from an
internal helper of the `build-context` lens into a first-class, directly queryable surface.

- Same traversal, same dedup/seed-first guarantees — no new graph engine.
- Output: JSON (id list / edge list) **and** a readable tree, both derived from one BFS result.
- Explicitly does not touch or require hybrid search — `search` stays the ranked-recall path;
  this is the structural "what's connected to X" path.

### C2 — Projects as a first-class supported convention

- `type: project` note — a note like any other; no schema change beyond the existing `type`
  enum gaining a value.
- `project:` field on task frontmatter — round-tripped like any unknown/optional key (tech.md §
  Invariant 3); optional, not required.
- Project-scoped view: `task list --project <id>` filter, plus a project read-lens alongside
  `recent-activity` / `build-context` / `session-start` (workstream A's read-lens layer is where
  this plugs in).
- **Evolution path, recorded not built:** structured so a `project` verb could graduate from this
  convention later if usage earns it. No verb ships this branch — the three-verb thesis
  (`note`/`task`/`search`) holds; any future verb needs its own spec sign-off per root `AGENTS.md`
  § 0.

---

## Workstream D — Deferred minimal task graph (Phase 3)

Design only — sequenced **after** workstreams A–C ship; this is the concrete shape for root
`plan.md`'s already-deferred `tasks-graph` row, refined from "ready/release, strict gate,
unblock-cascade" to the following v1-scoped mechanics:

1. Activate the already-on-disk `blocks` / `blocked_by` fields (currently inert — root `tech.md`
   Task adds).
2. Compute `ready` — an open task whose every `blocked_by` entry is `status: done`.
3. `task ready [--claim]` — atomic self-select, reusing the existing `O_EXCL` claim mechanic
   (root `tech.md` § Contracts, Locks).
4. `release` on `finish` surfaces newly-ready tasks in the output — pull-time unblock, honest to
   the no-push design (no daemon-driven cascade write).
5. Cycle-check at write time — reject a `blocks`/`blocked_by` edge that would create a cycle.

**Stop condition:** v1 stops at blocks + ready + release. **No parent-child hierarchy** — this is
exactly where the evaluated alternative (Beads) shipped bugs. Richer edge types stay as
round-tripped annotations (inert data), never machinery, unless a future spec reopens this.

---

## Gaps to close (opportunistic)

| Gap | Detail |
|---|---|
| No CI | **Resolved.** `.github/workflows/ci.yml` runs `ruff check` + `ruff format --check` + `ty check src/` + `pytest -q` + the startup-time guard on push/PR. The automated spec-test-count drift check was deliberately left out of CI (see the workflow file's own header comment) — the count is synced manually in the spec at each branch's finishing phase, as this doc sync does. |
| Spec drift | Root spec said 578, then 591, tests; actual is now 678 — synced here (root `tech.md`, `plan.md`, `AGENTS.md` § 4). |
| `indexed` contract unpinned | **Resolved.** NDJSON hit contract pinned with a shared msgspec schema (`index/indexed_client.py`); `search --health` reports `indexed`-reachable vs. substring-fallback distinctly (root `tech.md` § Risks — "`indexed` drift" now resolved). |
| Positioning review outstanding | Still outstanding — unchanged by this branch. Root `plan.md` § Open reviews still queues the adversarial audit of the "mesh" framing (self-flagged, not yet run). |
| Owner-identity trust boundary undocumented | **Resolved.** Documented in `AGENTS.md` § 6 ("Owner identity is trusted local input, not an authorization boundary"). |
| Hard delete, no soft-delete | **Evaluated, deferred — not built.** `note`/`task` delete stays a hard `unlink` by design; recorded in `AGENTS.md` § 6. |
