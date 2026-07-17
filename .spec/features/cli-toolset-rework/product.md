---
type: feature-product
feature: cli-toolset-rework
sibling: tech.md
parent: ../../product.md
updated: 2026-07-17
---

# Feature: CLI Toolset Rework — Product

Scopes the reworked shards toolset after evaluating and rejecting two off-the-shelf
replacements (GBrain, Beads). shards stays the Markdown-native, no-database substrate and
gets: internal tidying, a dedicated performance push, and two small additive capabilities
(graph-query output; projects as a supported convention). The task-dependency graph (Phase 3)
is designed here but stays deferred and gated behind the rest.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | Graph-query output (first-class BFS-over-`related` query); projects-as-convention (`type: project` note, `project:` field on tasks, project-scoped views); the decision record for keep-vs-replace; the performance goal statement; the deferred task-graph (Phase 3) design. |
| **Does not own** | Internal refactor mechanics (module extraction/decomposition — tech-only, no product surface); performance optimization tactics (pending parallel research, tracked as an open question); the positioning/overclaiming review itself (already queued in root [plan.md](../../plan.md) § Open reviews — this feature only flags it as a gap); CI tooling. |
| **Deferred** | Executing the task-graph (Phase 3) build — design only this branch, gated on this feature's own units shipping first. |

---

## Decision

Evaluated replacing shards outright with two candidates; rejected both, kept shards:

| Candidate | Verdict | Why rejected |
|---|---|---|
| [GBrain](https://github.com/garrytan/gbrain) | Rejected | Mandates Postgres/PGLite — contradicts no-database. No task primitive. |
| Beads | Rejected | Moved its source of truth to Dolt (a SQL DB) — contradicts "Markdown is the source of truth." |
| shards (keep + rework) | **Chosen** | Markdown/git stays the source of truth; no DB to operate; CLI + MCP both stay; existing daemon + hybrid-search architecture is optimized, not restructured. |

## Product framing

A fleet of agents (e.g. Personal-Assistant, Product, Revenue, Jira) shares **one** folder. Each
agent: (a) records its own memories as notes, (b) maintains its own projects, (c) claims tasks —
all concurrently, without stepping on each other. The concurrency safety already built (atomic
writes, `O_EXCL` claims, unknown-frontmatter-key round-tripping) is precisely what makes shared
concurrent editing safe — this feature leans on those invariants rather than adding new ones.

---

## Requirements

### Requirement: Graph-query output

The system SHALL answer "what's connected to X" directly from the `related` wikilink graph,
without going through hybrid search.

#### Scenario: Query returns a connected-content graph

- **Given** a note or task with `related` links, possibly several hops deep
- **When** an agent asks a graph query for that id
- **Then** the response includes every reachable id via BFS over `related`, deduped for cycles/diamonds, available as both JSON and a readable tree — the same traversal `build-context` already performs, promoted to a first-class answer

### Requirement: Projects as a supported convention

The system SHALL support a `type: project` note that groups tasks via a `project:` field on
task frontmatter, without introducing a fourth verb.

#### Scenario: Project-scoped task view

- **Given** a project note and several tasks carrying its id in `project:`
- **When** an agent lists tasks scoped to that project (e.g. `task list --project <id>`)
- **Then** only that project's tasks are returned, and the convention remains additive frontmatter — no new CLI verb, no schema requirement forcing every task to carry `project:`

#### Scenario: Convention, not a verb — yet

- **Given** the three-verb thesis (`note`, `task`, `search`)
- **When** projects are introduced
- **Then** they ship as a note type + task field + scoped view, explicitly structured so a future `project` verb could graduate from it if usage earns that — no such verb ships this branch

---

## Non-Goals

- No `project` verb this branch — projects are a convention (note type + frontmatter field + scoped view) only.
- No execution of the Phase-3 task-graph build — design captured in [tech.md](tech.md) / [plan.md](plan.md), gated behind this feature's own units.
- No resolution of the mesh-positioning/overclaiming review — that review is already queued in root [plan.md](../../plan.md) § Open reviews; this feature only surfaces it as a gap to close.
- No decision on rewriting the CLI in Rust — recorded as an open question in [tech.md](tech.md) § Open Questions, not resolved here.

---

## Open Questions

1. **Rust rewrite for CLI startup performance** — under active evaluation via parallel performance research. Runtime stays Python 3.11+ for now (keep-and-optimize). See [tech.md](tech.md) § Open Questions.
