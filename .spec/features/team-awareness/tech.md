---
type: feature-tech
feature: team-awareness
sibling: product.md
parent: ../../tech.md
updated: 2026-08-16
---

# Feature: Team Awareness — Architecture

Ten additive changes across `core/`, `cli/`, `index/`, `schemas/` and `mcp/`. Nothing here adds a
store, a daemon method, or a required frontmatter key. The headline capability — inbound links — is a
pure derivation over `related`, which is already on disk; the rest are one-comparison inversions,
one-key row additions, and sub-commands hung off the existing `task` verb.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/shards/storage/files.py    # + iter_vault_md() — the shared vault walk, moved out of index/
src/shards/core/context.py     # + inbound_ids/inbound_entries; _bfs gains a direction
src/shards/core/tasks.py       # + append_task, release_task; update owner; list stale/available/
                               #   multi-status; priority rank; write-boundary priority validation
src/shards/core/notes.py       # _format_block gains the agent; title_collisions()
src/shards/core/activity.py    # owner/claimed_by filters read the row, disk only as fallback
src/shards/core/lenses.py      # re-export inbound; session_start_entries gains mentions + reason
src/shards/index/warm.py       # _entry_dict adds owner/claimed_by; imports the moved walk
src/shards/cli/task.py         # append, release, --stale, --status CSV, --available,
                               #   --sort priority, claimed_by in text rows
src/shards/cli/session.py      # graph --direction; session-start --team/--owner
src/shards/cli/note.py         # duplicate-title warning on new
src/shards/cli/admin.py        # vault_status gains a per-agent breakdown
src/shards/mcp/server.py       # shards_session_start, _task_append, _task_release, new params
```

No new module. No new dependency.

---

## Contract / API

```python
# core/context.py — the inversion. One vault walk, frontmatter only, no daemon.
def inbound_ids(config: Config, target_id: str) -> list[str]: ...
def inbound_entries(config: Config, target_id: str) -> list[dict[str, Any]]: ...

Direction = Literal["out", "in", "both"]
def graph_query(config: Config, seed_id: str, depth: int = 1,
                direction: Direction = "out") -> GraphResult: ...

# core/tasks.py — the missing halves.
def append_task(config: Config, task_id: str, text: str, *,
                section: str | None = None, timestamp: bool = False) -> Task: ...
def release_task(config: Config, task_id: str, releaser: str, *,
                 force: bool = False) -> Task: ...
def list_tasks(..., status: str | None = None,   # now CSV: "open,claimed"
               stale: str | None = None,          # inverse of --since
               available: bool = False,           # status == open and claimed_by is None
               sort: str = "updated") -> list[TaskView]: ...  # sort now accepts "priority"
def update_task(..., owner: str | None = None) -> Task: ...

# core/notes.py — non-blocking collision report.
def title_collisions(config: Config, title: str, *, kind: Literal["note", "task"]) -> list[str]: ...

