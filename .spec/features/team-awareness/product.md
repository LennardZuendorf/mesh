---
type: feature-product
feature: team-awareness
sibling: tech.md
parent: ../../product.md
updated: 2026-08-15
---

# Feature: Team Awareness — Product

Shards is excellent at keeping agents from corrupting each other's files and weak at making them
aware of each other. Isolation shipped (`O_EXCL` claim, atomic writes, sandbox, id-only task
handles); *awareness* did not. This track closes that gap **inside the three verbs** — every
capability below is a sub-command of an existing verb, a flag on an existing command, a read-only
lens, or a warning. No fourth primitive.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | Inbound (`related`-reversed) derivation and its lens surface; `task append`; `task release` + owner reassignment; stale/live-work discovery (`task list --stale`, multi-status, `claimed_by` in rows, per-agent breakdown in `status`); priority ordering + `--available`; identity on activity rows; `session-start --team` / `--owner` + the mentions section; agent attribution on timestamped appends and terminal sections; duplicate-title warning at create; the `shards_*` counterparts of all of the above plus the missing `shards_session_start`. |
| **Does not own** | MCP `instructions` / `Field` descriptions, the shipped Skill, `shards init`, `--json` flag-position consistency → **agent-usability**. Correctness bugs, the daemon warm-index/503-stub decision, dead code, duplication, spec drift, test/CI gaps → **core-hardening**. The task dependency graph (`blocks`/`blocked_by` readiness, `ready`, strict gate, unblock-cascade) → root plan's deferred `tasks-graph` row, still Phase 3. |
| **Deferred** | Claim leases / auto-release, read-state for mentions, filterable last-editor identity, an agent registry or inbox — all argued and rejected below (§ Non-Goals, § Open Questions). |

---

## The argument: awareness needs no fourth verb

Teams normally grow five primitives for this — **comment, reply, mention, ack, subscribe**. Each of
them decomposes into two operations shards almost has:

1. **Write text into a shared node.** `note append` does this for notes. Tasks are write-once after
   creation, so the coordination surface — the task — is the one thing nobody can talk on. (R2)
2. **Read what points at me.** `related` is derived solely from a node's *own* body and the BFS
   follows it forward only, so a link is a one-way broadcast into the void. Reversing that read
   turns every existing wikilink into a delivered message. (R1, R7)

| Primitive people ask for | What it actually is here |
|---|---|
| comment | `note append` / `task append` on the shared node |
| reply | another append, mentioning `[[id]]` |
| mention | a `[[n-…]]` / `[[t-…]]` link — already recorded in `related` |
| ack | an append (and, for a claim, `task release`) |
| subscribe / inbox | inbound links to nodes I own or have claimed, in `session-start` |

**The addressable unit is the node, not the agent.** In the team simulation research-agent wrote
`@flights-agent — [[t-184G]] overlaps my [[n-FEWP]]`. The `@name` is decoration; the deliverable
address is `[[t-184G]]`, a task flights-agent holds. Delivery is therefore a *derivation over
frontmatter already on disk* — no registry, no inbox, no notification store, no schema change and
no daemon dependency. That inversion **is** the notify primitive.

Held honestly: `task release` is not a new primitive, but root `product.md` currently parks
`release` in the deferred Phase-3 row. Release needs no dependency graph — it is the missing half of
an already-built atomic primitive (claim is a test-and-set; nothing can unset it). Unpicking it from
Phase 3 is a **root-plan change requiring sign-off**, gated in [plan.md](plan.md), not a quiet
smuggle. Nothing else in this track touches root product surface beyond promoting shipped lenses.

---

## Requirements

### Requirement: Inbound awareness

The system SHALL derive, for any note or task id, the set of nodes whose `related` list contains
that id, and expose it through the existing `graph` lens without a daemon and without a schema
change.

#### Scenario: A reply is findable from the mentioned node

- **Given** research-agent's note `n-9QQ2` whose body links `[[t-184G]]`
- **When** any agent runs `shards graph t-184G --direction in`
- **Then** `n-9QQ2` appears in the result, and its edge is recorded source→target (`n-9QQ2` → `t-184G`)

#### Scenario: Dangling and foreign nodes never break the walk

- **Given** a vault containing a malformed `.md`, a file owned by another tool with no shards id, and a note whose `related` names a deleted id
- **When** an inbound query runs
- **Then** those files are skipped silently and the query still returns every valid inbound node

#### Scenario: Daemon down changes nothing

- **Given** the daemon is stopped
- **When** the same inbound query runs
- **Then** the output is byte-identical to the daemon-up result

### Requirement: A task body MUST be appendable

