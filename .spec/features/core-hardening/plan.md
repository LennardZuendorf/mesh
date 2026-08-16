---
type: feature-plan
feature: core-hardening
sibling: tech.md
parent: ../../plan.md
updated: 2026-08-16
---

# Feature: Core Hardening — Implementation Plan

Nine units. The four correctness bugs land first (1–4), then the daemon warm-index decision
(5), then the tests that prove all of it (6) and the CI that keeps it proven (7), then the
duplication cull (8), then spec reconciliation (9). The order is not negotiable in one
direction: **a refactor over a buggy reader bakes the bug in**, so every extraction waits on
the fixes it would otherwise entomb.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Starts on `cli-toolset-rework` DONE (root [plan.md](../../plan.md) Feature
Sequence — units 1–5 shipped). Depends on no other feature's units.

---

## Problem Frame

Everything here is already promised. The bugs (1–4) are unconditional: each one makes a
documented behaviour false today, and each is cheap and local. The daemon decision (5) is the
one architectural call — it resolves a falsified invariant and it is what the sibling
**team-awareness** lenses will land on, so it goes before the refactor and before their lens
work. Tests (6) come before the refactor, not after, because the refactor's only safety net is
a suite that actually exercises the claims. CI (7) follows 6 so the coverage floor is set at
the post-gap number, not the pre-gap one. The cull (8) is last of the code units and is
deliberately small — most candidates are **rejected** by the recorded DRY filter. Spec
reconciliation (9) closes with the gated root edits raised, not applied.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Foreign-file tolerance is symmetric](product.md#requirement-foreign-file-tolerance-is-symmetric) | core-hardening/1 |
| R2 | [Creates are race-safe](product.md#requirement-creates-are-race-safe) | core-hardening/2 |
| R3 | [Infrastructure failures exit like failures](product.md#requirement-infrastructure-failures-exit-like-failures) | core-hardening/3 |
| R4 | [Exit codes live on the exception classes](product.md#requirement-exit-codes-live-on-the-exception-classes) | core-hardening/3 |
| R5 | [Body matches are returned at default configuration](product.md#requirement-body-matches-are-returned-at-default-configuration) | core-hardening/4 |
| R6 | [Vault-health counts cover the whole vault](product.md#requirement-vault-health-counts-cover-the-whole-vault) | core-hardening/4 |
| R7 | [The daemon accelerates the reads it claims to](product.md#requirement-the-daemon-accelerates-the-reads-it-claims-to) | core-hardening/5 |
| R8 | [The spec matches the shipped surface](product.md#requirement-the-spec-matches-the-shipped-surface) | core-hardening/9 |
| R9 | [Concurrency and sandbox claims are tested](product.md#requirement-concurrency-and-sandbox-claims-are-tested-under-their-own-conditions) | core-hardening/6, core-hardening/7 |
| R10 | [One mechanic, one home — where the shapes match](product.md#requirement-one-mechanic-one-home--where-the-shapes-match) | core-hardening/8 |

---

## Key Technical Decisions

1. **Fixes before refactors.** Units 1–4 are behaviour fixes over the current tree; unit 8
   extracts nothing until they land. Extracting a shared reader from a broken reader makes the
   break canonical. → tech.md § Bugs and their fixes.
2. **Wire the list-shaped daemon reads; delete the point reads and passthroughs.** `task.list`,
   `note.list`, `vault.status`, `search.tag_pull` get real warm handlers; `note.get`,
   `task.get`, `search.query`, `index.reindex` and their dead client methods are removed. The
   warm index cannot make a point read or a subprocess hop faster; it can make an O(vault) walk
   disappear. → tech.md § Decision — the daemon's warm index.
3. **Fix the unpinned rule, not the pinned numbers.** The fallback tier matrix
   (1.0/0.8/0.6/0.4) is pinned in root tech.md § Implemented surfaces; the
   threshold-application rule is not. So the threshold applies only when explicitly set. →
   tech.md § B5.
4. **Creates get a per-kind allocator lock**, not an `exclusive` mode threaded through
   `atomic_write` — the latter re-encodes a per-caller difference into the one shared write
   primitive. → tech.md § B2.
5. **The DRY filter rejects more than it accepts.** `_is_tty` stays duplicated (documented
   per-module test seam, root tech.md:88); the substring fallback stays on disk (its body tier
   needs contents the index does not hold). → tech.md § Duplication.
6. **Root spec corrections are raised, not applied.** This branch edits only
   `.spec/features/core-hardening/` and the two feature-layer specs carrying dead gates. → §
   Root follow-ups.

---

## Unit IDs

Units are `core-hardening/n` — assigned once, never renumbered. Cite in commits
(`fix(tasks): core-hardening/1 route task readers through read_post`).

---

### core-hardening/1 — Safe reader symmetry

**Goal:** Every vault-Markdown parse in `src/` goes through `storage/files.read_post`; the
hand-rolled `frontmatter.loads(path.read_text())` idiom disappears.

**Requirements:** R1

**Dependencies:** —

**Files:**

```
src/shards/core/tasks.py     # :431 get_task, :472 list_tasks (reads); :227, :283, :336 (writes)
src/shards/core/notes.py     # :259 append, :322 update (writes)
```

**Test scenarios:**

- A `.md` file in `tasks/open/` that raises `OSError` on read is skipped by `task list`,
  `session-start`, and `status` — all three exit `0` with every other task present.
- The same vault produces identical skip behaviour from `note list` (symmetry assertion, one
  parametrised test over both verbs).
- A malformed-YAML task is not-found (exit `3`) on `task get`, and is skipped — not fatal — on
  `task claim`/`finish` of a *different* task.

**Verification:** `git grep -n "frontmatter.loads" src/` returns exactly one hit
(`storage/files.py`). New cases in `tests/tasks/` and a shared parametrised symmetry test;
`uv run pytest -q` green, `uv run ty check src/` clean.

---

### core-hardening/2 — Race-safe creates

**Goal:** `create_note` and `create_task` hold a per-kind allocator lock across id allocation
and write, closing the `_id_taken` → `atomic_write` TOCTOU.

**Requirements:** R2

**Dependencies:** core-hardening/1

**Files:**

```
src/shards/core/tasks.py     # :163-195 create_task -> under hold(<tasks allocator lock>)
src/shards/core/notes.py     # :210-218 create_note -> under hold(<notes allocator lock>)
src/shards/storage/locks.py  # allocator lock path helper (no semantics change)
```

**Test scenarios:**

- N separate **processes** create entities whose id candidates collide (id generation stubbed
  to a fixed hash); afterwards N distinct files exist with N distinct ids and N distinct
  bodies — no content lost.
- The extension loop is exercised: the second creator's id is longer than the first's.
- Create still succeeds with the daemon down (no new dependency).

**Verification:** New `tests/storage/test_create_race.py` using `multiprocessing`; the test
fails against `HEAD~1` and passes after. `uv run pytest -q` green.

---

### core-hardening/3 — Infrastructure-failure boundary and exit codes on exceptions

**Goal:** Domain exceptions carry their exit code; one boundary mapper converts them plus
`LockError` and `OSError` into codes and one-line messages. No user-reachable traceback.

**Requirements:** R3, R4

**Dependencies:** core-hardening/1

**Files:**

```
src/shards/core/errors.py                 # code attribute on every domain exception
src/shards/storage/locks.py               # LockError.code = 4
src/shards/cli/_errors.py                 # new — the single mapper
src/shards/cli/{note,task,session,admin,search}.py  # apply mapper; drop numeric literals
src/shards/mcp/server.py                  # same mapping surfaced as tool errors
```

**Test scenarios:**

- A pre-held, live, fresh lock on an entity makes `task claim` / `note append` exit `4` with a
  stderr line naming the entity — asserted on exit code **and** on the absence of `Traceback`
  in stderr.
- A write into a read-only vault exits `1` with an `io error:` line, not a traceback.
- Every existing exit-code assertion in the suite still passes (`2` validation, `3` not-found,
  `4` claim conflict) — the mapper is behaviour-preserving for the already-handled cases.
- `git grep -nE "typer.Exit\((2|3|4)\)" src/shards/cli/` returns nothing.

**Verification:** New `tests/cli/test_exit_codes.py` covering the matrix (one row per
exception class × surface); `uv run pytest -q` green.

---

### core-hardening/4 — Fallback recall and whole-vault link health

**Goal:** Body-only matches return at default config; the dangling-link count covers tasks as
well as notes.

**Requirements:** R5, R6

**Dependencies:** core-hardening/1

**Files:**

```
src/shards/index/fallback.py    # threshold applies only when explicitly set
src/shards/schemas/config.py    # record explicitly-set [search] keys at load
src/shards/core/wikilinks.py    # :113-135 find_dangling scans notes/ + tasks/
src/shards/cli/_output.py       # :64 timestamp -> Z  (drift row, same touch)
src/shards/core/search.py       # :165 timestamp -> Z
```

**Test scenarios:**

- Default config, no `indexed`: a note whose **body** contains the query is returned by
  `search` (the current `search "eTA"` → `[]` case now returns the hit).
- `--threshold 0.7` on the same vault excludes the body-only hit — the flag still filters.
- An explicit `threshold = 0.65` in `config.toml` behaves as today (explicit is honoured).
- A task body with a title-form wikilink matching no note appears in `shards status`'s dangling
  count; an id-form link does not.
- Every emitted timestamp matches `...Z` across `_output`, `core/search`, and the model dumps.

**Verification:** New `tests/index/test_fallback_threshold.py`; extended `tests/search/`;
dangling case in the wikilinks tests. `uv run pytest -q` green.

---

### core-hardening/5 — Daemon warm-index wiring and stub cull

**Goal:** `task.list`, `note.list`, `vault.status`, `search.tag_pull` get real handlers bound
to `VaultIndex` and real production callers; `note.get`, `task.get`, `search.query`,
`index.reindex` and their dead client methods are deleted. Invariant 7 becomes true.

**Requirements:** R7

**Dependencies:** core-hardening/1 (the list predicates must read through the safe reader
before they are shared with the index path)

**Files:**

```
src/shards/daemon/server.py     # :53-63 _STUB_METHODS culled; four warm handlers added
src/shards/daemon/client.py     # :177-203 note_get/task_get removed; list methods get callers
src/shards/index/warm.py        # VaultIndex list/status projections (no bodies)
src/shards/index/tagpull.py     # :190-204 served from the index when the daemon is up
src/shards/cli/task.py          # task list routes through DaemonClient
src/shards/cli/note.py          # note list routes through DaemonClient
src/shards/cli/admin.py         # status routes through DaemonClient
src/shards/cli/session.py       # session-start / project lens reuse the same client reads
src/shards/mcp/server.py        # same routing for the mirrored tools
```

**Test scenarios:**

- With a warmed daemon, `task list` and `note list` return the identical result the disk path
  returns for the same vault — asserted by running the whole read suite twice, daemon-up and
  daemon-down, against one fixture vault.
- With a warmed daemon, `task list` performs **no** full-vault parse (the disk walker is
  patched to raise; the command still succeeds).
- Daemon down / hung / mid-request kill: every wired read falls back and produces the same
  result plus one stderr notice.
- `vault.status` and `search.tag_pull` return the same shapes as their fallbacks.
- No method in the dispatch table raises `503`; `git grep -n '"note\.get"\|"task\.get"' src/shards/daemon/`
  returns nothing (as-corrected, core-hardening/9: the original pattern,
  `git grep -n "note_get\|task_get" src/`, false-positives on the unrelated MCP tool functions
  `shards_note_get`/`shards_task_get`, which were never removed and are not the RPC methods this
  scenario is about — re-scoped to the dotted RPC method strings in the daemon module where the
  dispatch table lives).
- Filtering/sorting/limiting is not reimplemented: `git grep` shows one implementation of each
  task/note predicate.

**Verification:** Extended `tests/daemon/`; a parametrised daemon-up/daemon-down fixture over
the existing read suites; `uv run pytest -q` green, `uv run ty check src/` clean.

---

### core-hardening/6 — Test gaps: locks, sandbox, MCP tools, daemon lifecycle

**Goal:** Cover the claims shards is built on under the conditions those claims are about.

**Requirements:** R9

**Dependencies:** core-hardening/1, core-hardening/2, core-hardening/3, core-hardening/5

**Files:**

```
tests/storage/test_locks.py      # new — cross-process claim race, stale reclaim CAS
tests/storage/test_sandbox.py    # new — four escape vectors × CLI and MCP
tests/mcp/test_tool_bodies.py    # new — every mutating tool executed against a temp vault
tests/daemon/test_lifecycle.py   # new — serve_forever, mid-request death, reconnect, EOF
tests/index/                     # new package (populated by unit 4 and here)
tests/cli/                       # new package (populated by unit 3 and here)
```

**Test scenarios:**

- N separate OS processes race one `task claim`: exactly one exits `0`, the rest exit `4`; the
  winner's `claimed_by` is durable on disk.
- A lock file owned by a **dead** PID is reclaimed and the claim proceeds — `_pid_alive`
  returns `False` in at least one test (impossible in the current same-PID barrier tests).
- Two reclaimers racing one stale lock yield exactly one holder — `locks.py:90-91` and `:96-97`
  execute.
- Rejected: an absolute out-of-vault path supplied as an id; a symlinked directory component
  mid-path; `..` inside a filename; a hardlink — each via the CLI **and** the MCP surface.
- Every mutating MCP tool body runs and its on-disk effect is asserted (not its registration
  metadata).
- `serve_forever` runs and is cancelled cleanly (socket unlinked); a daemon killed mid-request
  leaves the client on its fallback; a reply truncated before its newline
  (`client.py:121`) degrades instead of raising.

**Verification:** `uv run pytest -q --cov=src` reports `storage/locks.py` and
`storage/sandbox.py` at 100% of their reachable branches, `mcp/server.py` ≥ 90% (from 62%),
and every new test fails when its fix/guard is reverted.

---

### core-hardening/7 — CI hardening

**Goal:** The gate proves what it claims: coverage floor, real Python matrix, typed tests,
working packaging, fresh lockfile, and no phantom tooling.

**Requirements:** R9

**Dependencies:** core-hardening/6 (the coverage floor is set at the post-gap number)

**Files:**

```
.github/workflows/ci.yml     # permissions, concurrency, timeout-minutes, 3.11/3.12/3.13 matrix,
                             # ty over src+tests, uv sync --locked, uv build + wheel smoke,
                             # duplicate startup-guard step removed
pyproject.toml               # [tool.coverage.report] fail_under; ty root -> src + tests
.pre-commit-config.yaml      # new (ruff, ruff-format, ty) — or the dev dep is dropped
```

**Test scenarios:**

- CI fails on a deliberately-introduced coverage drop below the floor.
- CI fails on a type error introduced in `tests/`.
- The wheel job installs into a clean venv and `shards --version` / `shards --help` succeed —
  a broken console-script entry point turns the gate red.
- `uv sync --locked` fails on a stale `uv.lock`.
- `pre-commit run --all-files` succeeds (or `pre-commit` is absent from `pyproject.toml`).

**Verification:** A CI run on this branch green across the full matrix; each negative case
above demonstrated once on a scratch commit and recorded in § Progress.

---

### core-hardening/8 — Duplication cull under the DRY filter

**Goal:** Merge only what shares one shape; delete what is dead; record every rejection.

**Requirements:** R10

**Dependencies:** core-hardening/1, core-hardening/2, core-hardening/3, core-hardening/4,
core-hardening/5, core-hardening/6

**Files:**

```
src/shards/cli/__main__.py          # delete _LazyCommandGroup (:80-110, :143-153, tables :38-76)
                                    # and the typer._click.core import (:29)
src/shards/storage/files.py         # one parameterised vault walk
src/shards/core/{notes,tasks,wikilinks}.py, src/shards/index/{warm,tagpull}.py  # consume it
src/shards/core/search.py           # route_search() — the CLI/MCP duplicate collapses here
src/shards/cli/search.py            # :86-111 -> route_search
src/shards/mcp/server.py            # :168-190 -> route_search
src/shards/cli/_output.py           # one output-flag read; one session emit helper
src/shards/cli/{admin,session}.py   # :190-195 / :72-82 and the four emit blocks route through it
src/shards/cli/{note,task}.py       # _list_obj shared; _is_tty stays per-module (rejected)
src/shards/index/indexed_client.py  # delete register_hook (:324-330), reindex alias (:319-321)
src/shards/cli/admin.py             # :21-23 stale docstring
src/shards/cli/{note,task}.py       # :4-6 stale "sibling units" docstrings
```

**Test scenarios:**

- Full suite passes unmodified in behaviour — this unit changes no observable output.
- `shards --help` lists the same commands in the same order after `_LazyCommandGroup` is gone.
- `tests/test_startup_guard.py` still passes (verified: the guard asserts only that watchdog /
  fastmcp / rich / pydantic stay unimported, none of which the sibling verb modules leak).
- Cold-start time is within the recorded CI startup-guard budget after the lazy group is
  removed (~6ms of a ~70ms startup is the measured cost).
- The delete tests still monkeypatch `_is_tty` per module — the seam is intact.
- `git grep` shows one vault walk, one search router, one output-flag reader.

**Verification:** `uv run pytest -q` green with **zero** test edits beyond import paths; the CI
startup guard passes; `uv run ruff check .` and `uv run ty check src/ tests/` clean. The
rejection table in [tech.md](tech.md) § Duplication is updated with the as-built verdicts.

---

### core-hardening/9 — Spec reconciliation

**Goal:** Close every spec-vs-shipped disagreement in the feature layer; raise the root ones
as gated follow-ups.

**Requirements:** R8

**Dependencies:** core-hardening/5 (the daemon decision determines what the method table says)

**Files:**

```
.spec/features/cli-toolset-rework/product.md   # :87 "No project verb" — the verb shipped
.spec/features/cli-toolset-rework/plan.md      # :190 "--help shows no new verb"
.spec/features/shards-rebrand/tech.md          # :70 mypy -> ty
.spec/features/shards-rebrand/product.md       # :74 mypy -> ty
.spec/features/shards-rebrand/plan.md          # :153, :214 mypy -> ty
.spec/features/core-hardening/*.md             # as-built corrections
```

**Test scenarios:**

- Every verification command named in any `.spec/features/**` file executes against the current
  toolchain (no `mypy`).
- No feature spec forbids a verb that `shards --help` prints.
- The root follow-up list below is complete: every gated row has an owner and a landing point.

**Verification:** `bash .agents/skills/spec/scripts/validate.sh` clean; a scripted sweep of
`uv run <cmd>` for every command string in `.spec/features/**` exits `0`;
`git diff --stat .spec/` shows **no** change to root `.spec/*.md`.

---

## Dependencies

| Unit | Blocks | Blocked by |
|---|---|---|
| core-hardening/1 | 2, 3, 4, 5, 6, 8 | — |
| core-hardening/2 | 6, 8 | core-hardening/1 |
| core-hardening/3 | 6, 8 | core-hardening/1 |
| core-hardening/4 | 8 | core-hardening/1 |
| core-hardening/5 | 6, 8, 9 | core-hardening/1 |
| core-hardening/6 | 7, 8 | core-hardening/1, 2, 3, 5 |
| core-hardening/7 | — | core-hardening/6 |
| core-hardening/8 | — | core-hardening/1, 2, 3, 4, 5, 6 |
| core-hardening/9 | — | core-hardening/5 |

---

## Cross-feature notes

Not unit edges — whole-feature sequencing for the root [plan.md](../../plan.md) Feature
Sequence, recorded so the three concurrent tracks do not collide.

- **team-awareness depends on core-hardening/5.** `--stale`, `session-start --team`, and
  priority ordering are all list-shaped reads over task metadata. If unit 5 lands first they
  land on the warm index; if it does not, they land on a disk walk that unit 5 then reworks.
  Recommendation: gate team-awareness's lens units on core-hardening/5, or accept the rework.
- **agent-usability owns the flag contract.** The global `--owner` drop (parsed then ignored
  everywhere except `task claim` and `recent-activity`) is a bug *given* a global contract;
  agent-usability states the contract, core-hardening ships the plumbing under their
  requirement ID. Neither spec duplicates the flag surface.
- **agent-usability owns `search --health` and `note get --related`.** Both are surface
  reductions this feature only flags.
- **No shared files at unit level** between the three tracks except `cli/session.py`
  (core-hardening/8 output helper vs. team-awareness lenses) and `cli/__main__.py`
  (core-hardening/8 lazy-group removal vs. agent-usability flag wiring). Land core-hardening/8
  last, after both siblings, or expect a merge.

---

## Root follow-ups

Gated: root `.spec/*.md` is not editable from this branch. Each row lands at compound with
user approval.

| Root file | Correction | Source unit |
|---|---|---|
| `plan.md:29` | shards-rebrand `🛠 in progress` → `✅ DONE` (its own plan marks units 1–5 DONE; tree renamed) | 9 |
| `plan.md` Sequence | Add rows for `core-hardening`, `team-awareness`, and `agent-usability` (root plan.md's Sequence table and `children:` frontmatter only list `shards-rebrand`/`cli-toolset-rework` — all three newer tracks are missing, not just core-hardening). `core-hardening` is 8/9 done (unit 8 outstanding); `team-awareness` is 10/11 done (unit 11 is the gated root-compound step itself); `agent-usability` is 8/8 done. | 9 |
| `tech.md:91` | Reword to match :105 — codes live on the exception classes, the boundary maps them once | 3 |
| `tech.md:134` | RPC method table: drop `note.get`, `task.get`, `search.query`, `index.reindex`; mark the rest warm-served | 5 |
| `tech.md:54` | Invariant 7 becomes true — no text change needed if 5 ships; delete the invariant if it does not | 5 |
| `tech.md` § Contracts, Config row | Document the `[core].path` → `tolaria_path` alias (`schemas/config.py::load_config`, alias applied at lines ~176-178 as of this unit — cite the function, not a line number, since it has already drifted once) | 9 |
| `tech.md` § Contracts | Document that both delete verbs hard-`unlink` (stance currently lives only in `AGENTS.md` § 6) | 9 |
| `product.md:23` / `tech.md:137` | Remove `build-context` in favour of `graph` (~390 LOC incl. `tests/memory/test_build_context.py`) — **product decision required** | 8 |
| `lessons.md` | Record: "a fixed lesson is not a fixed codebase — apply a cross-cutting rule to every reader/writer at fix time, and re-verify it in the unit that claims the fix" | compound |

---

## Progress

| Unit | Status | Evidence |
|---|---|---|
| core-hardening/1 | DONE | `480e20a` fix(core): route task readers through read_post |
| core-hardening/2 | DONE | `d374029` fix(core): hold allocator lock across create id+write |
| core-hardening/3 | DONE | `c2f5ff0` fix(cli): map domain/OSError exceptions to exit codes once; `a22a87f` fix(mcp): add ValueError branch to the tool-error mapper (review round 1) |
| core-hardening/4 | DONE | `049b804` fix(search): recall body hits at default config; dangling covers tasks; `a749bd6` test(mcp): cover shards_search threshold resolution; dedupe with CLI (review round 1) |
| core-hardening/5 | DONE | `bb9758f` feat(daemon): wire warm list reads and cull stubs; `fb69f13` fix(daemon): scope warm rows and never gate on a bad reply (review round 1); `b73572a` docs(daemon): correct stale admin and daemon-test prose |
| core-hardening/6 | DONE | `ff99ccb` test(core-hardening): cover locks, sandbox, mcp bodies, daemon lifecycle |
| core-hardening/7 | DONE | `1efaee0` ci(core-hardening): harden CI gate |
| core-hardening/8 | DONE | `0986a2c` refactor(cli): cull duplication under the DRY filter |
| core-hardening/9 | DONE | this unit — spec reconciliation |

---

## Open Questions

1. **Is `build-context` removable?** Product call, not technical — it is a shipped lens in root
   product.md. Recommendation: remove; `graph`'s JSON strictly contains it. Blocked on the
   gated root edit. **Still open** — see § Root follow-ups.
2. **Does core-hardening/8 land before or after the sibling tracks?** It touches
   `cli/__main__.py` and `cli/session.py`, which agent-usability and team-awareness also edit.
   Recommendation: last, to avoid a three-way merge on files two other branches are rewriting.
   **Moot as of this unit** — per § Progress, both sibling tracks (team-awareness,
   agent-usability) are fully DONE, so unit 8 lands after them by construction; no merge risk
   remains to sequence around.
3. **Coverage floor value.** Set at the post-unit-6 measurement, not at today's 93%.
   Recommendation: floor at the measured number minus 1 point, ratcheted upward only.
   **Settled in core-hardening/7** (`1efaee0`) — `pyproject.toml`'s
   `[tool.coverage.report] fail_under = 97`, ~1.2 points below the measured 98.17–98.20%,
   exactly per this recommendation.
