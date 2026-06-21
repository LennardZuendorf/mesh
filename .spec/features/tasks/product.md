---
type: feature-product
feature: tasks
sibling: tech.md
parent: ../../product.md
updated: 2026-06-21
---

# Tasks — Product

`task` = coordination/handoff. Note + lifecycle fields; atomic claim; no separate handoff primitive.

**v1:** claim/finish/cancel/list/delete. `blocks`/`blocked_by` recorded, **inert** until Phase 3.

**Links:** [product](../../product.md) · [tech](tech.md) · [plan](plan.md)

## Scope

| | |
|---|---|
| **Owns** | `brain task`, lifecycle, exit `4` on lost claim |
| **Does not own** | note CRUD, search, daemon; dependency graph (Phase 3) |

## Requirements

### R1: Create / update
SHALL create in `tasks/open/`; update fields; `--blocked-by` recorded only.

- New task → `t-` id, `claimed_by: ~`. Unknown `--owner` → exit `2`.

### R2: Claim
SHALL atomic claim; exit `4` if taken by another; same-agent reclaim is no-op. No `release` in v1.

### R3: Finish
SHALL move to `done/`, append outcome; idempotent. Open or claimed → done allowed.

### R4: List / get
SHALL filter by status/owner/`--mine`/tags.

### R5: Cancel / delete
SHALL cancel → `done/` + reason (idempotent); delete guarded by `--force`.

## UX

```bash
brain task new "Verify NDC" --priority high --owner flights-agent
brain task claim t-c7d1    # exit 4 if taken
brain task finish t-c7d1 --outcome "Done."
brain task list --mine --status claimed --json
```
