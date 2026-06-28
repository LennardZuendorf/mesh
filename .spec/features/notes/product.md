---
type: feature-product
feature: notes
sibling: tech.md
parent: ../../product.md
updated: 2026-06-21
---

# Notes — Product

`note` captures knowledge as Markdown. Brain **owns writes** (atomic, hash IDs, wikilinks); **direct reads** for `get`/`list`. Coexists with [Tolaria](https://github.com/refactoringhq/tolaria) on the same folder — `note` commands only surface files with a brain `id`; recall is `search`.

**Links:** [product](../../product.md) · [tech](tech.md) · [plan](plan.md)

## Scope

| | |
|---|---|
| **Owns** | `brain note`, note frontmatter, append/sections, wikilinks/`related` |
| **Does not own** | vault/Git, tasks, search ranking, daemon |

## Requirements

### R1: Create
SHALL create `.md` with hash `n-` id, routed by `type` (see root tech).

- Create decision → file in `notes/decisions/` with tags and timestamps.
- Headless (`--json`/MCP) without `--body`/`--file` → exit `2`, not `$EDITOR`.

### R2: Amend
SHALL append (optional `--section`, `--timestamp`) and update fields; concurrent edits serialized (`O_EXCL`).

- Append under heading bumps `updated`; concurrent appends both land.

### R3: Read / list
SHALL `get <id|slug>` (preview/full/meta/related) and `list` with tag/owner/type/`--since` filters. Brain-id files only.

- `get` returns frontmatter + 200-char preview; `list --since 7d --tags ndc` filters correctly.

### R4: Wikilinks
SHALL resolve `[[Title]]`, `[[n-id]]`, `[[t-id]]` → `related`; dangling links stay verbatim, reported in `brain status`.

### R5: Delete
SHALL hard-delete with `--force` or prompt.

Trace: R1–R5 → [plan](plan.md).

## UX

```bash
brain note new "CLID fallback" --type decision --tags ndc --body "..."
brain note append n-a3f2 "Confirmed J/C" --section Follow-ups --timestamp
brain note get n-a3f2 --json
```
