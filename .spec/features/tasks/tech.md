---
type: feature-tech
feature: tasks
sibling: product.md
parent: ../../tech.md
updated: 2026-06-10
---

# Feature: Tasks — Architecture

Tasks reuse the note schema and storage primitives and add a status machine plus the concurrency-critical claim/finish operations. Correctness under concurrent agents is the whole point, so the atomic-claim lock and idempotent finish are the load-bearing parts. The frontmatter/ID/atomic-write/exit-code contracts are cross-cutting (root [tech.md](../../tech.md)); this feature owns the lifecycle on top of them.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/brain/cli/task.py        # brain task new|update|delete|claim|finish|cancel|list|get|ready
src/brain/core/tasks.py      # lifecycle, claim/finish, blocks/blocked_by, readiness
src/brain/storage/locks.py   # O_EXCL lockfiles for atomic claim
src/brain/schemas/task.py    # pydantic Task model (note fields + extras)
```

---

## Contract / API

Task frontmatter (note fields plus):

| Field | Type | Required | Notes |
|---|---|---|---|
| `id` | string | yes | `t-` prefix |
| `type` | const | yes | Always `task` |
| `status` | enum | yes | `open` \| `claimed` \| `done` \| `cancelled` |
| `priority` | enum | no | `low` \| `normal` \| `high` (default `normal`) |
| `claimed_by` | string\|`~` | no | `~` until claimed; owner name on claim |
| `blocks` | list[id] | no | Tasks this one blocks |
| `blocked_by` | list[id] | no | Tasks that must finish before this is ready |

Command surface:

| Command | Args | Description |
|---|---|---|
| `task new` | `"<title>" [--tags] [--owner] [--priority] [--blocks] [--blocked-by] [--body]` | Create in `tasks/open/` |
| `task update` | `<id> [--title] [--tags +/-] [--priority] [--blocks] [--blocked-by] [--body]` | Update fields |
| `task claim` | `<id> [--owner <name>]` | Atomic claim; sets `claimed_by`, `status=claimed`; fails (exit 4) if taken |
| `task finish` | `<id> [--outcome "<summary>"]` | Append outcome, move to `done/`, unblock dependents |
| `task cancel` | `<id> [--reason "<str>"]` | Cancel; append reason, set `status=cancelled`, move to `done/` |
| `task delete` | `<id> [--force]` | Hard delete; prompts unless `--force` |
| `task list` | `[--status (default open)] [--tags] [--owner] [--mine] [--ready] [--limit 20]` | List tasks |
| `task get` | `<id> [--full \| --meta]` | Show a task |
| `task ready` | `[--owner] [--tags]` | Unblocked, unclaimed tasks; JSON-friendly |

All commands accept the global `--json` (machine-readable) and `--quiet` (IDs/paths only) flags; `task ready` and `task list` are JSON-friendly by default.

Lifecycle: `open → claimed` (claim) · `claimed → open` (release/re-claim) · `open|claimed → done` (finish) · `open|claimed → cancelled` (cancel). `status` is authoritative; file location mirrors it (`open`/`claimed` in `tasks/open/`, `done`/`cancelled` in `tasks/done/`).

## Implementation Detail

- **Atomic claim.** Acquire `O_EXCL` on `tasks/.locks/<id>.lock`; under the lock, re-read frontmatter, verify `claimed_by == ~`, write `claimed_by` + `status=claimed` via temp + `os.replace`, release. If already claimed by another, exit `4` without mutation. `O_EXCL` create is the portable atomic test-and-set; an atomic-rename variant is equivalent on the same filesystem.
- **Idempotent finish.** Three individually-atomic steps: (1) append outcome + `status=done`; (2) rename `tasks/open/<id>.md → tasks/done/<id>.md`; (3) for each dependent listing `<id>` in `blocked_by`, remove the entry. The whole op is idempotent — re-running on a done task, or removing an already-gone entry, is a no-op. Crash recovery is "run it again."
- **Readiness.** A task is *ready* when it is `open`, unclaimed, and every `blocked_by` task is `done`. `task ready` and `--mine` (`owner == $BRAIN_AGENT or claimed_by == $BRAIN_AGENT`) answer "what should I pick up?"
- **Cancel and delete.** `task cancel` appends an optional `--reason`, sets `status=cancelled`, and atomically renames into `tasks/done/`. `task delete` is a hard removal of the file, prompting unless `--force`.
- **Exit codes.** `claim` returns `4` when the task is already claimed by another (above). Exit `5` (blocked) is reserved for an operation refused because the task still has unfinished `blocked_by` prerequisites — e.g. a strict claim/start gate. The default `claim` does **not** enforce readiness (an agent may claim a blocked task); see the Open Question for which command, if any, enforces `5`.

<!-- merge -->
The atomic-claim (`O_EXCL`) and idempotent multi-file finish patterns are the project's concurrency contract — they must run identically in daemon and fallback mode because the primitives live in `core`/`storage`, not the daemon. Coordination correctness does not depend on a running daemon.
<!-- /merge -->

## Open Questions

1. **`release`/`unclaim` command.** The lifecycle allows `claimed → open` but no command exposes it. *Default:* add `brain task release <id>` (clears `claimed_by`, status→open); confirm naming.
2. **Exit `5` (blocked) trigger.** Which command, if any, refuses an action on a still-blocked task? *Default:* reserve `5` for an opt-in strict gate (e.g. `task claim --strict` / a future `task start`); the default `claim`/`finish` do not enforce readiness. Confirm before IMPL.
