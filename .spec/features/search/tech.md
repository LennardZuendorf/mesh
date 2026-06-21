---
type: feature-tech
feature: search
sibling: product.md
parent: ../../tech.md
updated: 2026-06-21
---

# Feature: Search — Architecture

`brain search` delegates ranked retrieval to **[`indexed`](https://github.com/LennardZuendorf/indexed)** (first-party: FAISS + Docling/tree-sitter ingest, hybrid lexical+vector retrieval, CLI and MCP). This feature owns the *wrapper*: query shaping, mapping `indexed`'s results into Brain's JSON, the deterministic tag-pull fast path, the built-in substring fallback, and the `indexed_client` that implements incremental/full `indexed` updates. The daemon watcher owns the hook point; this feature registers `on_vault_change`.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/brain/cli/search.py            # brain search (+ query-less tag pull)
src/brain/index/indexed_client.py  # call `indexed` (CLI/MCP); map results; incremental/full rebuild
src/brain/index/tagpull.py         # deterministic frontmatter tag pull (no engine)
src/brain/index/fallback.py        # substring scan when hybrid unavailable
```

---

## Contract / API

Command: `search "<query>" [--type note|task] [--tags] [--owner] [--status] [--limit 10] [--threshold 0.0-1.0] [--meta-only] [--full]`. The `"<query>"` is optional when `--tags` is given (deterministic tag pull). Results are JSON by default; `--json` is also accepted explicitly.

### Search corpus

Hybrid and fallback search scan **all** `*.md` files under `notes/` and `tasks/` in `tolaria_path`, regardless of whether the file carries a brain `id`. This differs from `brain note list`, which surfaces only brain-authored notes. Foreign files (no brain `id`) appear in search results with `id: null` and `type` inferred from path (`note` vs `task`).

### Brain result schema (stable wrapper output)

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

### brain ↔ `indexed` contract (pinned)

**Invocation:**

```bash
indexed index search "<query>" --collection <vault> --json --limit <N>
```

`<vault>` comes from `[search].collection` in config (defaults to `tolaria_path` basename).

**Per-hit JSON from `indexed` (required fields):**

```json
{"path": "notes/decisions/n-a3f2.md", "score": 0.91, "snippet": "optional excerpt"}
```

`path` may be relative to `tolaria_path` or absolute; brain normalizes. `score` is `0.0–1.0` relevance. `snippet` is optional.

**Mapping:** `indexed_client` resolves each `path` through the warm frontmatter index (or an on-the-fly parse when enriching a single hit) to populate `id`, `type`, `title`, `tags`, `owner`, `updated`. `--limit` and `--threshold` pass through. Brain applies a light **recency tiebreak** on `updated` when scores differ by less than `0.02`.

**Freshness calls:**

```bash
indexed index update <path> --collection <vault>    # incremental (watcher callback)
indexed index create <tolaria_path> --collection <vault>  # full rebuild (`brain reindex`)
```

### Degradation matrix

| Condition | Mode | stderr notice |
|---|---|---|
| Daemon up + `indexed` installed + `[search].hybrid=true` | Hybrid via `indexed` + warm enrichment | No |
| Daemon down | Substring scan (`fallback.py`) | Yes (unless `--quiet`) |
| `indexed` not installed / not on PATH | Substring scan | Yes |
| `[search].hybrid=false` | Substring scan (tag-pull still uses warm index or scan) | Yes |

**Rationale for daemon-down → substring:** warm path→frontmatter enrichment and the watcher-driven freshness bridge are unavailable; running `indexed` without enrichment would return hits missing stable `id`/`type` fields.

### Fallback scoring

Substring scan assigns `score` as: `1.0` title exact match (case-insensitive) · `0.8` title substring · `0.6` tag match · `0.4` body substring; multiple matches take the max. Results sort by `score` desc, then `updated` desc.

## Implementation Detail

- **Delegated retrieval.** The hybrid engine lives entirely in `indexed`; brain neither ranks nor embeds.
- **Tag-pull fast path.** `--tags` with no query bypasses `indexed` entirely — frontmatter scan, zero embedding cost.
- **`indexed_client`.** Implements `incremental_update(path)`, `full_rebuild()`, and registers `incremental_update` on the daemon watcher's `on_vault_change` hook (search/3). `brain reindex` (daemon admin) calls `full_rebuild()`.

## Performance Budget

- < 50 ms tag-pull / substring fallback; engine-bound for hybrid (target < 200 ms against a warm `indexed` collection for a few-thousand-doc vault).

## Open Questions

None — contract pinned above; adjust jointly with `indexed` if `indexed index search --help` diverges.
