---
type: feature-product
feature: search
sibling: tech.md
parent: ../../product.md
updated: 2026-06-10
---

# Feature: Search — Product

The `search` verb is how knowledge is recalled. It delegates ranked retrieval to **[`indexed`](https://github.com/LennardZuendorf/indexed)** — which owns the hybrid engine (lexical + dense-vector + fusion) over the vault — and wraps it as a Brain verb: brain shapes the query, formats results as JSON with a `path` to every hit, offers a deterministic zero-cost tag pull, and falls back to a built-in substring scan when `indexed`/the daemon is unavailable. Search is what turns notes into memory.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The `brain search` command; the `indexed` client/wrapper and result-shape mapping; the deterministic tag-pull fast path; the result JSON schema; the `indexed`/daemon-down substring fallback |
| **Does not own** | The ranking engine itself — lexical + dense-vector retrieval, fusion, and any rerank live in `indexed`; the watcher and index freshness (daemon feature, which drives `indexed index update`); the note/task schemas (notes/tasks features) |

---

## Requirements

### Requirement: Hybrid retrieval across notes and tasks

The system SHALL search notes and tasks together via `indexed`'s hybrid (lexical + dense-vector) ranked retrieval, and MUST return JSON results that always include `id`, `type`, `title`, `score`, and `path`.

#### Scenario: Recall a decision

- **Given** notes and tasks exist in the vault
- **When** an agent runs `brain search "how did we handle the CLID fallback decision"`
- **Then** relevant notes and tasks are returned as ranked JSON, each with a `path`

### Requirement: Deterministic tag pull

The system SHALL support a query-less `--tags` pull that bypasses both embedding and BM25 and MUST return every matching document by frontmatter alone, reproducibly and at zero embedding cost.

#### Scenario: Tag-only pull

- **Given** notes tagged `ndc`
- **When** `brain search --tags ndc --type note --meta-only` runs
- **Then** all matching notes are returned with no bodies and no embedding call

### Requirement: Token-budgeted output

The system SHALL support `--meta-only` (drop snippet/body) and `--full` (whole bodies) and MUST apply a configurable relevance `--threshold`.

#### Scenario: Compact results for context injection

- **Given** a query with many hits
- **When** `--meta-only` is set
- **Then** results omit snippets/bodies to minimise tokens

### Requirement: Degrade to lexical-only when the daemon is down

The system SHALL return substring-scan results when `indexed` or the daemon is unavailable, MUST keep returning the same JSON result shape, and MUST print a one-line notice to stderr (suppressed under `--quiet`).

#### Scenario: Search with no engine

- **Given** the daemon is stopped (or `indexed` is not installed)
- **When** `brain search "ndc"` runs
- **Then** built-in substring-scan results are returned in the usual JSON shape and a one-line notice is printed to stderr, suppressed under `--quiet`

Reference requirements as R1, R2, R3, R4 in the feature plan's Requirements Trace.

## User Experience

```
$ brain search "CLID fallback decision"                       # hybrid, JSON by default
$ brain search --tags ndc,flights --type note --meta-only     # deterministic, zero-cost
$ brain search "ndc"                                          # daemon down: lexical-only + stderr notice
```

Results are JSON by default; `--json` is also accepted explicitly and is available on every command.

## Non-Goals

- Managing or storing vectors, or owning the ranking engine — `indexed` owns ingest, embeddings, lexical+vector retrieval, and fusion.
- A `think`/synthesis layer — brain returns ranked files; the calling agent reads and reasons.

## Prior Art & Inspiration

**Engine — [`indexed`](https://github.com/LennardZuendorf/indexed) (first-party):** Brain's own CLI/MCP indexer (FAISS, Docling + tree-sitter ingest) does the ranked retrieval; the `search` cluster is a thin wrapper. Because `indexed` is first-party, its hybrid (lexical + vector + fusion) leg and the brain↔indexed result contract are **co-designed**, not reverse-engineered.

- **Conceptual reference — [GBrain search](https://github.com/garrytan/gbrain):** the same hybrid vector + BM25 + RRF shape, validating the approach `indexed` implements. **Differ:** brain/`indexed` return ranked *files* only — no `think` synthesis, no Postgres/pgvector store.
- **Cousin — [memweave](https://towardsdatascience.com/memweave-zero-infra-ai-agent-memory-with-markdown-and-sqlite-no-vector-database-required/):** zero-infra hybrid over Markdown, validating "no vector DB to operate" as a real axis.