The system SHALL let an agent append text to a task's body — progress, a question, an
acknowledgement — without changing the task's lifecycle status, without moving its file, and while
recomputing `related` so that mentions inside the appended text become deliverable.

#### Scenario: Progress on a claimed task

- **Given** `t-10NT` is `claimed` by flights-agent
- **When** flights-agent appends `blocked on [[n-FEWP]]`
- **Then** the task stays `claimed` in `tasks/open/`, `updated` is bumped, and `n-FEWP` is now in the task's `related`

#### Scenario: Appending to a finished task never resurrects it

- **Given** `t-3KP1` is `done` and lives in `tasks/done/`
- **When** an agent appends a post-mortem line
- **Then** the status stays `done`, the file stays in `tasks/done/`, and no second `## Outcome` section is written

### Requirement: A claim MUST be releasable

The system SHALL let the holder of a claim hand it back — clearing `claimed_by` and returning the
task to `open` — and MUST allow an operator to break another agent's abandoned claim explicitly,
without destroying the task as live work.

#### Scenario: Holder releases

- **Given** `t-7B4Q` is `claimed` by notes-agent
- **When** notes-agent releases it
- **Then** `claimed_by` is null, `status` is `open`, the file stays in `tasks/open/`, and a re-run is a no-op

#### Scenario: Breaking someone else's claim is explicit

- **Given** `t-7B4Q` is claimed by notes-agent and idle for four days
- **When** the operator releases it without forcing
- **Then** the command exits 4 naming the holder; **and when** the operator forces, the claim clears

#### Scenario: Ownership is reassignable

- **Given** `t-7B4Q` is owned by operator
- **When** its owner is set to research-agent (a valid `[tasks].collections` identity)
- **Then** the task's `owner` is research-agent; an unknown identity exits 2 and writes nothing

### Requirement: Abandoned and live work MUST be findable

The system SHALL surface work that has stopped moving and work that is in flight: a staleness filter
that inverts the `--since` floor, a multi-status filter so "all live work" is one call, `claimed_by`
in human task rows, and a per-agent breakdown in `shards status`.

#### Scenario: The four-day-idle claim surfaces

- **Given** `t-7B4Q` is `claimed` and untouched for four days
- **When** the operator lists claimed tasks stale for two days
- **Then** `t-7B4Q` is listed — where the existing `--since 2d` floor hides it

#### Scenario: All live work in one call

- **Given** open and claimed tasks exist
- **When** a caller filters on both statuses at once
- **Then** both sets are returned, and each human row names the holder (or `-` when unclaimed)

### Requirement: Available work MUST be orderable

The system SHALL give `priority` a canonical ordering usable as a sort key and offer a single filter
for genuinely takeable work — open and unclaimed — so an arriving agent picks the top row instead of
guessing.

#### Scenario: The ready queue

- **Given** open unclaimed tasks with priorities `low`, `high`, and none, plus one claimed task
- **When** an agent asks for available work
- **Then** the claimed task is excluded and the rows come back `high` first, unprioritized last, oldest-first within a priority

#### Scenario: A legacy priority value never hides a task

- **Given** an existing task whose `priority` is a free-form string outside the vocabulary
- **When** any task list runs
- **Then** the task is still listed (sorted last), its value round-trips untouched, and only a *new* write of that value is rejected

### Requirement: Activity rows MUST carry identity

The system SHALL include `owner` and `claimed_by` on recent-activity rows, so "who did that" costs
no extra read.

#### Scenario: One call answers who

- **Given** four agents have written to the vault
- **When** recent activity is requested as JSON
- **Then** every row names its owner (and holder, for tasks) without a follow-up `note get`

### Requirement: `session-start` MUST be able to see the team

The system SHALL let `session-start` widen beyond the caller — a team view of recent vault activity,
and an explicit other-agent view — and SHALL deliver inbound mentions of the caller's own nodes in
the warm-start payload.

#### Scenario: Three days away

- **Given** research-agent mentioned `[[t-184G]]`, which flights-agent holds, two days ago
- **When** flights-agent starts a session
- **Then** the mentioning note appears in the payload, marked as a mention, above ordinary activity

#### Scenario: Widening keeps my queue mine

- **Given** the team view is requested
- **When** the payload is produced
- **Then** the activity half spans every agent while the task half still lists only my open/claimed tasks

### Requirement: Edits MUST be attributable

The system SHALL name the acting agent in every timestamped append and in the terminal
`## Outcome` / `## Cancelled` sections, so a body edit is not silently credited to the file's
creator.

#### Scenario: Who appended

