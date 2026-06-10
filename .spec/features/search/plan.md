---
type: feature-plan
feature: search
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-10
---

# Feature: Search — Implementation Plan

Search delivers hybrid recall over notes and tasks plus the deterministic tag pull. It is a closed, testable box: lexical search works standalone, and the semantic half plugs into the daemon's embedding cache. This completes the Phase-1 MVP.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Starts when **daemon** is `DONE` (root [plan.md](../../plan.md) Feature Sequence) — semantic search rides the daemon's index/cache, and the lexical path reuses its fallback. Does not depend on any other feature's units.

---

## Problem Frame

Notes are only memory once they are recallable. This plan builds the always-available lexical retriever and the deterministic tag pull first, then the embedder adapter, then RRF fusion with threshold and recency — so each scoring layer is testable before the next is added.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Hybrid retrieval across notes and tasks](product.md#requirement-hybrid-retrieval-across-notes-and-tasks) | search/1, search/2, search/3 |
| R2 | [Deterministic tag pull](product.md#requirement-deterministic-tag-pull) | search/1 |
| R3 | [Token-budgeted output](product.md#requirement-token-budgeted-output) | search/1, search/3 |
| R4 | [Degrade to lexical-only when the daemon is down](product.md#requirement-degrade-to-lexical-only-when-the-daemon-is-down) | search/1, search/2 |

Every unit cites the R-IDs it satisfies. Do not renumber R-IDs.

---

## Key Technical Decisions

1. **RRF over score normalisation.** Fuse ranks, not incomparable BM25/cosine magnitudes.
2. **Lexical + tag-pull are daemon-free.** Recall degrades, never disappears, when the daemon is down.

---

## Unit IDs

Units are `search/n` — assigned once, never renumbered. Cite IDs in commits and tests (`feat(search): search/1 ...`).

---

### search/1 — Lexical retriever, tag-pull, output schema

**Goal:** In-process BM25/substring, the query-less `--tags` fast path, the JSON result schema (always `id`/`type`/`title`/`score`/`path`) with `--meta-only`/`--full`, and the daemon-down stderr notice.

**Requirements:** R1, R2, R3, R4

**Dependencies:** —

**Files:**

```
src/brain/index/bm25.py
src/brain/cli/search.py
```

**Test scenarios:**

- Tag-only pull returns matches by frontmatter with no embedding call.
- Results always carry `id`, `type`, `title`, `score`, `path`; `--meta-only` drops bodies.
- With the daemon down, results stay JSON-shaped and a one-line stderr notice prints, suppressed under `--quiet`.

**Verification:** `uv run pytest tests/search/test_lexical_tagpull.py`

---

### search/2 — Embedder adapter

**Goal:** Pluggable `embed(texts) -> vectors` with the `indexed` backend and a BM25-only fallback when unavailable.

**Requirements:** R1, R4

**Dependencies:** search/1

**Files:**

```
src/brain/index/embedder.py
```

**Test scenarios:**

- With the backend present, queries produce vectors; absent, search falls back to lexical only.

**Verification:** `uv run pytest tests/search/test_embedder.py`

---

### search/3 — RRF fusion + threshold + recency

**Goal:** Merge retrievers via RRF, apply `--threshold`, and add the recency boost.

**Requirements:** R1, R3

**Dependencies:** search/2

**Files:**

```
src/brain/index/fusion.py
src/brain/cli/search.py
```

**Test scenarios:**

- Fused ranking beats either retriever alone on a benchmark query set.
- `--threshold` filters low-relevance hits; recency lifts fresh near-ties.

**Verification:** `uv run pytest tests/search/test_fusion.py`

---

## Progress

| Unit | Status |
|---|---|
| search/1 | NOT STARTED |
| search/2 | NOT STARTED |
| search/3 | NOT STARTED |
