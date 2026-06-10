---
type: feature-product
feature: search
sibling: tech.md
parent: ../../product.md
updated: 2026-06-10
---

# Feature: Search — Product

The `search` verb is how knowledge is recalled. It runs hybrid retrieval — semantic embeddings merged with lexical BM25/substring — across notes and tasks at once, returning JSON with a `path` to every hit. It also offers a deterministic, zero-cost tag pull. Search is what turns notes into memory.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The `brain search` command; the BM25 retriever, the pluggable embedder adapter, RRF fusion + threshold + recency boost, the deterministic tag-pull fast path, and the result JSON schema |
| **Does not own** | The watcher and index storage/freshness (daemon feature); the note/task schemas (notes/tasks features); embedding storage itself (the `indexed.sh` backend) |

---

## Requirements

### Requirement: Hybrid retrieval across notes and tasks

The system SHALL search notes and tasks together, merging a semantic retriever and a lexical retriever, and MUST return JSON results that always include `id`, `type`, `title`, `score`, and `path`.

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

Reference requirements as R1, R2, R3 in the feature plan's Requirements Trace.

## User Experience

```
$ brain search "CLID fallback decision"                       # hybrid, JSON by default
$ brain search --tags ndc,flights --type note --meta-only     # deterministic, zero-cost
```

## Non-Goals

- Managing or storing vectors — `indexed.sh` owns embeddings and their storage.
- Re-ranking models beyond RRF + recency unless Phase-1/2 recall metrics prove a need.
