---
type: feature-plan
feature: tasks
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-10
---

# Feature: Tasks — Implementation Plan

Tasks (v1) delivers the differentiating coordination core: per-file Markdown tasks, atomic `O_EXCL` claim, idempotent finish, cancel, and listing by status/owner. Concurrency correctness is the acceptance bar. The dependency graph is a deferred later phase.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Starts when **notes** is `DONE` (root [plan.md](../../plan.md) Feature Sequence) — it reuses the note schema and storage primitives. Does not depend on any other feature's units.

---

## Problem Frame

Once memory exists, agents need to pass work without a human relay. This plan adds the task schema and lifecycle, then the correctness-critical atomic claim and idempotent finish, then listing/cancel/delete. v1 stops short of the `blocks`/`blocked_by` dependency graph — edges may be recorded but are inert.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Create and update tasks](product.md#requirement-create-and-update-tasks) | tasks/1, tasks/2 |
| R2 | [Atomic claim](product.md#requirement-atomic-claim) | tasks/3 |
| R3 | [Idempotent finish](product.md#requirement-idempotent-finish) | tasks/4 |
| R4 | [List tasks by status and ownership](product.md#requirement-list-tasks-by-status-and-ownership) | tasks/5 |
| R5 | [Cancel or delete a task](product.md#requirement-cancel-or-delete-a-task) | tasks/5, tasks/6 |

Every unit cites the R-IDs it satisfies. Do not renumber R-IDs.

---

## Key Technical Decisions

1. **`O_EXCL` lockfile for claim.** Portable atomic test-and-set; runs identically with no daemon.
2. **Finish is two atomic steps, idempotent as a whole.** Re-running is always safe — the recovery story for any crash.
3. **Graph deferred.** `blocks`/`blocked_by` are stored but inert; readiness/release/unblock/strict ship in a later phase, keeping v1 small.

---

## Unit IDs

Units are `tasks/n` — assigned once, never renumbered. Cite IDs in commits and tests (`feat(tasks): tasks/3 ...`).

---

### tasks/1 — Task schema and folder routing

**Goal:** Pydantic Task model (note fields + status/priority/claimed_by, inert blocks/blocked_by) and `open`/`done` routing.

**Requirements:** R1

**Dependencies:** —

**Files:** `src/brain/schemas/task.py`, `src/brain/core/tasks.py`

**Test scenarios:**

- A task validates and lands in `tasks/open/` with `status: open`, a `t-` hash ID, and `claimed_by: ~`.

**Verification:** `uv run pytest tests/tasks/test_schema.py`

---

### tasks/2 — `task new` and `task update`

**Goal:** Create and field-update tasks from the CLI; `--blocks`/`--blocked-by` are recorded but inert.

**Requirements:** R1

**Dependencies:** tasks/1

**Files:** `src/brain/cli/task.py`

**Test scenarios:**

- `task new` writes a valid file and prints `<id>  <path>`.
- `--blocked-by t-c7d1` records the edge in frontmatter (no readiness behaviour in v1).

**Verification:** `uv run pytest tests/tasks/test_new_update.py`

---

### tasks/3 — Atomic claim

**Goal:** `O_EXCL`-locked claim with `claimed_by`/`status` mutation and exit `4` on conflict.

**Requirements:** R2

**Dependencies:** tasks/2

**Files:** `src/brain/storage/locks.py`, `src/brain/core/tasks.py`, `src/brain/cli/task.py`

**Test scenarios:**

- Two concurrent claims: exactly one wins, the other exits `4` with no mutation.

**Verification:** `uv run pytest tests/tasks/test_claim.py`

---

### tasks/4 — Idempotent finish

**Goal:** Append outcome, set `status=done`, move to `done/`; safe to re-run.

**Requirements:** R3

**Dependencies:** tasks/3

**Files:** `src/brain/core/tasks.py`, `src/brain/cli/task.py`

**Test scenarios:**

- Finish moves the file to `done/` and records the outcome.
- A second finish is a no-op.

**Verification:** `uv run pytest tests/tasks/test_finish.py`

---

### tasks/5 — Listing and cancel

**Goal:** `task list --status/--owner/--mine`, `task get`, and `task cancel` (reason + move to `done/`, idempotent).

**Requirements:** R4, R5

**Dependencies:** tasks/4

**Files:** `src/brain/core/tasks.py`, `src/brain/cli/task.py`

**Test scenarios:**

- `task list --status claimed --mine` returns only the caller's claimed, unfinished tasks.
- `task cancel` sets `status=cancelled` and moves to `done/`; re-running is a no-op.

**Verification:** `uv run pytest tests/tasks/test_list_cancel.py`

---

### tasks/6 — `task delete`

**Goal:** Hard, guarded deletion of a task by id, with a `--force` bypass for the confirmation prompt.

**Requirements:** R5

**Dependencies:** tasks/2

**Files:** `src/brain/cli/task.py`, `src/brain/core/tasks.py`

**Test scenarios:**

- `task delete --force` removes the file; the task disappears from `task list`.
- Without `--force`, deletion prompts for confirmation.

**Verification:** `uv run pytest tests/tasks/test_delete.py`

---

## Deferred (later phase)

The dependency graph — `task ready` readiness, `task release`, the `claim --strict` gate (exit `5`), and the finish/cancel unblock-cascade — is out of v1 scope and gets fresh `tasks/n` units when scheduled.

## Progress

| Unit | Status |
|---|---|
| tasks/1 | NOT STARTED |
| tasks/2 | NOT STARTED |
| tasks/3 | NOT STARTED |
| tasks/4 | NOT STARTED |
| tasks/5 | NOT STARTED |
| tasks/6 | NOT STARTED |
