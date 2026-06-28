---
type: feature-tech
feature: notes
sibling: product.md
parent: ../../tech.md
updated: 2026-06-21
---

# Notes — Tech

Markdown + frontmatter via `core/notes.py`, `cli/note.py`. Schemas/IDs/storage: root [tech.md](../../tech.md).

**Links:** [product](product.md) · [plan](plan.md)

## Files

`cli/note.py` · `core/notes.py` · `core/wikilinks.py` · (+ shared `schemas`, `storage`, `ids`)

## Commands

| Cmd | Summary |
|---|---|
| `new` | `--body` → `--file` → `$EDITOR` (TTY only) |
| `append` | `<id\|slug>` + optional `--section` (`##`), `--timestamp` |
| `update` | fields; `--tags +x,-y` or replace list; `--type` moves folder |
| `get` | default preview; `--full` / `--meta-only` / `--related` |
| `list` | filters; `--limit 20`; `--sort updated\|created\|title` |
| `delete` | `--force` skips prompt |

Files named `<id>.md`. Body writes refresh `related` via `wikilinks.py` (on-disk scan, daemon-independent). Append/update use `notes/.locks/<id>.lock`. Slug = normalized title; ambiguous slug → exit `2`.

<!-- merge -->
Wikilinks: `[[Title]]` → id by title match; `[[n-id]]`/`[[t-id]]` passthrough; dedupe `related`; unresolvable → dangling in `brain status`.
<!-- /merge -->
