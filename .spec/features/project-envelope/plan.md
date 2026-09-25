---
type: feature-plan
feature: project-envelope
sibling: tech.md
parent: ../../plan.md
updated: 2026-09-25
---

# Feature: Project Envelope — Implementation Plan

Small model change first (the memory `project` field), then the lens that reads it, then the two
filters, then MCP parity. Every unit is read-time composition — no verb gains a write it didn't
have.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** starts when [note-adoption](../note-adoption/plan.md) is `DONE` (root
[plan.md](../../plan.md) Feature Sequence) — the envelope hangs off real ids. No unit-level
cross-feature dependencies.

---

## Problem Frame

The project lens today reads one note plus a raw-equality task filter. The envelope needs a
containment rule for notes (`related`) and a membership field for memories — both read-time —
plus a resolved membership set for `--project` that behaves as an active filter on both search
branches.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Envelope membership has one rule per space](product.md#requirement-envelope-membership-has-one-rule-per-space) | project-envelope/1, project-envelope/2 |
| R2 | [Lens output extends by append only](product.md#requirement-lens-output-extends-by-append-only) | project-envelope/2 |
| R3 | [`--project` scopes search](product.md#requirement---project-scopes-search) | project-envelope/3 |
| R4 | [`--project` scopes memory recall](product.md#requirement---project-scopes-memory-recall) | project-envelope/4 |
| R5 | [MCP parity](product.md#requirement-mcp-parity) | project-envelope/5 |
| R6 | [The seed gate is unchanged](product.md#requirement-the-seed-gate-is-unchanged) | project-envelope/3 |

---

### project-envelope/1 — Memory `project` field

**Goal:** Memories accept an optional raw-string `project` (create/update/list), declared after
`scope`; scope enum untouched.

**Requirements:** R1

**Dependencies:** —

**Files:** `src/model/memory.rs`, `src/domain/memories.rs`, `src/cli/memory.rs`

**Test scenarios:** create with `--project n-P1` round-trips; update changes it; absent stays
absent; unknown value tolerated (no existence validation, matching tasks); scope validation
unchanged (`invalid scope` still exits 2 for bad scopes)

**Verification:** `cargo test --test memory_cli`.

---

### project-envelope/2 — Lens extension

**Goal:** `mesh project` returns notes (related-containment) and memories (project equality)
after the existing sections; `--space` narrows each.

**Requirements:** R1, R2

**Dependencies:** project-envelope/1

**Files:** `src/domain/lenses.rs`, `src/cli/lens.rs` (rendering only)

**Test scenarios:** wikilinked note joins the envelope; `project`-tagged memory joins; sections
appended last with key-order pin extended; zero members → empty sections, never an error;
`--space notes` drops the memories section

**Verification:** `cargo test --test lens_cli` (key-order pin green).

---

### project-envelope/3 — `search --project`

**Goal:** Membership-set filter on both engine branches, resolved before any search I/O.

**Requirements:** R3, R6

**Dependencies:** project-envelope/1

**Files:** `src/search/{mod,builtin,indexed}.rs`, `src/cli/search.rs`

**Test scenarios:** scoped search returns only members on the built-in branch; the indexed stub
fixture pins the unbounded-fetch + post-filter + display-cap pattern; unknown project → exit 3
`not_found` with candidates; foreign file as project → exit 3 `seed is not mesh-native`;
composition with `--tags`/`--status` conjunction

**Verification:** `cargo test --test search_cli`.

---

### project-envelope/4 — `memory recall --project`

**Goal:** Recall filters by memory `project`, composing with existing filters.

**Requirements:** R4

**Dependencies:** project-envelope/1

**Files:** `src/domain/memories.rs`, `src/cli/memory.rs`

**Test scenarios:** only project memories eligible; composes with kind/scope filters; unknown
project → exit 3

**Verification:** `cargo test --test memory_cli`.

---

### project-envelope/5 — MCP parity

**Goal:** `project` params on search/recall/memory tools; `mesh_project` extended via the same
domain function; count unchanged (40 from note-adoption).

**Requirements:** R5

**Dependencies:** project-envelope/2, project-envelope/3, project-envelope/4

**Files:** `src/mcp/{schema,tools}.rs`

**Test scenarios:** MCP recall with project matches CLI; MCP search scoped identically; tool
count still pinned; schema enum/values generated from domain constants

**Verification:** `cargo test --test mcp_cli`.

---

## Progress

| Unit | Status |
|---|---|
| project-envelope/1 | NOT STARTED |
| project-envelope/2 | NOT STARTED |
| project-envelope/3 | NOT STARTED |
| project-envelope/4 | NOT STARTED |
| project-envelope/5 | NOT STARTED |
