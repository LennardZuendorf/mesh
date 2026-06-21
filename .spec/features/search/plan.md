---
type: feature-plan
feature: search
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-21
---

# Feature: Search — Implementation Plan

Search delivers recall over notes and tasks by wrapping `indexed` (the engine) and adding the deterministic tag pull plus an always-available substring fallback. It is a closed, testable box: tag-pull and fallback work standalone, and ranked retrieval rides `indexed`. This completes the Phase-1 MVP.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Starts when **daemon** is `DONE` (root [plan.md](../../plan.md) Feature Sequence) — the daemon hosts the warm frontmatter index the wrapper resolves paths against and the `on_vault_change` hook. Does not depend on any other feature's units.

---

## Problem Frame

Notes are only memory once they are recallable. Brain does not build a ranking engine — `indexed` owns that. This plan builds the always-available tag-pull + substring fallback first (so recall degrades, never disappears), then the `indexed` wrapper that maps engine results into Brain's JSON, then `indexed_client` freshness wired into the daemon watcher hook.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Hybrid retrieval across the vault](product.md#requirement-hybrid-retrieval-across-the-vault) | search/2, search/3 |
| R2 | [Deterministic tag pull](product.md#requirement-deterministic-tag-pull) | search/1 |
| R3 | [Token-budgeted output](product.md#requirement-token-budgeted-output) | search/1, search/2 |
| R4 | [Degrade to substring-scan when hybrid is unavailable](product.md#requirement-degrade-to-substring-scan-when-hybrid-is-unavailable) | search/1, search/2 |

Every unit cites the R-IDs it satisfies. Do not renumber R-IDs.

---

## Key Technical Decisions

1. **Delegate ranking to `indexed`.** Brain neither embeds nor fuses; the wrapper maps hits into a stable Brain JSON shape.
2. **Tag-pull + substring are engine-free.** Recall degrades, never disappears, per the degradation matrix.
3. **Freshness via hook registration.** `indexed_client` registers on daemon `on_vault_change`; daemon does not import ranking logic.

---

## Unit IDs

Units are `search/n` — assigned once, never renumbered. Cite IDs in commits and tests (`feat(search): search/1 ...`).

---

### search/1 — Tag-pull, substring fallback, output schema

**Goal:** The query-less `--tags` fast path, the always-available substring scan (with pinned scoring), the JSON result schema (always `id`/`type`/`title`/`score`/`path`) with `--meta-only`/`--full`/`--threshold`, and the engine-down stderr notice.

**Requirements:** R2, R3, R4

**Dependencies:** —

**Files:** `src/brain/index/tagpull.py`, `src/brain/index/fallback.py`, `src/brain/cli/search.py`

**Test scenarios:**

- Tag-only pull returns matches by frontmatter with no engine call.
- Results always carry `id`, `type`, `title`, `score`, `path`; `--meta-only` drops bodies; `--threshold` filters.
- With daemon down or `indexed` absent, substring results stay JSON-shaped and a one-line stderr notice prints, suppressed under `--quiet`.

**Verification:** `uv run pytest tests/search/test_tagpull_fallback.py`

---

### search/2 — `indexed` wrapper + result mapping

**Goal:** `indexed_client` invokes `indexed index search ... --json` per pinned contract, maps each hit to Brain's schema by resolving `path` through the warm index, passes `--limit`/`--threshold`, and applies the recency tiebreak.

**Requirements:** R1, R3, R4

**Dependencies:** search/1

**Files:** `src/brain/index/indexed_client.py`, `src/brain/cli/search.py`

**Test scenarios:**

- A query routes to `indexed` (mocked), and hits are returned in Brain's JSON shape with resolved `id`/`type`/`title`/`path`.
- Foreign vault files (no brain `id`) appear with `id: null`.
- When `indexed` is unavailable, the wrapper transparently falls back to search/1's substring scan.

**Verification:** `uv run pytest tests/search/test_indexed_client.py`

---

### search/3 — `indexed_client` freshness

**Goal:** Implement `incremental_update(path)` and `full_rebuild()`; register `incremental_update` on the daemon watcher's `on_vault_change` hook.

**Requirements:** R1

**Dependencies:** search/2 (registers on the daemon watcher hook — satisfied because **daemon** is `DONE` before this feature starts)

**Files:** `src/brain/index/indexed_client.py`

**Test scenarios:**

- `on_vault_change` triggers `indexed index update` for the affected path (mocked `indexed` CLI).
- `brain reindex` calls `full_rebuild()` and subsequent hybrid searches reflect new content.

**Verification:** `uv run pytest tests/search/test_freshness.py`

---

## DONE

All three units pass; `brain search` returns hybrid results when daemon + `indexed` are up, substring-scan results when degraded, and tag-pull works with no engine; `indexed_client` registers on the watcher hook.

## Progress

| Unit | Status |
|---|---|
| search/1 | NOT STARTED |
| search/2 | NOT STARTED |
| search/3 | NOT STARTED |
