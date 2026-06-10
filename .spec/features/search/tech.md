---
type: feature-tech
feature: search
sibling: product.md
parent: ../../tech.md
updated: 2026-06-10
---

# Feature: Search — Architecture

`brain search` runs two independent retrievers over the same corpus (notes *and* tasks) and fuses them with Reciprocal Rank Fusion. The lexical retriever is in-process and always available (so search works with no daemon); the semantic retriever uses the pluggable embedder, accelerated by the daemon's embedding cache. The index host and freshness belong to the daemon feature; this feature owns scoring.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/brain/cli/search.py      # brain search
src/brain/index/bm25.py      # lexical retriever (title, tags, body)
src/brain/index/embedder.py  # pluggable adapter: indexed | openai | local
src/brain/index/fusion.py    # RRF merge, threshold, recency boost
```

---

## Contract / API

Command: `search "<query>" [--type note|task] [--tags] [--owner] [--status] [--limit 10] [--threshold 0.0-1.0] [--meta-only] [--full]`. The `"<query>"` is optional when `--tags` is given (deterministic tag pull). Results are JSON by default; `--json` is also accepted explicitly.

Result schema:

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

## Implementation Detail

- **Two retrievers.** (1) BM25/substring over title, tags, body — always in-process, even daemon-down. (2) Semantic: the configured embedder embeds the query; cosine similarity against cached document vectors supplied by the backend.
- **RRF merge.** `score(d) = Σ 1/(k + rank_i(d))` over each retriever `i` (`k≈60`), avoiding normalisation of incomparable BM25 and cosine scores. The fused list is filtered by `--threshold` (default `0.65` from config) and may receive a **recency boost** weighted by `updated` so fresh docs float up among near-ties.
- **Tag-pull fast path.** `--tags` with no query skips both retrievers entirely and returns matching documents by frontmatter alone — zero embedding cost, fully reproducible.
- **Daemon-down degradation.** When the daemon or embedder is unavailable, only the lexical retriever runs; results keep the same JSON shape and a one-line notice is printed to stderr (suppressed under `--quiet`). The connect-then-fallback path itself is owned by the daemon feature.
- **Embedder pluggability.** `[search].embedder` selects `indexed | openai | local` behind one adapter interface (`embed(texts) -> vectors`). Switching embedders requires only a `brain reindex` (the `reindex` command is owned by the daemon feature).

## Performance Budget

- < 50 ms lexical-only; < 200 ms hybrid against a warm daemon (few-thousand-doc vault).

## Open Questions

1. **indexed.sh interface.** Exact query/output format unknown. *Default:* wrap behind the `indexed` adapter; fall back to BM25-only if it cannot produce document embeddings.
2. **Tag-pull without a query.** Confirm `brain search --tags ndc --meta-only` (no positional query) is the canonical deterministic pull. *Default:* yes — query optional when `--tags` present.
