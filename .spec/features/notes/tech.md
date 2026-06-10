---
type: feature-tech
feature: notes
sibling: product.md
parent: ../../tech.md
updated: 2026-06-10
---

# Feature: Notes — Architecture

Notes are Markdown files with YAML frontmatter, manipulated through `core/notes.py` and the `brain note` Typer subcommands. The note schema, hash-ID scheme, atomic-write, and path-sandbox contracts are cross-cutting and defined in root [tech.md](../../tech.md); this feature consumes them and adds note-specific CRUD, section-aware appends, and wikilink resolution.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/brain/cli/note.py        # brain note new|append|update|delete|get|list
src/brain/core/notes.py      # note CRUD, section-aware append, field updates
src/brain/core/wikilinks.py  # [[link]] -> ID resolution, related graph
src/brain/core/ids.py        # shared hash-ID generation (cross-cutting, see root tech.md)
src/brain/schemas/note.py    # pydantic Note model (cross-cutting, see root tech.md)
src/brain/storage/files.py   # atomic write + folder routing (cross-cutting, see root tech.md)
src/brain/storage/sandbox.py # path sandbox (cross-cutting, see root tech.md)
```

---

## Contract / API

Note frontmatter (see root tech.md for the canonical schema invariant):

| Field | Type | Required | Notes |
|---|---|---|---|
| `id` | string | yes | `n-` + hash; assigned on create, never changes |
| `type` | enum | yes | `note` \| `log` \| `decision` \| `reference`; drives subfolder |
| `title` | string | yes | Human title; also feeds the ID hash |
| `tags` | list[str] | no | Lowercase; AND/OR filtering and tag-pull |
| `owner` | string | no | Defaults to `$BRAIN_AGENT` / `agent_name` |
| `created` | datetime | yes | ISO-8601 UTC, set on create |
| `updated` | datetime | yes | ISO-8601 UTC, bumped on every write |
| `related` | list[id] | no | Wikilink refs resolved to IDs by the daemon |

Command surface:

| Command | Args | Description |
|---|---|---|
| `note new` | `"<title>" [--type] [--tags] [--owner] [--body "<str>"] [--file <path>]` | Create a note; `--file` ingests body, else `$EDITOR` |
| `note append` | `<id\|slug> "<content>" [--section "<heading>"] [--timestamp]` | Append content, optionally under a heading / with a timestamp |
| `note update` | `<id\|slug> [--title] [--tags (+tag/-tag)] [--type] [--body]` | Update fields; `+tag`/`-tag` add/remove, bare list replaces |
| `note delete` | `<id\|slug> [--force]` | Hard delete; prompts unless `--force` |
| `note get` | `<id\|slug> [--full \| --meta \| --related]` | Default: frontmatter + first 200 chars; `--full` returns the whole body, `--meta` frontmatter only, `--related` inlines related notes' frontmatter |
| `note list` | `[--tags] [--owner] [--type] [--since <ISO>] [--limit 20] [--sort updated\|created\|title]` | List notes |

## Implementation Detail

- **Section-aware append.** `--section "<heading>"` finds the matching `##` heading and appends beneath it, creating the heading at the end if absent. `--timestamp` prefixes the appended block with an ISO-8601 line. The whole note is rewritten atomically (temp + `os.replace`).
- **Field updates.** `--tags +x,-y` mutates the set additively; a bare `--tags a,b` replaces it. `--type` changes drive a folder move (atomic rename) to keep folder routing consistent with frontmatter.
- **Slug resolution.** `<id|slug>` accepts a kebab-cased title resolved to an ID through the index; ambiguous slugs error and ask for the ID.

<!-- merge -->
Wikilink resolution is shared with tasks and the daemon's index: `core/wikilinks.py` resolves `[[Title]]` (title match → ID) and `[[t-c7d1]]` (ID passthrough), maintaining a deduplicated `related` set. Unresolvable links stay verbatim in the body and surface via `brain status` as dangling. This belongs to the project-wide resolution contract.
<!-- /merge -->

## Open Questions

1. **`slug` definition.** Confirm slug = kebab-cased title resolved via the index, with ambiguous slugs erroring. *Default:* yes.
