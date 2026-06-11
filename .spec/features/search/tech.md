---
type: feature-tech
feature: search
sibling: product.md
parent: ../../tech.md
updated: 2026-06-10
---

# Feature: Search — Architecture

`brain search` delegates ranked retrieval to **[`indexed`](https://github.com/LennardZuendorf/indexed)** (first-party: FAISS + Docling/tree-sitter ingest, hybrid lexical+vector retrieval, CLI and MCP). This feature owns the *wrapper*: query shaping, mapping `indexed`'s results into Brain's JSON, the deterministic tag-pull fast path, and a built-in substring fallback when `indexed`/the daemon is unavailable. The index host and freshness belong to the daemon feature, which also drives `indexed index update`.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/brain/cli/search.py            # brain search (+ query-less tag pull)
src/brain/index/indexed_client.py  # call `indexed` (CLI/MCP); map results to Brain's shape
src/brain/index/tagpull.py         # deterministic frontmatter tag pull (no engine)
src/brain/index/fallback.py        # substring scan when indexed/daemon is down
```

---

## Contract / API

Command: `search "<query>" [--type note|task] [--tags] [--owner] [--status] [--limit 10] [--threshold 0.0-1.0] [--meta-only] [--full]`. The `"<query>"` is optional when `--tags` is given (deterministic tag pull). Results are JSON by default; `--json` is also accepted explicitly.

Brain result schema (the wrapper's output, stable regardless of engine):

```json
[
  {"id":"n-a3f2","type":"note","title":"NDC config for Lufthansa",
   "score":0.91,"tags":["ndc","lufthansa","flights"],"owner":"lennard",
   "updated":"2026-06-05T14:01:00Z",
   "snippet":"…CLID provides better seat-map coverage than NDC on LH metal…",
   "path":"/tolaria/notes/decisions/n-a3f2.md"}
]
```

`path` is always returned. `--meta-only` drops `snippet`/body; `--full` returns whole bodies.

**brain ↔ `indexed` contract (co-designed, first-party):** `indexed_client` invokes `indexed index search "<query>" --collection <vault> --json` (or the equivalent MCP call) and maps each hit to the schema above. Required from `indexed` per hit: a **file path** (→ `path`, from which brain resolves `id`/`type`/`title`/`tags`/`owner`/`updated` via the frontmatter index) and a **relevance score** (→ `score`); optional **snippet**. `--limit` and `--threshold` pass through. Exact `indexed` flag/field names are finalized against `indexed index search --help` (see Open Questions) — because `indexed` is first-party, the contract is defined jointly, not reverse-engineered.

## Implementation Detail

- **Delegated retrieval.** The hybrid engine (lexical + dense-vector + any fusion/rerank) lives entirely in `indexed`; brain neither ranks nor embeds. `indexed_client` shells out (or uses `indexed`'s MCP) and re-shapes results — `score` from the engine drives ordering; brain may apply a light **recency tiebreak** on `updated` among near-equal scores.
- **Tag-pull fast path.** `--tags` with no query bypasses `indexed` entirely and returns matching documents by frontmatter alone — zero embedding cost, fully reproducible, always available.
- **Freshness.** `indexed` is pull-based (`indexed index update`, git-tracked incremental). The daemon's watcher triggers `indexed index update` on create/modify/delete so the vector index tracks the vault; `brain reindex` forces a full `indexed index create`/rebuild.
- **Degradation.** When `indexed` is absent or the daemon is down, `search` runs the built-in **substring scan** (`fallback.py`) over title/tags/body, keeps the same JSON shape, and prints a one-line stderr notice (suppressed under `--quiet`). `[search].hybrid=false` forces this lexical-only path even when `indexed` is available (minus the notice).

## Performance Budget

- < 50 ms tag-pull / substring fallback; engine-bound for hybrid (target < 200 ms against a warm `indexed` collection for a few-thousand-doc vault).

## Open Questions

1. **`indexed index search` interface (first-party — co-define, not blocking).** Finalize the exact flag names (`--json`, result-limit, score/threshold) and the per-hit JSON field names (path, score, snippet) against `indexed index search --help`, plus whether `indexed` exposes its lexical leg or brain treats it as vector-primary. Owner maintains `indexed`, so this is a contract decision, not an external unknown.
2. **Tag-pull without a query.** Confirmed: `brain search --tags ndc --meta-only` (no positional query) is the canonical deterministic pull — query optional when `--tags` is present.
