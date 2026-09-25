---
type: feature-plan
feature: note-adoption
sibling: tech.md
parent: ../../plan.md
updated: 2026-09-25
---

# Feature: Note Adoption & Claims — Implementation Plan

Groundwork first: the read model widens (mesh-native = frontmatter id, foreign types opaque)
before any new verb exists, so adoption lands on a model that can already see what it produces.
Then the verb, then ownership, then claims, then the surfaces that report them.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** none — this is the root of the round-2 bundle; [project-envelope](../project-envelope/plan.md) waits on it.

---

## Problem Frame

655 of the brain vault's 656 files are invisible to every list verb because `Note::from_meta`
rejects foreign `type` values and id resolution is stem-shaped. The units below decouple
addressability from the namer's filenames first — without that, adoption would mint ids into
files that still don't list.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Minimal, idempotent adoption](product.md#requirement-minimal-idempotent-adoption) | note-adoption/2, note-adoption/7 |
| R2 | [Foreign type values stay opaque](product.md#requirement-foreign-type-values-stay-opaque) | note-adoption/1 |
| R3 | [Adoption is in place](product.md#requirement-adoption-is-in-place) | note-adoption/1 |
| R4 | [Batch adoption is a sequence of single-entity transactions](product.md#requirement-batch-adoption-is-a-sequence-of-single-entity-transactions) | note-adoption/2 |
| R5 | [Durable ownership is area, not assignment](product.md#requirement-durable-ownership-is-area-not-assignment) | note-adoption/3 |
| R6 | [Atomic note claims](product.md#requirement-atomic-note-claims) | note-adoption/4, note-adoption/7 |
| R7 | [Idempotent release](product.md#requirement-idempotent-release) | note-adoption/4, note-adoption/7 |
| R8 | [Claim visibility on lists](product.md#requirement-claim-visibility-on-lists) | note-adoption/4 |
| R9 | [Status census sees note ownership and claims](product.md#requirement-status-census-sees-note-ownership-and-claims) | note-adoption/5 |
| R10 | [MCP parity for the new verbs](product.md#requirement-mcp-parity-for-the-new-verbs) | note-adoption/6 |

---

## Key Technical Decisions

1. **Read model before verbs.** Units 1 ships tolerance + meta-id resolution on its own, pinned
   by tests against today's foreign fixtures. → [tech.md](tech.md) § Read-model widening
2. **In-place, no copy-in, no rename.** Adopted files keep paths and stems; the `exists` check
   for id minting reads frontmatter ids, not stems. → [tech.md](tech.md) § Adoption
3. **Claim envelope parity over invention.** A note claim conflict is byte-identical in shape to
   a task claim conflict; no `retry_after_ms`. → [tech.md](tech.md) § Claims

---

### note-adoption/1 — Read-model widening

**Goal:** A foreign-stemmed, id-bearing file with a foreign `type` lists, resolves by id, and
counts as mesh-native — before any adopt verb exists.

**Requirements:** R2, R3

**Dependencies:** —

**Files:** `src/model/note.rs`, `src/domain/notes.rs` (resolve, `foreign_count`,
classification), `src/storage/walk.rs` (classification input only)

**Test scenarios:** foreign-type fixture lists and resolves by meta id; `status` foreign count
excludes id-bearing files; delete by id works on an adopted-shaped fixture; opaque type filters
(`list --type Team` raw equality)

**Verification:** `cargo test --test note_cli` + `--test admin_cli` + `--test lens_cli` green
with new fixtures; `mesh note list` shows a hand-written id-bearing foreign-type file.

---

### note-adoption/2 — The adopt verb

**Goal:** `mesh note adopt <PATH>... [--owner]` with the full discrimination and safety core.

**Requirements:** R1, R4

**Dependencies:** note-adoption/1

**Files:** `src/domain/notes.rs` (`adopt`), `src/cli/note.rs`, `src/cli/mod.rs`

**Test scenarios:** rich-frontmatter foreign file adopts with only absent keys injected;
re-adoption is a byte-identical no-op; frontmatter-less file gains a block; malformed block →
exit 3 (never body-ified); nested-space-root path → exit 2; outside-sandbox → exit 2; missing →
exit 3; foreign `id` key → exit 2; batch stops at first failure, names committed files, heals
on re-run; id collision extends

**Verification:** `cargo test --test note_cli`; manual: adopt one real brain-vault file in a
copy, `note get` by id, git diff shows only frontmatter keys added.

---

### note-adoption/3 — Ownership at adoption and by update

**Goal:** `--owner` on adopt (insert-when-absent) and `--owner` on `note update` (explicit set).

**Requirements:** R5

**Dependencies:** note-adoption/2

**Files:** `src/domain/notes.rs`, `src/cli/note.rs` (update flags)

**Test scenarios:** owner inserted when absent + flag given; never inserted without the flag;
existing owner untouched by adopt; `update --owner` changes it; identity validated
(`[tasks].collections` roster); empty owner clears/absent per the identity rule

**Verification:** `cargo test --test note_cli`; roster rejection pinned (`unknown owner` exit 2).

---

### note-adoption/4 — Claims, release, mine

**Goal:** `mesh note claim/release <ID>` with task-grade atomicity, and `--mine` on note list.

**Requirements:** R6, R7, R8

**Dependencies:** note-adoption/2

**Files:** `src/domain/notes.rs` (`claim`, `release`), `src/cli/note.rs`, `src/model/note.rs`
(`claimed_by` field order)

**Test scenarios:** claim writes `claimed_by` only (no status, no owner change); second identity
→ exit 4 `claim_conflict` envelope, same shape as tasks; same-identity re-claim no-op, byte-identical; release idempotent (unclaimed → report, no write); `--force` overrides another holder;
`--mine` lists owner-or-claimed_by

**Verification:** `cargo test --test note_cli`; envelope shape diffed against the task claim
envelope in a review-regression test.

---

### note-adoption/5 — Status census extension

**Goal:** Per-agent note ownership/claims in `mesh status`, payload keys appended last.

**Requirements:** R9

**Dependencies:** note-adoption/4

**Files:** `src/domain/lenses.rs`, `src/cli/admin.rs`

**Test scenarios:** census shows owned/claimed notes per agent; pinned key-order test extends
with the new keys last; zero-note agent unchanged

**Verification:** `cargo test --test admin_cli --test lens_cli` (key-order pin green).

---

### note-adoption/6 — MCP parity

**Goal:** `mesh_note_adopt` / `mesh_note_claim` / `mesh_note_release` with annotations; count
37 → 40; SKILL playbook updated.

**Requirements:** R10

**Dependencies:** note-adoption/2, note-adoption/3, note-adoption/4

**Files:** `src/mcp/{mod,schema,tools}.rs`, `plugins/mesh/skills/mesh/SKILL.md`

**Test scenarios:** tool count 40 in both pins; adopt idempotent / claim write / release
idempotent annotations; adopt over MCP on a fixture vault; SKILL bundle invariants

**Verification:** `cargo test --test mcp_cli --test bundle`.

---

### note-adoption/7 — Race and regression hardening

**Goal:** Multi-process claim races and byte-for-byte no-op pins in the suites that guard them.

**Requirements:** R1, R6, R7

**Dependencies:** note-adoption/4

**Files:** `tests/race.rs`, `tests/review_regressions.rs`

**Test scenarios:** 8-way real-process note claim race → exactly one winner (mirrors the task
race); concurrent claim/release CAS safety; no-op adopt/claim/release never rewrite
(byte-for-byte)

**Verification:** `cargo test --test race --test review_regressions`; break the fix and watch
the race go red, then restore (lesson: a green race test proves nothing until seen red).

---

## Progress

| Unit | Status |
|---|---|
| note-adoption/1 | NOT STARTED |
| note-adoption/2 | NOT STARTED |
| note-adoption/3 | NOT STARTED |
| note-adoption/4 | NOT STARTED |
| note-adoption/5 | NOT STARTED |
| note-adoption/6 | NOT STARTED |
| note-adoption/7 | NOT STARTED |
