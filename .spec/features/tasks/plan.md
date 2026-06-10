---
type: feature-plan
feature: tasks
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-10
---

# Feature: Tasks — Implementation Plan

Tasks delivers coordination: lifecycle, atomic claim, idempotent finish, and dependency-driven readiness. It is a closed, testable box that works over the vault with no daemon. Concurrency correctness is the acceptance bar.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Starts when **notes** is `DONE` (root [plan.md](../../plan.md) Feature Sequence) — it reuses the note schema and storage primitives. Does not depend on any other feature's units.

---

## Problem Frame

Once memory exists, agents need to pass work without a human relay. This plan adds the task schema and lifecycle, then the two correctness-critical operations — atomic claim and idempotent finish — and finally readiness/listing. Claim and finish come before readiness because readiness is only meaningful once finish can clear blockers.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Create and update tasks](product.md#requirement-create-and-update-tasks) | tasks/1, tasks/2 |
| R2 | [Atomic claim](product.md#requirement-atomic-claim) | tasks/3 |
| R3 | [Idempotent finish that unblocks dependents](product.md#requirement-idempotent-finish-that-unblocks-dependents) | tasks/4 |
| R4 | [Find ready work](product.md#requirement-find-ready-work) | tasks/5 |

Every unit cites the R-IDs it satisfies. Do not renumber R-IDs.

---

## Key Technical Decisions

1. **`O_EXCL` lockfile for claim.** Portable atomic test-and-set; runs identically with no daemon.
2. **Finish is three atomic steps, idempotent as a whole.** Re-running is always safe — the recovery story for any crash.

---

## Unit IDs

Units are `tasks/n` — assigned once, never renumbered. Cite IDs in commits and tests (`feat(tasks): tasks/3 ...`).

---

### tasks/1 — Task schema and folder routing

**Goal:** Pydantic Task model (note fields + status/priority/claimed_by/blocks/blocked_by) and `open`/`done` routing.

**Requirements:** R1

**Dependencies:** —

**Files:**

```
src/brain/schemas/task.py
src/brain/core/tasks.py
```

**Test scenarios:**

- A task validates and lands in `tasks/open/` with `status: open`.

**Verification:** `uv run pytest tests/tasks/test_schema.py`

---

### tasks/2 — `task new` and `task update`

**Goal:** Create and field-update tasks from the CLI, including `blocks`/`blocked_by`.

**Requirements:** R1

**Dependencies:** tasks/1

**Files:**

```
src/brain/cli/task.py
```

**Test scenarios:**

- `task new --blocked-by t-c7d1` records the dependency edge.

**Verification:** `uv run pytest tests/tasks/test_new_update.py`

---

### tasks/3 — Atomic claim

**Goal:** `O_EXCL`-locked claim with `claimed_by`/`status` mutation and exit `4` on conflict.

**Requirements:** R2

**Dependencies:** tasks/2

**Files:**

```
src/brain/storage/locks.py
src/brain/core/tasks.py
src/brain/cli/task.py
```

**Test scenarios:**

- Two concurrent claims: exactly one wins, the other exits `4` with no mutation.

**Verification:** `uv run pytest tests/tasks/test_claim.py`

---

### tasks/4 — Idempotent finish + unblock

**Goal:** Append outcome, move to `done/`, strip the ID from dependents' `blocked_by`; safe to re-run.

**Requirements:** R3

**Dependencies:** tasks/3

**Files:**

```
src/brain/core/tasks.py
src/brain/cli/task.py
```

**Test scenarios:**

- Finish moves the file and clears the blocker on a dependent.
- A second finish is a no-op.

**Verification:** `uv run pytest tests/tasks/test_finish.py`

---

### tasks/5 — Readiness, listing, cancel

**Goal:** `task ready`/`--mine`/`--ready` readiness computation, `task list`/`get`, and `task cancel`.

**Requirements:** R4

**Dependencies:** tasks/4

**Files:**

```
src/brain/core/tasks.py
src/brain/cli/task.py
```

**Test scenarios:**

- `task ready` returns only open, unclaimed, fully-unblocked tasks.
- `--mine` matches owner or claimer.

**Verification:** `uv run pytest tests/tasks/test_ready_list.py`

---

## Progress

| Unit | Status |
|---|---|
| tasks/1 | NOT STARTED |
| tasks/2 | NOT STARTED |
| tasks/3 | NOT STARTED |
| tasks/4 | NOT STARTED |
| tasks/5 | NOT STARTED |
