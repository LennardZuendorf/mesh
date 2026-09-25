---
type: feature-tech
feature: project-envelope
sibling: product.md
parent: ../../tech.md
updated: 2026-09-25
---

# Feature: Project Envelope — Architecture

The envelope composes three existing membership rules — tasks already have `project`, notes
already have `related` (wikilink-backfilled), memories gain an optional `project` mirroring the
task field. No new verb, no new scan: the lens and the filters read the same rows everyone else
reads. The one model change is the memory field; the one search change is a resolved membership
set applied as an active filter on both engine branches.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)

---

## Files

```
src/model/memory.rs       # optional `project` field, declared after `scope`
src/domain/memories.rs    # create/update accept --project; no scope-enum change
src/domain/lenses.rs      # project_view_in: notes + memories sections, appended last
src/search/mod.rs         # SearchFilter.project + membership set
src/search/builtin.rs     # member-set filter on the built-in branch
src/search/indexed.rs     # active-filter pattern on the indexed branch (fetch unbounded, filter, cap)
src/cli/search.rs         # --project argument
src/cli/memory.rs         # --project on recall, new, update
src/mcp/schema.rs         # project params on search/recall/memory tools
src/mcp/tools.rs          # pass-through
tests/memory_cli.rs tests/lens_cli.rs tests/search_cli.rs tests/mcp_cli.rs
```

---

## Contract / API

### Membership

```text
envelope(P) = { tasks : task.project  == P }
            ∪ { notes : P ∈ note.related }
            ∪ { memories : memory.project == P }
```

- Notes: `related` is the stored, wikilink-backfilled list — the lens checks **containment**
  (a list predicate, not the scalar `Filter.extra` equality; computed in the lens over
  `notes::rows`, which already exist).
- Memories: `project` is a new optional raw-string field declared **after `scope`** in the
  memory model, settable on create/update through the same owner-validated write boundary.
  The `scope` enum (`shared|private`, `validate_scope`) is untouched.
- Tasks: no change — `Filter.with_extra("project", id)` raw equality is already the lens's task
  rule.

### Project resolution

`--project` / the lens seed resolve exactly as the lens does today: id-or-slug point-read;
a miss is `MeshError::ProjectNotFound` (exit 3) with candidates; a foreign file with no mesh id
stays `seed is not mesh-native` (exit 3). Both resolutions run **before any search I/O**.

### Lens output

`project_view_in` keeps its existing keys and **appends** `notes` then `memories` sections last
(same discipline as `notes_foreign` in the status payload — pinned key-order tests extend).
Entries reuse the existing lens entry shape; spaces filter (`--space`) narrows each section's
corpus as it does today.

### `--project` as an active search filter

- `SearchFilter` gains `project: Option<String>`; the query resolves it once and builds the
  member-id set from one pass over task/note/memory rows.
- Built-in branch: filter hits by membership after scoring, before `--limit`.
- Indexed branch: the existing active-filter pattern — fetch unbounded, filter by membership,
  `--limit` stays a display cap.
- `memory recall --project`: the memory-side filter is `Filter.with_extra("project", id)`
  applied in the existing `matches_filters` re-check — no search-side set needed.

### MCP

`mesh_search` and `mesh_memory_recall` gain a `project` param; `mesh_memory_new` /
`mesh_memory_update` gain one too; `mesh_project` extends automatically (same domain function).
**No tool count change** (37 → 40 belongs to [note-adoption](../note-adoption/tech.md)).

---

## Implementation Detail

- Unknown/dangling `project` values on tasks and memories remain tolerated (zero-member
  envelopes), exactly as task `project` is today — membership is a read-time join, never a
  validated reference (§ Derived state is never stored).
- `--project` composes with `--tags`/`--status`/`--owner` by simple conjunction; a conjunction
  that yields zero rows is an empty result, never an error.

<!-- merge -->
### Memories carry an optional `project`

The memory per-space additions are `kind, scope, importance, source, project, expires,
superseded_by` — `project` mirrors the task field and is how a memory joins a workstream
envelope; `scope` stays the shared/private visibility axis.
<!-- /merge -->

---

## Performance Budget

Envelope resolution is one extra rows pass per query (rows walks are milliseconds); the
membership set is O(corpus). No index changes.
