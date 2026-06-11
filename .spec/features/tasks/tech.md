---
type: feature-tech
feature: tasks
sibling: product.md
parent: ../../tech.md
updated: 2026-06-10
---

# Feature: Tasks — Architecture

Tasks reuse the note schema and storage primitives and add a status machine plus the concurrency-critical claim/finish operations. Correctness under concurrent agents is the whole point, so the atomic-claim lock and idempotent finish are the load-bearing parts of **v1**. The dependency graph (readiness, release, strict gate, unblock-cascade) is deferred to a later phase. The frontmatter/ID/atomic-write/exit-code contracts are cross-cutting (root [tech.md](../../tech.md)); this feature owns the lifecycle on top of them.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/brain/cli/task.py        # brain task new|update|claim|finish|cancel|delete|list|get
src/brain/core/tasks.py      # lifecycle, claim/finish/cancel
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
| `blocks` | list[id] | no | Recorded in v1, inert until the graph phase |
| `blocked_by` | list[id] | no | Recorded in v1, inert until the graph phase |

Command surface (v1):

| Command | Args | Description |
|---|---|---|
| `task new` | `"<title>" [--tags] [--owner] [--priority] [--blocks] [--blocked-by] [--body]` | Create in `tasks/open/`; `--blocks`/`--blocked-by` recorded but inert in v1 |
| `task update` | `<id> [--title] [--tags +/-] [--priority] [--blocks] [--blocked-by] [--body]` | Update fields |
| `task claim` | `<id> [--owner <name>]` | Atomic claim; sets `claimed_by`, `status=claimed`; fails (exit 4) if taken |
| `task finish` | `<id> [--outcome "<summary>"]` | Append outcome, set `status=done`, move to `done/`; idempotent |
| `task cancel` | `<id> [--reason "<str>"]` | Cancel; append reason, set `status=cancelled`, move to `done/`; idempotent |
| `task delete` | `<id> [--force]` | Hard delete; prompts unless `--force` |
| `task list` | `[--status open\|claimed\|done\|cancelled (default open)] [--tags] [--owner] [--mine] [--limit 20]` | List tasks; combine `--mine --status claimed` for "my in-progress work" |
| `task get` | `<id> [--full \| --meta-only]` | Show a task |

All commands accept the global `--json` (machine-readable) and `--quiet` (IDs/paths only) flags; `task list` is JSON-friendly by default.

Lifecycle (v1): `open → claimed` (claim) · `open|claimed → done` (finish) · `open|claimed → cancelled` (cancel). `status` is authoritative; file location mirrors it (`open`/`claimed` in `tasks/open/`, `done`/`cancelled` in `tasks/done/`).

## Implementation Detail

- **Atomic claim.** Acquire `O_EXCL` on `tasks/.locks/<id>.lock`; under the lock, re-read frontmatter, verify `claimed_by == ~`, write `claimed_by` + `status=claimed` via temp + `os.replace`, release. If already claimed by another, exit `4` without mutation. `O_EXCL` create is the portable atomic test-and-set; an atomic-rename variant is equivalent on the same filesystem.
- **Idempotent finish.** Two individually-atomic steps: (1) append outcome + `status=done`; (2) rename `tasks/open/<id>.md → tasks/done/<id>.md`. Idempotent — re-running on a done task is a no-op. Crash recovery is "run it again."
- **Cancel and delete.** `task cancel` appends an optional `--reason`, sets `status=cancelled`, and atomically renames into `tasks/done/` (idempotent). `task delete` is a hard removal of the file, prompting unless `--force`.
- **Listing.** `task list` filters the warm index (or a directory scan when the daemon is down) by `--status`/`--owner`/`--mine` (`owner == $BRAIN_AGENT or claimed_by == $BRAIN_AGENT`)/tags — the minimal coordination-visibility surface for v1.
- **Exit codes.** `claim` returns `4` when the task is already claimed by another. Exit `5` (blocked) is **reserved** for the deferred strict gate and is not emitted in v1.

<!-- merge -->
The atomic-claim (`O_EXCL`) and idempotent finish patterns are the project's concurrency contract — they must run identically in daemon and fallback mode because the primitives live in `core`/`storage`, not the daemon. Coordination correctness does not depend on a running daemon.
<!-- /merge -->

## Deferred — dependency-graph phase

Not built in v1; scheduled as a later phase with fresh unit/requirement IDs:

- **Readiness** (`task ready`): `open`, unclaimed, every `blocked_by` task `done`.
- **Unblock-cascade**: `finish`/`cancel` remove `<id>` from each dependent's `blocked_by`.
- **`task release`**: `claimed → open` (clear `claimed_by`).
- **Strict gate**: `task claim --strict` refuses (exit `5`) a task with unfinished `blocked_by`.

## Open Questions

None for v1. The deferred graph phase carries its own design when scheduled.
