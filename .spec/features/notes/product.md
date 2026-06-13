---
type: feature-product
feature: notes
sibling: tech.md
parent: ../../product.md
updated: 2026-06-10
---

# Feature: Notes — Product

The `note` verb captures and recalls knowledge as plain Markdown files in the Tolaria vault. A note is the unit of memory: writing one is remembering, and (via the search feature) finding one is recalling. Notes are authored and consumed by both the human operator and agents.

Brain **owns writes** (atomic temp+`os.replace`, hash IDs, wikilink resolution) and cheap **direct reads** (`get`/`list` by frontmatter scan), and **coexists** with Tolaria on the same folder: Tolaria owns the vault and its Git history and exposes its own filesystem-direct MCP (`read_note`/`search_notes`/`list_notes`/`get_vault_context`/…) over these very files. Brain writes vault-native files Tolaria can also serve, delegates heavy *recall* to the search feature (`indexed`), and may call Tolaria's read tools — but never depends on Tolaria running, because a note is just a Markdown file Brain can read directly. Brain's `note` commands surface only notes it authored (those carrying a brain `id`); Tolaria- or hand-authored files without one are left for Tolaria to serve.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The `brain note` command surface; note frontmatter (`id`, `type`, `title`, `tags`, `owner`, `created`, `updated`, `related`); body authoring (append, sections); wikilink resolution and the `related` graph |
| **Does not own** | The vault, its Git history, and the Tolaria MCP's read tools (Tolaria coexists on the same folder); task fields and lifecycle (tasks feature); ranking and retrieval (search feature); the watcher/index (daemon feature); cross-cutting schema/ID/storage contracts (root tech.md) |

---

## Requirements

### Requirement: Create notes

The system SHALL create a note as a single Markdown file with schema-valid YAML frontmatter and a content-hash ID, routed by `type`: the default `note` lands directly under `notes/`; `log`, `decision`, and `reference` land under `notes/logs/`, `notes/decisions/`, `notes/references/` respectively (canonical `type → folder` map in root [tech.md](../../tech.md)).

#### Scenario: Capture a decision

- **Given** an operator runs `brain note new "Use CLID fallback for Lufthansa GDS" --type decision --tags ndc,lufthansa`
- **When** the command completes
- **Then** a `.md` file is written under `notes/decisions/` with a `n-` hash ID, `created`/`updated` timestamps, and the given title and tags

#### Scenario: Headless create requires a body source

- **Given** an agent runs `brain note new "..."` over MCP or with `--json` and supplies neither `--body` nor `--file`
- **When** the command runs on a non-interactive (machine) path
- **Then** it exits `2` (usage) rather than launching `$EDITOR`; an interactive terminal session still opens `$EDITOR`

### Requirement: Amend notes without rewriting

The system SHALL append content (optionally under a named heading and/or with a timestamp) and update fields (`title`, `type`, body, and additive `+tag`/`-tag` or replacement tag lists) in place, bumping `updated`. Concurrent appends/updates to the same note MUST be serialized so no edit is lost.

#### Scenario: Append a finding under a heading

- **Given** an existing note `n-a3f2`
- **When** an agent runs `brain note append n-a3f2 "Confirmed for J/C class" --section "Follow-ups" --timestamp`
- **Then** the content is appended under a `Follow-ups` heading (created if missing) with an ISO timestamp, and `updated` is bumped

#### Scenario: Two agents append at once

- **Given** two agents append to the same `log` note `n-b8c1` simultaneously
- **When** both commands run
- **Then** both appended blocks are present (the writes serialize via an `O_EXCL` lock) and neither is lost

### Requirement: Retrieve and list notes

The system SHALL return a note by `<id|slug>` (frontmatter + a body preview by default; `--full`, `--meta-only`, or `--related` on request) and MUST list notes filtered by tags, owner, type, and recency (`--since`).

#### Scenario: Read a note by ID

- **Given** a note `n-a3f2` exists
- **When** `brain note get n-a3f2` runs
- **Then** its frontmatter and the first 200 characters of the body are returned

### Requirement: Resolve wikilinks

The system SHALL resolve `[[Title]]`, `[[n-id]]`, and `[[t-id]]` references in note bodies to canonical IDs and MUST maintain the `related` array as the resolved, deduplicated set, leaving unresolvable links verbatim. Resolution lives in `core/wikilinks.py` and runs against an on-disk title/ID scan, so it works identically with the daemon down (the daemon only caches the result); a task ID in `related` is the note↔task provenance link.

#### Scenario: Link by title

- **Given** a note body contains `[[NDC config for Lufthansa]]` and a note with that title exists as `n-a3f2`
- **When** the note is saved
- **Then** `n-a3f2` appears in the note's `related` array and the body text is preserved

### Requirement: Delete notes

The system SHALL delete a note by `<id|slug>`, prompting for confirmation unless `--force` is given. Deletion is a hard removal of the file — there is no soft-delete or trash.

#### Scenario: Guarded delete

- **Given** a note `n-a3f2` exists
- **When** `brain note delete n-a3f2 --force` runs
- **Then** the file is removed and `n-a3f2` no longer appears in `note list` or search

Reference requirements as R1, R2, R3, R4, R5 in the feature plan's Requirements Trace.

## User Experience

```
$ brain note new "Use CLID fallback for Lufthansa GDS" --type decision \
    --tags ndc,lufthansa,flights --owner flights-agent \
    --body "CLID gives better seat-map coverage than NDC on LH metal."
n-a3f2  notes/decisions/n-a3f2.md

$ brain note append n-a3f2 "Confirmed: only needed for J/C class" --section "Follow-ups" --timestamp
$ brain note update n-a3f2 --tags +confirmed
$ brain note get n-a3f2 --json        # machine-readable; --json is available on every command
```

## Non-Goals

- Soft-delete, trash, or recovery — `note delete` is a hard, guarded removal.
- Full-text ranking or semantic recall — that is the search feature.

## Prior Art & Inspiration

**Coexisting tool — [Tolaria](https://github.com/refactoringhq/tolaria):** the files-first, git-first Markdown vault Brain runs over. Its standalone MCP (`read_note`/`create_note`/`append_to_note`/`edit_note_frontmatter`/`link_notes`/`search_notes`/`list_notes`/`get_vault_context`/`delete_note`) maps near-1:1 to Brain's note surface, but documents **no** atomicity/`O_EXCL`/hash-ID/wikilink guarantees — which is exactly why Brain owns writes and only delegates/aligns reads. Brain sits *beside* Tolaria on one folder, not subordinate to it.

- **Conceptual anchor — [Basic Memory](https://github.com/basicmachines-co/basic-memory):** AI and humans write the *same* Markdown files, linked by relations. **Borrow:** files as the shared substrate; links as connective tissue. **Differ:** Brain keeps a flat, deduplicated `related` set, not a knowledge graph, and adds no DB store.
- **Surface reference — [mcp-obsidian](https://github.com/MarkusPfundstein/mcp-obsidian):** confirms the canonical note-tool surface (get/list/search/append/patch/delete), though it hard-depends on the Obsidian app — a poorer fit than Tolaria's headless, filesystem-direct server.
