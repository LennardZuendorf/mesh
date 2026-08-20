---
type: feature-plan
feature: team-awareness
sibling: tech.md
parent: ../../plan.md
updated: 2026-08-16
---

# Feature: Team Awareness — Implementation Plan

Eleven units. Units 1–3 are the three faces of the core gap (inbound derivation, task append,
release) and unblock everything else; 4–6 are discovery and attribution; 7 composes them into the
warm-start payload; 8–9 are small standalone fixes; 10 is the MCP sweep; 11 is the gated compound
step that touches root specs.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate.** This feature is independent of the deferred root `tasks-graph` row — release needs
no dependency graph. It shares files with two sibling tracks and must be sequenced against them at
the whole-feature level, not by unit edges:

| Sibling | Overlap | Handling |
|---|---|---|
| **core-hardening** | owns the warm-index / 503-stub decision; `index/warm.py` | Unit 1 moves the vault walk into `storage/files.py` so this feature works either way. Land unit 1 and unit 6 **after** that decision, or coordinate the single-line move. No unit here depends on the warm index existing. |
| **agent-usability** | MCP `instructions` / `Field` descriptions, the Skill, `--json` flag placement | Unit 10 adds tools and params only; their prose descriptions belong to that track. |

---

## Requirements Trace

| ID | Requirement | Unit | Status |
|---|---|---|---|
| R1 | [Inbound awareness](product.md#requirement-inbound-awareness) | team-awareness/1 | DONE |
| R2 | [A task body MUST be appendable](product.md#requirement-a-task-body-must-be-appendable) | team-awareness/2 | DONE |
| R3 | [A claim MUST be releasable](product.md#requirement-a-claim-must-be-releasable) | team-awareness/3 | DONE |
| R4 | [Abandoned and live work MUST be findable](product.md#requirement-abandoned-and-live-work-must-be-findable) | team-awareness/4 | DONE |
| R5 | [Available work MUST be orderable](product.md#requirement-available-work-must-be-orderable) | team-awareness/5 | DONE |
| R6 | [Activity rows MUST carry identity](product.md#requirement-activity-rows-must-carry-identity) | team-awareness/6 | DONE |
| R7 | [`session-start` MUST be able to see the team](product.md#requirement-session-start-must-be-able-to-see-the-team) | team-awareness/7 | DONE |
| R8 | [Edits MUST be attributable](product.md#requirement-edits-must-be-attributable) | team-awareness/8 | DONE |
| R9 | [Duplicate titles MUST warn at creation](product.md#requirement-duplicate-titles-must-warn-at-creation) | team-awareness/9 | DONE |
| R10 | [MCP parity](product.md#requirement-mcp-parity) | team-awareness/10 | DONE |

---

## Key Technical Decisions

1. **The inversion is the notify primitive.** Inbound = `{N : X ∈ N.related}`, derived on demand
   from frontmatter already on disk. No store, no schema change, no daemon. → tech.md § Inbound
   derivation.
2. **Extend the resolver, not the vocabulary.** A task is a note with lifecycle fields, so append
   gains an id space rather than shards gaining a verb. → tech.md § `task append`.
3. **Release ships now; the graph stays deferred.** Release is the missing half of an already-built
   atomic primitive, not graph work — but it is scheduled in the root plan's Phase-3 row, so
   unpicking it needs sign-off (unit 11). → product.md § The argument.
4. **`claimed_at` rejected.** `updated` is the better liveness signal once tasks are appendable; a
   timestamp nobody enforces is a frontmatter key nobody reads. Invariant 3 stays clean. → product.md
   § Open Questions.
5. **Ordering is a sort key, not a schema constraint.** A strict `priority` Literal would make legacy
   tasks silently vanish from every listing (`list_tasks` skips validation failures). Tolerant read,
   canonical write. → tech.md § Priority ordering.
6. **The vault walk moves to `storage/`** so the headline path is immune to core-hardening's
   warm-index decision. → tech.md § The vault walk moves to `storage/`.
7. **`--force` is not exposed over MCP.** Owner identity is not an authorization boundary; breaking a
   peer's claim stays a human/CLI action. → tech.md § `task release`.

---

## Unit IDs

Units are `team-awareness/n`. Cite in commits (`feat(task): team-awareness/2 append to task bodies`).

---

### team-awareness/1 — Inbound derivation + `graph --direction`

**Goal:** Reverse `related`. One vault walk producing `{N : X ∈ N.related}`, wired into the existing
BFS as a direction, exposed as `shards graph <id> --direction in|out|both`. Move the shared vault
walk into `storage/files.py` on the way.

**Requirements:** R1

**Dependencies:** — (whole-feature gate: land after core-hardening's warm-index decision)

**Files:** `storage/files.py`, `core/context.py`, `core/lenses.py`, `cli/session.py`

**Test scenarios:**

- A note linking `[[t-184G]]` is returned by an inbound query on `t-184G`; the forward query on the
  note is unchanged.
- Inbound edges are emitted source→target; `--direction both` returns the union with each node and
  edge once (cycle + diamond cases, mirroring `tests/memory/test_graph_query.py`).
- A malformed `.md`, a foreign file with no shards id, and a `related` entry naming a deleted id are
  all skipped without aborting the walk.
- An unresolvable seed exits 3 in every direction.
- Same output with the daemon stopped as with it running.

**Verification:** new tests in `tests/memory/test_graph_query.py` (direction cases) plus a new
`tests/memory/test_inbound.py`; `uv run pytest -q` green, `uv run ty check src/` clean; `git grep`
shows exactly one vault-walk implementation.

---

### team-awareness/2 — `task append`

**Goal:** `shards task append <t-id> <text> [--section] [--timestamp]` — body append with no
lifecycle change, reusing the note body helpers.

**Requirements:** R2

**Dependencies:** —

**Files:** `core/tasks.py`, `cli/task.py`

**Test scenarios:**

- Appending to a `claimed` task leaves `status=claimed`, leaves the file in `tasks/open/`, bumps
  `updated`.
- Appending text containing `[[n-…]]` adds that id to the task's `related` (this is the R1 delivery
  path end-to-end).
- Appending to a `done` task writes the text, keeps `status=done`, keeps the file in `tasks/done/`,
  and adds no second `## Outcome`.
- `--section` creates the heading when absent and appends under it when present; unknown frontmatter
  keys round-trip byte-identically.
- A concurrent append and finish on one id serialize under the entity lock with no lost write.
- No second body-append implementation: `core/tasks.py` imports the helpers from `core/notes.py`.

**Verification:** new `tests/tasks/test_append.py`; round-trip assertion mirroring
`tests/notes/test_append_update.py`; `uv run pytest -q` green.

---

### team-awareness/3 — `task release` + reassignment

**Goal:** `shards task release <t-id> [--force] [--note <text>]` and `task update --owner <agent>`.

**Requirements:** R3

**Dependencies:** team-awareness/2 (`--note` composes with append)

**Files:** `core/tasks.py`, `cli/task.py`

**Test scenarios:**

- Holder releases → `claimed_by` null, `status=open`, file unmoved, `updated` bumped; a second
  release is a no-op that writes nothing.
- Non-holder releases → exit 4 naming the holder, file untouched; with `--force` the claim clears.
- Release on a `done`/`cancelled` task is an idempotent no-op.
- Release → claim by a second agent succeeds (the handoff loop closes).
- `update --owner` with a valid `[tasks].collections` identity rewrites `owner`; an unknown identity
  exits 2 and writes nothing.
- `--note` produces exactly one appended block via the unit-2 path.

**Verification:** new tests in `tests/tasks/test_claim.py` (release branches beside the claim
branches) and `tests/tasks/test_new_update.py` (owner reassignment); `uv run pytest -q` green.

---

### team-awareness/4 — Stale, multi-status, board visibility

**Goal:** `task list --stale <dur>`, comma-separated `--status`, `claimed_by` in human rows, and a
per-agent breakdown in `shards status`.

**Requirements:** R4

**Dependencies:** —

**Files:** `core/tasks.py`, `cli/task.py`, `cli/admin.py`

**Test scenarios:**

- A task last updated four days ago is returned by `--stale 2d` and hidden by `--since 2d` (the
  inversion, asserted against the same fixture).
- `--status open,claimed` returns both sets; `--status open` behaves exactly as before.
- `--stale` and `--since` combined yield the band; neither implies a status.
- Human rows show the holder, or `-` when unclaimed; `--json` output is unchanged.
- `shards status` reports per-agent open/claimed/stale-claim counts and is absent from the MCP tool
  list.

**Verification:** new tests in `tests/tasks/test_list_cancel.py` and the admin/status suite under
`tests/daemon/`; `uv run pytest -q` green; text-format change reflected in updated assertions, not
worked around.

---

### team-awareness/5 — Priority ordering + `--available`

**Goal:** canonical `high|normal|low` ordering as a sort key, write-boundary validation, and a
single "takeable work" filter.

**Requirements:** R5

**Dependencies:** team-awareness/4 (same filter/sort tail in `list_tasks`)

**Files:** `core/tasks.py`, `cli/task.py`

**Test scenarios:**

- `--sort priority` orders high → normal → low → unprioritized, `created` ascending within a rank.
- A pre-existing task with a free-form `priority` value still appears in every listing, sorts last,
  and round-trips untouched.
- Creating or updating a task with a value outside the vocabulary exits 2 naming the allowed values
  and writes nothing.
- `--available` excludes claimed tasks and any `open` file carrying a stale `claimed_by`, and
  defaults to priority order.
- `--owner ""` is still rejected; no unowned state is introduced.

**Verification:** new tests in `tests/tasks/test_list_cancel.py` + `tests/tasks/test_schema.py`
(tolerant read); `uv run pytest -q` green, `uv run ty check src/` clean.

---

### team-awareness/6 — Identity on activity rows

**Goal:** `owner` and `claimed_by` on every activity row; owner/mine filters read the row, disk only
as fallback.

**Requirements:** R6

**Dependencies:** — (whole-feature gate: land after core-hardening's warm-index decision)

**Files:** `index/warm.py`, `core/activity.py`

**Test scenarios:**

- Warm-index rows and `scan_recent` rows carry identical keys including `owner`/`claimed_by`; notes
  carry `claimed_by: null`.
- A row payload still survives `json.dumps` over the socket (no `datetime` leaks).
- `--owner` / `--mine` filtering produces the same results as today with no per-row disk read.
- A row **without** an `owner` key (older peer) still filters correctly via the disk fallback.
- Daemon down: same rows, same keys.

**Verification:** `tests/memory/test_recent_activity.py` extended with a row-shape assertion and a
legacy-row fallback case; `uv run pytest -q` green.

---

### team-awareness/7 — `session-start --team` / `--owner` + mentions

**Goal:** the warm-start payload stops being solipsistic: widen the activity half, honour `--owner`,
and deliver inbound mentions of my nodes, each entry tagged with a `reason`.

**Requirements:** R7, R1

**Dependencies:** team-awareness/1 (inbound), team-awareness/6 (row identity)

**Files:** `core/lenses.py`, `cli/session.py`

**Test scenarios:**

- Four-identity fixture: a note by research-agent linking a task flights-agent holds appears in
  flights-agent's payload, marked `reason=mention`, ordered after tasks and before activity.
- `--team` widens the activity half to every agent while the task half stays the caller's
  open/claimed queue.
- `--owner <agent>` produces that agent's payload from another agent's session; the flag is honoured
  on both sides of the command name.
- Mentions by me of my own nodes are excluded; mentions outside the window are excluded; dedupe by id
  still holds across all three sections.
- `--meta-only --json` (the hook invocation) stays valid and body-free.
- Daemon down: identical payload, no infrastructure notice.

**Verification:** new tests in `tests/memory/test_session_hook.py`; `uv run pytest -q` green; the
end-to-end check is the simulation case — flights-agent's `session-start` surfaces the reply that
today is findable only via search at threshold ≤ 0.4.

---

### team-awareness/8 — Attribution on stamps

**Goal:** `--timestamp` renders `<iso> — <agent>`; `## Outcome` / `## Cancelled` name the acting
agent.

**Requirements:** R8

**Dependencies:** team-awareness/2 (task append shares the helper)

**Files:** `core/notes.py`, `core/tasks.py`

**Test scenarios:**

- A timestamped note append by notes-agent on a flights-agent note names notes-agent; the ISO
  token remains the first field on the line.
- Finish and cancel sections name the acting agent; idempotent re-runs add nothing.
- No frontmatter key is added; a file's frontmatter is byte-identical to the pre-change output apart
  from `updated`.

**Verification:** updated assertions in `tests/notes/test_append_update.py` and
`tests/tasks/test_finish.py`; `uv run pytest -q` green.

---

### team-awareness/9 — Duplicate-title warning at create

**Goal:** non-blocking collision warning naming the existing id — stderr for the CLI, a `warnings`
key for MCP.

**Requirements:** R9

**Dependencies:** —

**Files:** `core/notes.py`, `cli/note.py`, `cli/task.py`

**Test scenarios:**

- Creating a second note with an existing exact title still creates it and warns naming the prior id;
  `--quiet` suppresses the line and `--json` never carries it in the payload.
- Creating a second task with an existing task title warns the same way.
- A note and a task sharing a title do not warn.
- Titles differing only by case/whitespace follow the same exact-match rule the wikilink title index
  uses — asserted, not incidental.
- The CI startup guard stays green (the added frontmatter read is the cost under test).

**Verification:** new tests in `tests/notes/test_new.py` and `tests/tasks/test_new_update.py`;
`uv run pytest -q` green; startup-guard job green (`tests/test_startup_guard.py`).

---

### team-awareness/10 — MCP parity sweep

**Goal:** every capability above reachable from an agent, plus the missing `shards_session_start`.

**Requirements:** R10

**Dependencies:** team-awareness/1, team-awareness/2, team-awareness/3, team-awareness/4,
team-awareness/5, team-awareness/6, team-awareness/7, team-awareness/9

**Files:** `mcp/server.py`

**Test scenarios:**

- Tool enumeration lists `shards_session_start`, `shards_task_append`, `shards_task_release`, and the
  new `direction` / `stale` / `available` / `sort` / `owner` params with correct annotations.
- `shards_task_release` has no `force` parameter; both delete verbs, `daemon`, `reindex` and `status`
  stay withheld.
- `shards_note_new` / `shards_task_new` return `warnings` on a title collision and an empty list
  otherwise.
- Every tool routes through the same `core` function the CLI calls — no parallel implementation.
- The module docstring's Phase-3 `task_release` deferral note is corrected.

**Verification:** `tests/memory/test_tools.py` extended (enumeration + annotation assertions);
`uv run pytest -q` green, `uv run ruff check .` clean.

---

### team-awareness/11 — Compound (GATED — root specs, human sign-off)

**Goal:** promote this feature's cross-cutting outcomes into the root layer. **Requires explicit
sign-off before any root file is touched** (root AGENTS.md § 0, § write invariants).

**Requirements:** — (no new behaviour)

**Dependencies:** team-awareness/1 – team-awareness/10 DONE

**What is being asked for, precisely:**

| Root file | Change | Why it needs sign-off |
|---|---|---|
| `product.md` § Requirements | inbound/backlinks named as a shipped read-only lens beside `graph` / `project` | extends the lens list under the three-verb thesis |
| `product.md` § Phases | `release` removed from the Phase-3 row and recorded as shipped | **the substantive ask** — release is currently scheduled in the deferred Phase-3 row |
| `product.md` § Requirements 3 | v1 task model gains `release` beside `claim`/`finish`/`cancel`/`list` | same |
| `tech.md` § Contracts / Task adds | `priority` vocabulary + ordering; no new frontmatter key | records that invariant 3 was held |
| `tech.md` § Implemented surfaces | inbound derivation, `task append`, `release`, activity-row keys, session-start sections | contract surface |
| `tech.md` § Shared primitives | vault walk relocated to `storage/files.py` | one-mechanic-one-home statement |
| `design.md` § Global flags | `--stale`, `--available`, `--direction`, `--team` | flag vocabulary |
| `plan.md` § Sequence | add the feature row; note that the deferred `tasks-graph` row keeps the graph work | scheduling |
| `lessons.md` | candidate lesson: *a derived field read in one direction is half a feature — the inverse read is usually the missing capability, not a new primitive* | lessons are gated |

**Verification:** `bash .agents/skills/spec/scripts/validate.sh` clean; root plan row reflects
reality; feature folder archived then deleted before the branch merges.

---

## Dependencies

| Unit | Blocks | Blocked by |
|---|---|---|
| team-awareness/1 | 7, 10, 11 | — |
| team-awareness/2 | 3, 8, 10, 11 | — |
| team-awareness/3 | 10, 11 | team-awareness/2 |
| team-awareness/4 | 5, 10, 11 | — |
| team-awareness/5 | 10, 11 | team-awareness/4 |
| team-awareness/6 | 7, 10, 11 | — |
| team-awareness/7 | 10, 11 | team-awareness/1, team-awareness/6 |
| team-awareness/8 | 11 | team-awareness/2 |
| team-awareness/9 | 10, 11 | — |
| team-awareness/10 | 11 | team-awareness/1 – team-awareness/9 |
| team-awareness/11 | — | team-awareness/1 – team-awareness/10 |

Units 1, 2, 4, 6 and 9 are independent and parallelizable. Units 1 and 6 additionally sit behind the
whole-feature gate against core-hardening (see § Feature gate).

---

## Progress

| Unit | Status | Evidence |
|---|---|---|
| team-awareness/1 | DONE | `8854319` feat(context): invert related for backlinks via graph --direction |
| team-awareness/2 | DONE | `f9aa448` feat(task): append text to a task body |
| team-awareness/3 | DONE | `d001d3a` feat(task): add release + owner reassignment |
| team-awareness/4 | DONE | `35f7301` feat(task): stale filter, multi-status, agent breakdown |
| team-awareness/5 | DONE | `3235de3` feat(task): priority ordering + --available filter |
| team-awareness/6 | DONE | `c0bc525` feat(activity): carry owner/claimed_by on activity rows |
| team-awareness/7 | DONE | `04e9f4b` feat(session): session-start --team/--owner + mentions; `3b692a8` fix(session): degrade note-mention target set on unset identity |
| team-awareness/8 | DONE | `06b13a8` feat(notes,tasks): attribute stamps to the acting agent |
| team-awareness/9 | DONE | `2dc6dea` feat(create): warn on duplicate title at create; `a4606d2` fix(create): mirror slug rule for duplicate-title check |
| team-awareness/10 | DONE | `9e2f995` feat(mcp): close MCP parity sweep for team-awareness |
| team-awareness/11 | GATED — NOT RUN | root-layer compound; requires human sign-off before any root spec edit (root AGENTS.md § 0, § write invariants). See the core-hardening plan's § Root follow-ups table for the promote-to-root items this unit would carry. |
