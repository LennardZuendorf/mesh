---
type: feature-product
feature: tasks
sibling: tech.md
parent: ../../product.md
updated: 2026-06-10
---

# Feature: Tasks — Product

The `task` verb is how work is coordinated and handed off. A task is a note with `type: task` plus lifecycle fields. Agents and the human create tasks, claim them, and finish them — coordination is durable and inspectable, living in files rather than a chat transcript. There is no separate handoff mechanism; tasks *are* the handoff.

**v1 scope:** the differentiating core — per-file Markdown tasks with hash IDs, an atomic `O_EXCL` claim, idempotent finish, cancel, and listing by status/owner. The **dependency graph** (`blocks`/`blocked_by` readiness, `release`, the strict gate, and finish/cancel unblock-cascade) is a deliberately **deferred** later phase; `blocked_by` edges may be *recorded* in v1 but are inert until then.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The `brain task` command surface; task lifecycle (`open`/`claimed`/`done`/`cancelled`); the `status`, `priority`, `claimed_by` fields; atomic claim, idempotent finish, idempotent cancel, delete; listing by status/owner/`--mine`; the `4` (already claimed) coordination exit code |
| **Does not own** | Note bodies and note-only fields (notes feature); the shared frontmatter/ID/atomic-write contracts (root tech.md); search/ranking (search feature); the watcher (daemon feature) |
| **Deferred (later phase)** | The dependency graph: `blocks`/`blocked_by` readiness (`task ready`), `task release`, the strict claim gate (exit `5`), and the finish/cancel unblock-cascade |

---

## Requirements

### Requirement: Create and update tasks

The system SHALL create a task in `tasks/open/` with lifecycle fields and MUST update its fields (`title`, `tags`, `priority`, body) in place. A `--blocked-by`/`--blocks` edge MAY be recorded in frontmatter for the future graph phase but is not acted upon in v1.

#### Scenario: File a task

- **Given** an agent runs `brain task new "Verify NDC config" --priority high`
- **When** the command completes
- **Then** a task file is created in `tasks/open/` with `status: open`, a `t-` hash ID, and `claimed_by: ~`

### Requirement: Atomic claim

The system SHALL guarantee that two agents cannot both claim the same task; a claim sets `claimed_by` and `status: claimed`, and MUST fail with a distinct exit code when the task is already claimed by another.

#### Scenario: Concurrent claim

- **Given** two agents attempt `brain task claim t-c7d1` simultaneously
- **When** both run
- **Then** exactly one succeeds and the other exits `4` (already claimed) without mutating the file

### Requirement: Idempotent finish

The system SHALL append an optional outcome, set `status: done`, and move the file to `tasks/done/`; re-running finish MUST be a no-op. (Unblocking dependents arrives with the deferred graph phase.)

#### Scenario: Finish a task

- **Given** a claimed task `t-c7d1`
- **When** `brain task finish t-c7d1 --outcome "All J/C fares resolved via CLID fallback."` runs
- **Then** `t-c7d1` moves to `done/` with `status: done` and the outcome recorded, and a second `finish t-c7d1` changes nothing

### Requirement: List tasks by status and ownership

The system SHALL list tasks filtered by `--status` (`open`/`claimed`/`done`/`cancelled`), `--owner`, `--mine`, and tags — so the operator can see in-progress work and an agent can recover the tasks it has claimed but not finished.

#### Scenario: See what I am mid-way through

- **Given** several tasks across statuses
- **When** an agent runs `brain task list --mine --status claimed --json`
- **Then** only its own claimed, unfinished tasks are returned

### Requirement: Cancel or delete a task

The system SHALL cancel a task — appending an optional `--reason`, setting `status: cancelled`, and moving it to `tasks/done/` (idempotent) — and SHALL delete a task as a hard, `--force`-guarded removal of the file.

#### Scenario: Cancel a task

- **Given** an open task `t-c7d1`
- **When** `brain task cancel t-c7d1 --reason "no longer relevant"` runs
- **Then** its `status` becomes `cancelled`, the reason is recorded, and the file moves to `tasks/done/`

#### Scenario: Guarded delete

- **Given** a task `t-c7d1` exists
- **When** `brain task delete t-c7d1 --force` runs
- **Then** the file is removed and the task no longer appears in `task list`

Reference requirements as R1–R5 in the feature plan's Requirements Trace. The deferred graph phase (readiness, release, strict gate, unblock-cascade) gets fresh requirement IDs when it is scheduled.

## User Experience

```
$ brain task new "Verify NDC config for LH" --priority high --owner flights-agent
$ brain task list --mine --status claimed --json
$ brain task claim t-c7d1 --owner flights-agent          # atomic; exit 4 if taken
$ brain task finish t-c7d1 --outcome "All J/C fares resolved via CLID fallback."
```

## Prior Art & Inspiration

**Anchor — [tick-md](https://purplehorizons.io/blog/tick-md-multi-agent-coordination-markdown):** multi-agent coordination in a single git-tracked Markdown file, where agents claim tasks and a file-lock stops two agents colliding.

- **Borrow:** the file *is* the coordination; a claim is a lock; everything stays inspectable in plain Markdown under Git. (For the later graph phase: tick's Mermaid/ASCII dependency-graph rendering.)
- **Differ:** Brain uses an `O_EXCL` atomic test-and-set over **per-task files with hash IDs**, crash-safe with **no process running** — tick centralizes into one `TICK.md` with an advisory `.tick.lock`+timeout and **sequential IDs**, which Brain's spec forbids. Brain self-builds because routing into tick would surrender per-file storage, `O_EXCL`, and hash IDs for no real delivery-speed gain.
- **Contrast — [Claude Code Agent Teams](https://www.mindstudio.ai/blog/claude-code-agent-teams-parallel-collaboration):** a shared task list scoped to one session; Brain's tasks are durable across sessions and agents.

## Non-Goals

- Scheduling, orchestration, or any agent runtime — Brain is not an agent platform.
- An external task backend — tasks are Markdown files, never Todoist, tick's container format, or an API.
