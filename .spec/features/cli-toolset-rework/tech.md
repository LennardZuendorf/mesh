---
type: feature-tech
feature: cli-toolset-rework
sibling: product.md
parent: ../../tech.md
updated: 2026-07-17
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

1. Baseline cold-start of `shards --help`, `note new`, `task claim` before touching anything.
2. Extend the lazy-import discipline already applied to `watchdog` (see root `lessons.md` §
   "Instant CLI: import heavy and daemon-only deps lazily") to every heavy dependency still on
   the CLI import path: `pydantic`, `FastMCP`, `python-frontmatter`.
3. Trim dependencies where the lazy-import audit finds an import that a given command path never
   needs.
4. Add a startup-time regression guard to CI (workstream gaps, below) so a re-introduced eager
   import fails the build, not just a future profiling pass.

**Detailed optimization tactics: pending performance research (this branch)** — the exact
techniques (which imports move, in what order, what the trimmed dependency set looks like) are
being researched in parallel; this plan states the target and the mechanism, not yet the tactics.

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
| No CI | Add `.github/workflows` (none exists today) running `pytest` + `mypy` + `ruff` + the workstream-B startup-time guard + a check that the spec's stated test count matches actual. |
| Spec drift | Root spec said 578 tests; actual is 591 — fixed as part of this branch (root `tech.md`, `plan.md`). |
| `indexed` contract unpinned | Pin the `indexed` NDJSON contract with a shared schema; add a `search --health` / status signal so silent degradation (root `tech.md` § Risks — "`indexed` drift") is visible instead of silent. |
| Positioning review outstanding | Root `plan.md` § Open reviews already queues an adversarial audit of the "mesh" framing (self-flagged, not yet run). This feature does not resolve it — flagged again here so it isn't lost among the rework. |
| Owner-identity trust boundary undocumented | `$SHARDS_AGENT` / `--owner` is currently trusted input with no documented boundary — write down what is and isn't verified. |
| Hard delete, no soft-delete | `note`/`task` delete is a hard `unlink` today; consider an optional soft-delete (trash) instead. |

---

## Open Questions

1. **Rust rewrite for CLI startup performance** — under active evaluation via parallel
   performance research (outside this branch). Not decided here: runtime stays **Python 3.11+**
   for now, keep-and-optimize (workstream B). If the parallel research finds Python's cold-start
   floor insufficient even after the lazy-import work, a Rust rewrite becomes a future spec
   decision — not assumed, not started, not scheduled by this branch.
