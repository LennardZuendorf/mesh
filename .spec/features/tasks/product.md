---
type: feature-product
feature: tasks
sibling: tech.md
parent: ../../product.md
updated: 2026-06-10
---

# Feature: Tasks — Product

The `task` verb is how work is coordinated and handed off. A task is a note with `type: task` plus lifecycle fields. Agents and the human create tasks, claim them, finish them, and wire dependencies — coordination is durable and inspectable, living in files rather than a chat transcript. There is no separate handoff mechanism; tasks *are* the handoff.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The `brain task` command surface; task lifecycle (`open`/`claimed`/`done`/`cancelled`); the `status`, `priority`, `claimed_by`, `blocks`, `blocked_by` fields; atomic claim and idempotent finish; readiness computation; coordination exit codes (`4` claimed, `5` blocked) |
| **Does not own** | Note bodies and note-only fields (notes feature); the shared frontmatter/ID/atomic-write contracts (root tech.md); search/ranking (search feature); the watcher (daemon feature) |

---

## Requirements

### Requirement: Create and update tasks

The system SHALL create a task in `tasks/open/` with lifecycle fields and MUST update its fields (`title`, `tags`, `priority`, `blocks`, `blocked_by`, body) in place.

#### Scenario: File a task with a dependency

- **Given** an agent runs `brain task new "Verify NDC config" --priority high --blocked-by t-c7d1`
- **When** the command completes
- **Then** a task file is created in `tasks/open/` with `status: open` and `t-c7d1` in its `blocked_by`

### Requirement: Atomic claim

The system SHALL guarantee that two agents cannot both claim the same task; a claim sets `claimed_by` and `status: claimed`, and MUST fail with a distinct exit code when the task is already claimed by another.

#### Scenario: Concurrent claim

- **Given** two agents attempt `brain task claim t-c7d1` simultaneously
- **When** both run
- **Then** exactly one succeeds and the other exits `4` (already claimed) without mutating the file

### Requirement: Idempotent finish that unblocks dependents

The system SHALL append an outcome, set `status: done`, move the file to `tasks/done/`, and remove the task's ID from every dependent's `blocked_by`; re-running finish MUST be a no-op.

#### Scenario: Finish clears a blocker

- **Given** `t-c7d1` is `blocked_by` of a downstream task and is claimed
- **When** `brain task finish t-c7d1` runs
- **Then** `t-c7d1` moves to `done/`, the downstream task's `blocked_by` no longer lists it, and a second `finish t-c7d1` changes nothing

### Requirement: Find ready work

The system SHALL report tasks that are `open`, unclaimed, and have all `blocked_by` prerequisites `done`, and MUST support `--mine` and tag/owner filters.

#### Scenario: Pick up the next task

- **Given** several open tasks, some still blocked
- **When** an agent runs `brain task ready --owner flights-agent`
- **Then** only unblocked, unclaimed tasks are returned, JSON-friendly

### Requirement: Cancel or delete a task

The system SHALL cancel a task — appending an optional `--reason`, setting `status: cancelled`, and moving it to `tasks/done/` — and SHALL delete a task as a hard, `--force`-guarded removal of the file.

#### Scenario: Cancel a task

- **Given** an open task `t-c7d1`
- **When** `brain task cancel t-c7d1 --reason "no longer relevant"` runs
- **Then** its `status` becomes `cancelled`, the reason is recorded, and the file moves to `tasks/done/`

#### Scenario: Guarded delete

- **Given** a task `t-c7d1` exists
- **When** `brain task delete t-c7d1 --force` runs
- **Then** the file is removed and the task no longer appears in `task list`

Reference requirements as R1, R2, R3, R4, R5 in the feature plan's Requirements Trace.

## User Experience

```
$ brain task ready --owner flights-agent --json
$ brain task claim t-c7d1 --owner flights-agent          # atomic; exit 4 if taken
$ brain task finish t-c7d1 --outcome "All J/C fares resolved via CLID fallback."
```

## Non-Goals

- Scheduling, orchestration, or any agent runtime — Brain is not an agent platform.
- An external task backend — tasks are Markdown files, never Todoist or an API.