# core/lenses.py
def session_start_entries(task_views, activity, mentions, *, meta_only: bool) -> list[dict]: ...
```

**Exit codes reuse the fixed convention** — release conflict is `4` (same as claim), unknown
priority / unknown owner is `2`, unresolvable id is `3`. No new codes.

---

## Implementation Detail

### Inbound derivation (R1)

`related` is a pure function of a node's own body (`core/notes.py::append_note` recomputes it on
every write; `core/tasks.py::_terminate_task` too). The inverse is therefore *computable at any
time from the same files*:

```
inbound(X) = { N ∈ vault : X ∈ N.related }
```

One walk over `notes/**` + `tasks/{open,done}/`, `read_post` per file (the shared guarded reader —
foreign/corrupt files skip silently, per the whole-corpus invariant), keep nodes whose `related`
contains the target. **Daemon-free by construction**, exactly like `build_context` / `graph_query`
today: no degradation path, no infrastructure notice, identical output daemon-up and daemon-down.

`_bfs` gains a neighbour function selected by `direction`: `out` reads `entry["related"]` (today's
behaviour, untouched); `in` calls `inbound_ids`; `both` unions them. Dedup, cycle and diamond
semantics are unchanged — the *seen* set already handles a symmetric graph.

**Edges stay direction-true.** `GraphResult.edges` are always `[source_id, target_id]` in link
direction regardless of traversal direction, so an inbound edge is emitted `(mentioner, mentioned)`.
`tree_lines()` nests by *traversal* order. This keeps `--direction both` output mergeable and keeps
the JSON contract one shape.

**Cost:** O(vault) frontmatter reads per query — the same class `shards status` already pays via
`wikilinks.find_dangling` (which walks twice). Acceptable for a lens invoked per session, not per
keystroke. If it ever becomes hot, the warm index holds every node's `related` in RAM and can serve
an identical result behind the usual never-gates fallback — an accelerator, added later, not now.

### The vault walk moves to `storage/` (dependency isolation)

`index/warm.py::_iter_vault_md` is the canonical notes+tasks walk, but `index/warm.py` is inside
**core-hardening's** decision surface (wire the warm index vs. delete the stub machinery). Inbound
derivation must not inherit that uncertainty, so the walk moves to `storage/files.py` — which `core`
already imports, and which no daemon decision touches — and `index/warm.py` imports it back. Net
behaviour identical, one implementation still (§ root tech.md *Shared primitives*), and this
feature's headline path survives either core-hardening outcome untouched.

### `task append` (R2)

A task is a note with lifecycle fields, so the resolver is extended, not the vocabulary. `note append`
fails on `t-` ids only because `core/notes.py::_iter_note_files` walks `notes/` alone.

`append_task` reuses `core/notes.py`'s body helpers (`_append_to_end`, `_append_under_section`,
`_format_block`) via the existing private-import precedent (`core/tasks.py` already imports
`_matches_tags`, `_parse_since`, `_validate_owner` from it) — **no second append implementation**.
Mechanics mirror `update_task`: resolve *inside* `hold(_lock_path(config, task_id))` so a concurrent
finish/cancel move cannot race the read, mutate `post.metadata` in place (unknown keys round-trip),
recompute `related` from the amended body, bump `updated`, `atomic_write`.

| Invariant of this verb | Why |
|---|---|
| never writes `status` | append is not a lifecycle transition |
| never moves the file | a `done` task appended to stays in `tasks/done/` |
| accepted in every state | post-mortems on finished work are legitimate; no second `## Outcome` is written |
| recomputes `related` | this is what makes an append *deliverable* — mentions inside the text become inbound links |

CLI surface: `shards task append <t-id> <text> [--section] [--timestamp]`, mirroring `note append`
flag-for-flag. Because the shared resolver now covers both id spaces, `note append <t-id>` also
resolves — accepted as a consequence of "a task is a note", not special-cased. `task append` is the
canonical, documented spelling.

### `task release` (R3)

Claim is a compare-and-set with no inverse; release is the compare-and-clear. Under the same
`O_EXCL` entity lock, resolved inside the lock, four branches mirroring `claim_task`:

| State | Result |
|---|---|
| terminal (`done`/`cancelled`) | idempotent no-op, no write |
| `claimed_by is None` | idempotent no-op, no write (already released) |
| `claimed_by == releaser` | `claimed_by → None`, `status → open`, bump `updated`, atomic write |
| `claimed_by == other` | `ClaimConflictError` (exit 4) — unless `force=True`, which performs the clear |

`open` and `claimed` both route to `tasks/open/`, so a release never moves a file. Release records
nothing in the body by default; `--note "<text>"` composes with `append_task` (R2) rather than
inventing a second body-writing path. Reassignment is `update_task(owner=...)`, validated through
the existing `_validate_owner` (`[tasks].collections`); pushing work at a peer is
`task claim <id> --owner <agent>`, which already works.

`--force` is a **speed bump and an audit affordance, not an authorization boundary** — owner identity
is trusted local input (root AGENTS.md § 6). It is deliberately **not** exposed over MCP: an agent may
release its own claim; breaking a peer's claim stays a human/CLI action.

### Stale, multi-status, board (R4)

- `--stale <dur>` inverts the existing floor: `--since` keeps `task.updated >= cutoff`, `--stale`
  keeps `task.updated < cutoff`, both parsed by the same `_parse_since`. They are conjunctive and may
  be combined (a band-pass); neither implies a status.
- `--status` becomes a comma-set membership test (`{s.strip() for s in status.split(",")}`), so a
  single value behaves exactly as today and `open,claimed` is one call.
- Human task rows become `id  status  claimed_by-or-"-"  title`. This **changes an existing text
  format** — the only backward-incompatible surface in this feature; JSON is untouched (it already
  dumps the full model).
- `cli/admin.py::vault_status` gains `agents: {identity: {owns_open, claimed, stale_claims}}`,
  computed from the task list it already walks. Human-only, like the rest of `status` — not exposed
  over MCP. `stale_claims` uses a fixed, non-configurable window
  (`cli/admin.py::_STATUS_STALE_WINDOW = "2d"`, matching the illustrative threshold below) — it is
  a summary count, not a query surface; an operator who wants a different window runs
  `task list --status claimed --stale <dur>` directly rather than reconfiguring the summary.
  (Documented here, core-hardening/9 — the constant previously had no spec anchor.)

"What is stuck" is then a *query*, not a new surface: `task list --status claimed --stale 2d`.

### Priority ordering (R5)

**Do not tighten the schema.** `list_tasks` skips any file that fails `Task.model_validate`, so
turning `priority: str | None` into a strict `Literal` would make every legacy free-form task
*silently vanish from every listing*. Instead:

- schema stays `str | None` — tolerant read, unknown values round-trip untouched;
- ordering lives in `core/tasks.py`: `{"high": 0, "normal": 1, "low": 2}`, unknown/None → `3` (sorts
  last);
- `_SORT_FIELDS` gains `"priority"`, with its own sort branch (like `title`'s): rank ascending, then
  `created` ascending — FIFO within a priority;
- the vocabulary is enforced only at the **write** boundary (`create_task` / `update_task` raise
  `ValueError` → exit 2, naming the allowed values, mirroring `unknown owner:`), so new writes are
  canonical while the vault's history stays readable.

`--available` = `status == "open" and claimed_by is None`, defaulting to `--sort priority`. It is
*nearly* `--status open` in a well-formed vault — the difference is a hand-edited or released file
carrying a stale `claimed_by`, plus the default ordering. **There is no unowned state and none is
added**: `owner` means accountability, `claimed_by` means execution, and the takeable pool is
defined by `claimed_by is null` — not by owner. `--owner ""` stays rejected.

### Identity on activity rows (R6)

`index/warm.py::_entry_dict` gains `owner` and `claimed_by` (null for notes). The existing exclusion
of `created`/`updated` is about `datetime` objects breaking `json.dumps` over the socket — strings
are safe, so this is one line and both producers (`VaultIndex.recent`, `scan_recent`) change together
because they already share the one helper.

`core/activity.py::_owner_match` then reads the row and only falls back to the disk re-read when the
`owner` key is **absent** — which is exactly what an older daemon on the other end of the socket
returns. Additive wire change, no version negotiation, no breakage in either direction.

**core-hardening dependency, stated:** if the warm index is wired, rows come from RAM; if the stub
machinery is deleted, rows come from `scan_recent`; if the daemon is down, likewise. All three
produce the same keys because `_entry_dict` is one function. Nothing in this feature gates on the
daemon (invariant 1).

### `session-start` widening and mentions (R7)

Today `mine=True` is hardcoded on both halves (`cli/session.py`) and the root `--owner` is ignored.

- `--owner <agent>` is honoured on both the leaf and the root callback via the existing `_coalesce`
  seam, and drives both halves (an operator can ask what flights-agent sees).
- `--team` drops the owner filter on the **activity half only**; the task half stays the caller's
  queue. The two flags are independent: `--team --owner X` is X's queue plus the whole team's
  activity.
- **Mentions section:** inbound links (R1) to nodes where `owner == me or claimed_by == me`,
  restricted to mentioning nodes not owned by me and updated within the same `_SESSION_SINCE`
  window (`7d`) the activity half uses. Stateless — the window is the read-state.
- Payload order becomes **my tasks → mentions → remaining activity**, deduped by id as today, and
  each entry gains a `reason` key (`"task" | "mention" | "activity"`) so a flat JSON array stays
  readable. Additive key; unknown-key-tolerant consumers are unaffected.

### Attribution (R8)

`_format_block(text, timestamp)` becomes `_format_block(text, timestamp, agent)`, rendering
`<iso> — <agent>`; `_terminate_task`'s `## Outcome` / `## Cancelled` stamp gains the same suffix.
The agent is the resolved config identity (`$SHARDS_AGENT` / `[core].agent`). The separator is an
em-dash with surrounding spaces so the leading ISO token stays trivially parseable.

Deliberately **not** done: an `updated_by` frontmatter key. It would make edits filterable but costs a
key on every file, a write in every path, and a third arm on `--mine`'s meaning — see product.md
§ Open Questions for the reopen trigger. Bodies are still never given machinery beyond the stamp
line that already existed.

### Duplicate-title warning (R9)

`wikilinks._title_index` maps exact title → id but scans `notes/` only. `title_collisions` reuses
the same shape over the walk for the requested kind (notes for `note new`, tasks for `task new`);
a note and a task sharing a title is **not** a collision — different id spaces, different resolvers.

The check is a **pre-check called by the caller**, not a side effect of `create_note`/`create_task`,
because `core` must not print and the create functions return an on-disk schema model with nowhere
to hang a warning. CLI writes one stderr line (suppressed by `--quiet`, never in the JSON payload);
**MCP returns `"warnings": ["duplicate title: n-1J8J"]` in the create result** — an agent cannot see
stderr, and this is the case that matters most (two agents each claiming their own copy of one task
is the real coordination failure; `claim` protects a file, not a piece of work).

Cost: create already walks the vault for id-collision (`_id_taken`); this adds a frontmatter read per
file to that walk. Measured by the existing CI startup guard — see product.md § Open Questions for
the fallback if the guard reddens.

### MCP parity (R10)

| Tool | Annotation | Note |
|---|---|---|
| `shards_session_start` | read-only | **missing entirely today** |
| `shards_task_append` | write | free once the resolver is extended |
| `shards_task_release` | idempotent | `force` **not** exposed |
| `shards_graph(direction=…)` | read-only | new param, default `"out"` |
| `shards_task_list(status=, stale=, available=, sort=)` | read-only | new params |
| `shards_recent_activity` | read-only | rows now carry `owner` / `claimed_by` |
| `shards_note_new` / `shards_task_new` | write | return `warnings` |
| `shards_task_update(owner=…)` | idempotent | validated identity |

The Phase-3 deferral note in `mcp/server.py`'s module docstring ("not the Phase-3 `task_release`")
is amended: release ships here, `force` does not.

---

## Degradation

| Path | Daemon down | Warm index deleted (core-hardening) |
|---|---|---|
| inbound / `graph --direction` | identical — disk-direct by construction | identical |
| `task append` / `release` | identical — `storage` primitives only | identical |
| `task list` filters | identical — direct scan | identical |
| activity rows | `scan_recent` produces the same keys | same, `scan_recent` is the only producer |
| `session-start` | composite of the above; no notice emitted | unchanged |
| duplicate check | direct walk | unchanged |

No new command gains a daemon dependency, and no read-lens in this feature has a degraded mode —
only equivalent ones.

---

## Open Questions

1. **`--direction` vs `--backlinks` spelling.** `--direction out|in|both` is one flag with three
   values and covers the merged view; a separate `--backlinks` would be a second spelling of
   `--direction in` and a per-command bespoke shape (root design.md § Rules). *Recommendation:* ship
   `--direction` only.
2. **Warm-index acceleration for inbound.** Deliberately not built. If session-start latency on a
   large vault becomes visible, add a daemon method that returns byte-identical results with the
   scan as fallback — never as a gate.
3. **Vault-walk relocation vs core-hardening.** Moving `_iter_vault_md` into `storage/files.py`
   touches a file core-hardening may also be editing. *Recommendation:* sequence this unit after
   core-hardening's warm-index decision lands, or coordinate the single-line move; the design works
   either way, only the merge order is at stake.