- **Given** notes-agent appends a timestamped line to a note flights-agent created
- **When** the note is read
- **Then** the stamp names notes-agent alongside the ISO instant

#### Scenario: Who finished

- **Given** flights-agent finishes a task owned by operator
- **When** the task body is read
- **Then** the `## Outcome` section names flights-agent

### Requirement: Duplicate titles MUST warn at creation

The system SHALL warn — without blocking — when a new note or task carries a title an existing
node of the same kind already uses, naming the colliding id, because silent duplication is
permanently damaging: the slug resolver refuses forever, and two agents each claim their own copy of
one piece of work.

#### Scenario: Second note with the same title

- **Given** `n-1J8J` titled "Vault sync design" exists
- **When** an agent creates another note with that exact title
- **Then** the note is still created and a warning naming `n-1J8J` reaches the caller — on stderr for the CLI, in the returned payload for MCP

### Requirement: MCP parity

Every capability this feature adds to the CLI MUST have a `shards_*` counterpart with a correct
annotation, and the missing `shards_session_start` MUST ship.

#### Scenario: An agent has the same reach as the CLI

- **Given** an MCP client connected to the shards server
- **When** it enumerates tools
- **Then** it finds session-start, task append, task release, and the inbound direction / staleness / availability parameters, each annotated read-only, idempotent, write, or destructive

---

## User Experience

```
$ shards graph t-184G --direction in
n-9QQ2  note  Overlap with the flights rewrite
n-FEWP  note  Sync notes

$ shards task list --status open,claimed --stale 2d
t-7B4Q  claimed  notes-agent    Migrate the vault watcher
t-2M8C  open     -              Draft the retry policy

$ shards task append t-10NT "waiting on [[n-FEWP]] before I continue" --timestamp
appended t-10NT

$ shards task release t-7B4Q
task t-7B4Q is claimed by notes-agent          # exit 4
$ shards task release t-7B4Q --force
released t-7B4Q

$ shards session-start
t-184G  task  Rewrite the flights parser            # my queue
n-9QQ2  note  Overlap with the flights rewrite      # mention of t-184G
n-3PDA  note  Retry policy draft                    # my recent activity
```

---

## Non-Goals

- **No fourth verb, no notification store, no inbox, no agent registry.** Awareness is derived from
  frontmatter already on disk. An agent that wants a mentionable handle keeps a note about itself
  and is linked by title — a convention, like projects, not a primitive.
- **No claim leases, no auto-release, no background reaper.** A daemon that expires claims would make
  the accelerator a writer and a gate (invariant 1). Staleness is *reported*; a human or an agent
  acts on it.
- **No read-state for mentions.** The recency window is the state; adding per-agent read marks is a
  store by another name.
- **No dependency-graph work.** `blocks` / `blocked_by` readiness, `ready`, the strict gate and the
  unblock-cascade stay in the deferred Phase-3 row. Release ships here *without* its Phase-3
  companion behaviour (surfacing newly-ready tasks).
- **No `@agent` parsing.** Plain-text handles stay inert data; `[[id]]` is the address.
- **No task title-slug resolution.** Tasks stay id-only handles by existing decision — a mention of a
  task uses `[[t-id]]`.

---

## Open Questions

1. **Mention read-state.** The mentions section is stateless and window-bounded, so a mention repeats
   for the length of the window. *Recommendation:* accept it; the window is the state. Revisit only
   if agents report churn.
2. **`claimed_at` and leases.** Rejected for v1 — `updated` is the better liveness signal once
   tasks are appendable (an append is a genuine heartbeat), and a `claimed_at` nobody enforces is a
   frontmatter key nobody reads. *Reopen trigger:* if lease-based auto-release is ever wanted, it
   needs a claim-start instant and this decision must be revisited with invariant 3 in hand.
3. **Filterable last-editor.** `--mine` / `--owner` mean *creator*, always; the attribution stamp
   makes an edit visible but not filterable. A `updated_by` frontmatter key would fix that at the
   cost of a third arm on `--mine` and a key on every file. *Recommendation:* reject for v1; the
   git-backed vault is the audit trail. *Reopen trigger:* an operator query that genuinely cannot be
   answered by stamp + inbound links.
4. **Priority vocabulary.** Fixed `high | normal | low`. *Recommendation:* keep it fixed; a
   `[tasks].priorities` config knob is configuration for its own sake.
5. **Duplicate-check cost.** The collision check adds a frontmatter-reading walk to `note new` /
   `task new`. *Recommendation:* ship the walk and let the CI startup guard arbitrate; if it
   regresses, serve the check from the warm index with the scan as fallback (never as a gate).
