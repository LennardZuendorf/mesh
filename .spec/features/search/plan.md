---
type: feature-plan
feature: search
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-10
---

# Feature: Search — Implementation Plan

Search delivers recall over notes and tasks by wrapping `indexed` (the engine) and adding the deterministic tag pull plus an always-available substring fallback. It is a closed, testable box: tag-pull and fallback work standalone, and ranked retrieval rides `indexed`. This completes the Phase-1 MVP.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Starts when **daemon** is `DONE` (root [plan.md](../../plan.md) Feature Sequence) — the daemon hosts the warm frontmatter index the wrapper resolves paths against and drives `indexed index update`. Does not depend on any other feature's units.

---

## Problem Frame

Notes are only memory once they are recallable. Brain does not build a ranking engine — `indexed` owns that. This plan builds the always-available tag-pull + substring fallback first (so recall degrades, never disappears), then the `indexed` wrapper that maps engine results into Brain's JSON, then the freshness bridge that keeps `indexed` current with the vault.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Hybrid retrieval across notes and tasks](product.md#requirement-hybrid-retrieval-across-notes-and-tasks) | search/2, search/3 |
| R2 | [Deterministic tag pull](product.md#requirement-deterministic-tag-pull) | search/1 |
| R3 | [Token-budgeted output](product.md#requirement-token-budgeted-output) | search/1, search/2 |
| R4 | [Degrade to substring-scan when the engine is down](product.md#requirement-degrade-to-lexical-only-when-the-daemon-is-down) | search/1, search/2 |

Every unit cites the R-IDs it satisfies. Do not renumber R-IDs.

---

## Key Technical Decisions

1. **Delegate ranking to `indexed`.** Brain neither embeds nor fuses; the wrapper maps `indexed`'s ranked hits into a stable Brain JSON shape, resolving frontmatter via the warm index.
2. **Tag-pull + substring are engine-free.** Recall degrades, never disappears, when `indexed`/the daemon is down.
3. **Co-designed contract.** Because `indexed` is first-party, the brain↔indexed result contract is defined jointly (see [tech.md](tech.md) Open Question 1).

---

## Unit IDs

Units are `search/n` — assigned once, never renumbered. Cite IDs in commits and tests (`feat(search): search/1 ...`).

---

### search/1 — Tag-pull, substring fallback, output schema

**Goal:** The query-less `--tags` fast path, the always-available substring scan, the JSON result schema (always `id`/`type`/`title`/`score`/`path`) with `--meta-only`/`--full`, and the engine-down stderr notice.

**Requirements:** R2, R3, R4

**Dependencies:** —

**Files:** `src/brain/index/tagpull.py`, `src/brain/index/fallback.py`, `src/brain/cli/search.py`

**Test scenarios:**

- Tag-only pull returns matches by frontmatter with no engine call.
- Results always carry `id`, `type`, `title`, `score`, `path`; `--meta-only` drops bodies.
- With `indexed`/daemon down, substring results stay JSON-shaped and a one-line stderr notice prints, suppressed under `--quiet`.

**Verification:** `uv run pytest tests/search/test_tagpull_fallback.py`

---

### search/2 — `indexed` wrapper + result mapping

**Goal:** `indexed_client` invokes `indexed index search ... --json` (or its MCP), maps each hit to Brain's schema by resolving the file `path` through the frontmatter index, passes `--limit`/`--threshold`, and applies the recency tiebreak.

**Requirements:** R1, R3, R4

**Dependencies:** search/1

**Files:** `src/brain/index/indexed_client.py`, `src/brain/cli/search.py`

**Test scenarios:**

- A query routes to `indexed`, and hits are returned in Brain's JSON shape with resolved `id`/`type`/`title`/`path`.
- When `indexed` is unavailable, the wrapper transparently falls back to search/1's substring scan.

**Verification:** `uv run pytest tests/search/test_indexed_client.py`

---

### search/3 — Freshness bridge

**Goal:** Drive `indexed index update` from the daemon's watch events (incremental) and expose `brain reindex` → full `indexed` rebuild, so the vector index tracks the vault despite `indexed`'s pull-based model.

**Requirements:** R1

**Dependencies:** search/2

**Files:** `src/brain/index/indexed_client.py` (update/rebuild calls; wired into the daemon watcher)

**Test scenarios:**

- A file create/modify/delete triggers an incremental `indexed index update`.
- `brain reindex` performs a full rebuild and subsequent searches reflect it.

**Verification:** `uv run pytest tests/search/test_freshness.py`

---

## Progress

| Unit | Status |
|---|---|
| search/1 | NOT STARTED |
| search/2 | NOT STARTED |
| search/3 | NOT STARTED |
