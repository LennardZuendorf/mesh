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
| `type` | enum | yes | `note` \| `log` \| `decision` \| `reference`; drives folder (`note → notes/` root; others → `notes/<type-plural>/`), per root tech.md routing |
| `title` | string | yes | Human title; also feeds the ID hash |
| `tags` | list[str] | no | Lowercase; AND/OR filtering and tag-pull |
| `owner` | string | no | Defaults to `$BRAIN_AGENT` / `agent_name` |
| `created` | datetime | yes | ISO-8601 UTC, set on create |
| `updated` | datetime | yes | ISO-8601 UTC, bumped on every write |
| `related` | list[id] | no | Wikilink refs resolved to IDs by `core/wikilinks.py` (daemon-independent; the daemon only caches the result) |

Command surface:

| Command | Args | Description |
|---|---|---|
| `note new` | `"<title>" [--type] [--tags] [--owner] [--body "<str>"] [--file <path>]` | Create a note. Body source: `--body`, else `--file` (read as UTF-8 — an external input path, not vault-sandboxed; the written destination is), else `$EDITOR` **only on an interactive TTY**; on a machine path (`--json`/MCP/no TTY) with no `--body`/`--file`, exit `2` rather than block on an editor |
| `note append` | `<id\|slug> "<content>" [--section "<heading>"] [--timestamp]` | Append content, optionally under a heading / with a timestamp |
| `note update` | `<id\|slug> [--title] [--tags (+tag/-tag)] [--type] [--body]` | Update fields; `+tag`/`-tag` add/remove, bare list replaces |
| `note delete` | `<id\|slug> [--force]` | Hard delete; prompts unless `--force` |
| `note get` | `<id\|slug> [--full \| --meta-only \| --related]` | Default: frontmatter + first 200 chars; `--full` returns the whole body, `--meta-only` frontmatter only, `--related` inlines related notes' frontmatter (resolved on-disk, daemon-independent) |
| `note list` | `[--tags] [--any-tag] [--owner] [--type] [--since <ISO\|duration>] [--limit 20] [--sort updated\|created\|title]` | List notes; `--tags` is AND, `--any-tag` switches to OR; `--since` accepts an ISO timestamp or a duration (e.g. `24h`, `7d`) for "what changed recently" |

## Implementation Detail

- **Write path & filenames.** Files are named by ID — `<id>.md` — under the type-derived folder, so a `--title` change never renames the file (only a `--type` change moves it between folders). Every body-changing write (`new`, `append`, `update --body`) is the place `core/notes.py` invokes `core/wikilinks.py` to refresh `related` before the atomic rewrite; field-only updates that don't touch the body skip resolution.
- **Reads and coexistence.** `note get`/`note list` are cheap **direct reads** (single-file read / `notes/` directory scan + frontmatter filter) — always available, no engine or daemon required. Heavy *recall* (semantic/hybrid) is delegated to the search feature (`indexed`), never reimplemented here. Brain writes vault-native files that Tolaria's MCP also reads; Brain may call Tolaria's read tools (`search_notes`/`get_vault_context`) opportunistically but never depends on them.
- **Section-aware append.** `--section "<heading>"` finds the matching `##` heading and appends beneath it, creating the heading at the end if absent. `--timestamp` prefixes the appended block with an ISO-8601 line. The whole note is rewritten atomically (temp + `os.replace`).
- **Field updates.** `--tags +x,-y` mutates the set additively; a bare `--tags a,b` replaces it. `--type` changes drive a folder move (atomic rename) to keep folder routing consistent with frontmatter.
- **Slug resolution.** `<id|slug>` accepts either a hash ID or a slug. An argument matching `^[nt]-[a-z0-9]{4,}$` is treated as an ID; anything else is a slug. A slug is the title lowercased, non-alphanumerics collapsed to single hyphens, leading/trailing hyphens trimmed. Slugs resolve to an ID through the index by exact normalized-title match; a slug matching two or more notes errors with exit `2` and lists the candidate IDs.

<!-- merge -->
Wikilink resolution is shared with tasks and the daemon's index: `core/wikilinks.py` resolves `[[Title]]` (normalized-title match → ID), `[[n-id]]`, and `[[t-id]]` (ID passthrough), maintaining a deduplicated `related` set. It runs against an on-disk title/ID scan and is **daemon-independent** — the daemon only caches the resolution, so wikilinks resolve identically when the daemon is down (and in feature 1, before a daemon exists). Unresolvable links stay verbatim in the body and surface via `brain status` as dangling. This belongs to the project-wide resolution contract.
<!-- /merge -->

## Open Questions

None — slug normalization, headless-create behaviour, folder routing, and daemon-independent wikilink resolution are now specified above.
