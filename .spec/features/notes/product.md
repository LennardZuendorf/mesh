---
type: feature-product
feature: notes
sibling: tech.md
parent: ../../product.md
updated: 2026-06-10
---

# Feature: Notes — Product

The `note` verb captures and recalls knowledge as plain Markdown files in the Tolaria vault. A note is the unit of memory: writing one is remembering, and (via the search feature) finding one is recalling. Notes are authored and consumed by both the human operator and agents.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The `brain note` command surface; note frontmatter (`id`, `type`, `title`, `tags`, `owner`, `created`, `updated`, `related`); body authoring (append, sections); wikilink resolution and the `related` graph |
| **Does not own** | Task fields and lifecycle (tasks feature); ranking and retrieval (search feature); the watcher/index (daemon feature); cross-cutting schema/ID/storage contracts (root tech.md) |

---

## Requirements

### Requirement: Create notes

The system SHALL create a note as a single Markdown file with schema-valid YAML frontmatter and a content-hash ID, routed to a subfolder derived from its `type` (`note` | `log` | `decision` | `reference`).

#### Scenario: Capture a decision

- **Given** an operator runs `brain note new "Use CLID fallback for Lufthansa GDS" --type decision --tags ndc,lufthansa`
- **When** the command completes
- **Then** a `.md` file is written under `notes/decisions/` with a `n-` hash ID, `created`/`updated` timestamps, and the given title and tags

### Requirement: Amend notes without rewriting

The system SHALL append content (optionally under a named heading and/or with a timestamp) and update fields (`title`, `type`, body, and additive `+tag`/`-tag` or replacement tag lists) in place, bumping `updated`.

#### Scenario: Append a finding under a heading

- **Given** an existing note `n-a3f2`
- **When** an agent runs `brain note append n-a3f2 "Confirmed for J/C class" --section "Follow-ups" --timestamp`
- **Then** the content is appended under a `Follow-ups` heading (created if missing) with an ISO timestamp, and `updated` is bumped

### Requirement: Retrieve and list notes

The system SHALL return a note by `<id|slug>` (frontmatter + a body preview by default; `--full`, `--meta`, or `--related` on request) and MUST list notes filtered by tags, owner, type, and recency.

#### Scenario: Read a note by ID

- **Given** a note `n-a3f2` exists
- **When** `brain note get n-a3f2` runs
- **Then** its frontmatter and the first 200 characters of the body are returned

### Requirement: Resolve wikilinks

The system SHALL resolve `[[Title]]` and `[[n-id]]` references in note bodies to canonical IDs and MUST maintain the `related` array as the resolved, deduplicated set, leaving unresolvable links verbatim.

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
