---
type: feature-tech
feature: tasks
sibling: product.md
parent: ../../tech.md
updated: 2026-06-21
---

# Tasks — Tech

Lifecycle on shared note/storage primitives. Graph deferred to Phase 3.

**Links:** [product](product.md) · [plan](plan.md) · schemas in root [tech.md](../../tech.md)

## Files

`cli/task.py` · `core/tasks.py` · `schemas/task.py` · `storage/locks.py`

## Commands

| Cmd | Notes |
|---|---|
| `new` / `update` | `--blocks`/`--blocked-by` inert v1; update locked |
| `claim` | `O_EXCL`; same owner = no-op; other owner = exit `4` |
| `finish` / `cancel` | append section + folder move; idempotent |
| `list` / `get` | warm index or dir scan; id only (no slug) |
| `delete` | hard, `--force` |

Lifecycle: `open→claimed→done|cancelled`. Terminal re-run = no-op. Exit `5` reserved for Phase 3 `--strict`.

<!-- merge -->
Claim/finish use `core`/`storage` directly — identical with daemon down.
<!-- /merge -->

## Phase 3 (deferred)

`task ready`, `release`, `--strict` (exit `5`), finish/cancel unblock-cascade.
