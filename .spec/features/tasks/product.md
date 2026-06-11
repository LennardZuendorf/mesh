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
| **Owns** | The `brain task` command surface; task lifecycle (`open`/`claimed`/`done`/`cancelled`); the `status`, `priority`, `claimed_by`, `blocks`, `blocked_by` fields; atomic claim, idempotent finish, idempotent cancel (which unblocks dependents), release; readiness computation; listing by status/owner/`--mine`; coordination exit codes (`4` claimed; `5` blocked, emitted only by the opt-in `claim --strict` gate) |
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

### Requirement: Release a claimed task

The system SHALL release a claimed task back to `open` — clearing `claimed_by` and setting `status: open` — so an agent can hand back work it cannot finish. Release is idempotent on an already-open task.

#### Scenario: Hand work back

- **Given** a task `t-c7d1` claimed by `flights-agent`
- **When** `brain task release t-c7d1` runs
- **Then** `claimed_by` becomes `~`, `status` becomes `open`, and the task is `ready` again if its `blocked_by` are all `done`

### Requirement: List tasks by status and ownership

The system SHALL list tasks filtered by `--status` (`open`/`claimed`/`done`/`cancelled`), `--owner`, `--mine`, and tags — so the operator can see in-progress work and an agent can recover the tasks it has claimed but not finished.

#### Scenario: See what I am mid-way through

- **Given** several tasks across statuses
- **When** an agent runs `brain task list --mine --status claimed --json`
- **Then** only its own claimed, unfinished tasks are returned

### Requirement: Cancel or delete a task

The system SHALL cancel a task — appending an optional `--reason`, setting `status: cancelled`, moving it to `tasks/done/`, and removing its ID from every dependent's `blocked_by` (symmetric with `finish`, so cancelling a blocker never strands its dependents) — and SHALL delete a task as a hard, `--force`-guarded removal of the file. Cancel is idempotent.

#### Scenario: Cancel a task and clear its dependents

- **Given** an open task `t-c7d1` that is `blocked_by` of a downstream task
- **When** `brain task cancel t-c7d1 --reason "no longer relevant"` runs
- **Then** its `status` becomes `cancelled`, the reason is recorded, the file moves to `tasks/done/`, and the downstream task's `blocked_by` no longer lists `t-c7d1`

#### Scenario: Guarded delete

- **Given** a task `t-c7d1` exists
- **When** `brain task delete t-c7d1 --force` runs
- **Then** the file is removed and the task no longer appears in `task list`

Reference requirements as R1–R7 in the feature plan's Requirements Trace (R5 = cancel/delete, R6 = release, R7 = list by status/ownership).

## User Experience

```
$ brain task ready --owner flights-agent --json
$ brain task claim t-c7d1 --owner flights-agent          # atomic; exit 4 if taken
$ brain task finish t-c7d1 --outcome "All J/C fares resolved via CLID fallback."
```

## Non-Goals

- Scheduling, orchestration, or any agent runtime — Brain is not an agent platform.
- An external task backend — tasks are Markdown files, never Todoist or an API.

## Prior Art & Inspiration

**Anchor — [tick-md](https://purplehorizons.io/blog/tick-md-multi-agent-coordination-markdown):** multi-agent coordination in a single git-tracked Markdown file, where agents claim tasks and a file-lock stops two agents colliding.

- **Borrow:** the file *is* the coordination; a claim is a lock; everything stays inspectable in plain Markdown under Git.
- **Differ:** brain uses an `O_EXCL` atomic test-and-set plus idempotent finish and a `blocks`/`blocked_by` dependency graph, so handoff is crash-safe with **no process running** — tick-md only locks one file; GBrain needs a Postgres job queue ("Minions") for the same guarantee.
- **Contrast — [Claude Code Agent Teams](https://www.mindstudio.ai/blog/claude-code-agent-teams-parallel-collaboration):** a shared task list scoped to one session; brain's tasks are durable across sessions and agents.
