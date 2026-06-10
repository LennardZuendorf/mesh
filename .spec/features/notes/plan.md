---
type: feature-plan
feature: notes
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-10
---

# Feature: Notes — Implementation Plan

Notes is the first feature and establishes the primitives every later feature reuses: the note schema, hash IDs, atomic writes, and path sandboxing. It is a closed, deliverable, testable box — `brain note` fully works over the vault with no daemon.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Starts the build (feature 1 in the root [plan.md](../../plan.md) Feature Sequence). No upstream feature. Does not depend on any other feature's units.

---

## Problem Frame

Memory must exist before anything can recall or coordinate over it. This plan stands up the shared storage and schema primitives, then layers note CRUD, section-aware appends, and wikilink resolution on top — in that order so each unit is independently testable against the vault.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Create notes](product.md#requirement-create-notes) | notes/1, notes/2 |
| R2 | [Amend notes without rewriting](product.md#requirement-amend-notes-without-rewriting) | notes/3 |
| R3 | [Retrieve and list notes](product.md#requirement-retrieve-and-list-notes) | notes/4 |
| R4 | [Resolve wikilinks](product.md#requirement-resolve-wikilinks) | notes/5 |

Every unit cites the R-IDs it satisfies. Do not renumber R-IDs.

---

## Key Technical Decisions

1. **Shared primitives land first.** Hash IDs, atomic writes, and path sandbox are built as cross-cutting `core`/`storage` modules (see root [tech.md](../../tech.md)) so tasks and search inherit them.
2. **Atomic rewrite for every edit.** Appends and updates rewrite the whole file via temp + `os.replace`; no in-place mutation.

---

## Unit IDs

Units are `notes/n` — assigned once, never renumbered. Cite IDs in commits and tests during impl (`feat(notes): notes/1 ...`).

---

### notes/1 — Schema, IDs, and atomic storage

**Goal:** Pydantic note model, hash-ID generation, and atomic write/sandbox primitives.

**Requirements:** R1

**Dependencies:** —

**Files:**

```
src/brain/schemas/note.py
src/brain/core/ids.py
src/brain/storage/files.py
src/brain/storage/sandbox.py
```

**Test scenarios:**

- A created note validates against the schema and lands under the type-derived subfolder.
- Path escapes outside `tolaria_path` are rejected.

**Verification:** `uv run pytest tests/notes/test_schema.py tests/notes/test_storage.py`

---

### notes/2 — `note new`

**Goal:** Create notes from the CLI with title, type, tags, owner, and body or `--file`.

**Requirements:** R1

**Dependencies:** notes/1

**Files:**

```
src/brain/cli/note.py
```

**Test scenarios:**

- `brain note new` writes a valid file and prints the ID and path.

**Verification:** `uv run pytest tests/notes/test_new.py`

---

### notes/3 — `note append` and `note update`

**Goal:** Section-aware append and additive/replacement field updates, bumping `updated`.

**Requirements:** R2

**Dependencies:** notes/2

**Files:**

```
src/brain/core/notes.py
src/brain/cli/note.py
```

**Test scenarios:**

- Append under a missing heading creates it; `--timestamp` prefixes an ISO line.
- `--tags +x` adds; `--tags a,b` replaces.

**Verification:** `uv run pytest tests/notes/test_append_update.py`

---

### notes/4 — `note get` and `note list`

**Goal:** Retrieve a note (preview/full/meta/related) and list with filters.

**Requirements:** R3

**Dependencies:** notes/2

**Files:**

```
src/brain/cli/note.py
src/brain/core/notes.py
```

**Test scenarios:**

- `note get` defaults to frontmatter + 200-char preview; `--full` returns the body.
- `note list --tags --since` filters correctly.

**Verification:** `uv run pytest tests/notes/test_get_list.py`

---

### notes/5 — Wikilink resolution

**Goal:** Resolve `[[...]]` to IDs and maintain the `related` set; surface dangling links.

**Requirements:** R4

**Dependencies:** notes/3

**Files:**

```
src/brain/core/wikilinks.py
```

**Test scenarios:**

- Title and ID links resolve into a deduplicated `related` array.
- Unresolvable links stay verbatim and are reported as dangling.

**Verification:** `uv run pytest tests/notes/test_wikilinks.py`

---

## Progress

| Unit | Status |
|---|---|
| notes/1 | NOT STARTED |
| notes/2 | NOT STARTED |
| notes/3 | NOT STARTED |
| notes/4 | NOT STARTED |
| notes/5 | NOT STARTED |
