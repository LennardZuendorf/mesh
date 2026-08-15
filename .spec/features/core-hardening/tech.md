---
type: feature-tech
feature: core-hardening
sibling: product.md
parent: ../../tech.md
updated: 2026-08-15
---

# Feature: Core Hardening — Architecture

Nine workstreams over the existing tree: route every reader through the one safe reader, put
the create path under the lock every other write already holds, give the CLI boundary an
infrastructure-failure arm, fix the fallback threshold that suppresses body matches, count
dangling links across the whole vault, resolve the daemon's warm-index promise, put exit codes
on the exception classes, cull the duplication that survives the DRY filter, and close the
test/CI gaps around the claims this project is built on. No new modules except test packages.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/shards/core/tasks.py            # 5 hand-rolled frontmatter.loads -> read_post; lock create
src/shards/core/notes.py            # 2 hand-rolled frontmatter.loads -> read_post; lock create
src/shards/storage/files.py         # read_post stays the one reader (no change expected)
src/shards/storage/locks.py         # LockError gains an exit code; reclaim CAS gets tests
src/shards/core/errors.py           # new — or per-module: `code` attribute on domain exceptions
src/shards/cli/_errors.py           # new — one boundary decorator/handler mapping code + OSError
src/shards/cli/{note,task,session,admin,search}.py  # boundary uses the mapper; literals removed
src/shards/index/fallback.py        # threshold application rule (tier matrix values unchanged)
src/shards/core/wikilinks.py        # dangling scan covers tasks/ as well as notes/
src/shards/daemon/server.py         # warm handlers for the list-shaped reads; stub cull
src/shards/daemon/client.py         # dead point-read methods removed; list methods get callers
src/shards/index/warm.py            # VaultIndex gains the list/status projections
src/shards/index/tagpull.py         # tag-pull served from the warm index when the daemon is up
src/shards/cli/__main__.py          # _LazyCommandGroup removal (~60 LOC + typer._click import)
src/shards/cli/_output.py           # single output-flag read; session lenses emit through it
src/shards/index/indexed_client.py  # drop register_hook + reindex alias (dead)
src/shards/cli/admin.py             # stale module docstring; --health note
src/shards/mcp/server.py            # no surface change — tool bodies get tests

tests/storage/                      # new package — locks (cross-process), sandbox vectors
tests/index/                        # new package — fallback thresholds, tagpull warm path
tests/cli/                          # new package — exit-code boundary matrix
tests/mcp/                          # new package — every mutating tool body
tests/daemon/                       # serve_forever, restart/reconnect, EOF-before-newline
.github/workflows/ci.yml            # permissions, concurrency, timeout, matrix, build, --locked
pyproject.toml                      # coverage fail_under; pre-commit dep resolved
.pre-commit-config.yaml             # new — or the dev dep is dropped
```

---

## Bugs and their fixes

### B1 — Task-side readers bypass the safe reader

`core/tasks.py::get_task` (:431) and `::list_tasks` (:472) hand-roll
`frontmatter.loads(path.read_text())` catching only `yaml.YAMLError`.
`storage/files.py::read_post` (:75–90) already guards `OSError` **and** `yaml.YAMLError` and is
what the note side, `tagpull`, `activity`, `wikilinks`, and `scan_recent` all use. An
unreadable path therefore tracebacks out of `task list`, `session-start`, and `status` while
`note list` survives the same vault — a verbatim regression of the recorded lesson *"Foreign-file
tolerance must be symmetric across every reader"*.

The same bypass sits on five write paths: `core/notes.py:259` (append), `:322` (update),
`core/tasks.py:227` (update), `:283` (claim), `:336` (terminate).

**Fix.** Every one of those seven call sites becomes `read_post`. Read paths treat `None` as
skip (list) or not-found (get). Write paths treat `None` as the target being unreadable →
the domain not-found error, which the boundary already maps. `frontmatter.loads` must appear
in `src/` only inside `read_post`; a `git grep` for it elsewhere is the unit's gate.

### B2 — Creates have no lock

`core/tasks.py::create_task` (:163–195) and `core/notes.py::create_note` (:210–218) are the
only write paths that never enter `storage/locks.py::hold`. The sequence is: `_id_taken`
scans the vault → `generate_*_id` extends on collision using that same predicate →
`atomic_write` `os.replace`s. Nothing serializes the gap. Ids are 4 Crockford base-32
characters (~1.05M space; birthday collisions become likely around ~1.2k entities), and the
collision-extension loop rides the racy predicate, so two concurrent colliding creates both
pass `_id_taken` and the second `os.replace` silently destroys the first file. Rare,
unbounded damage, and it contradicts "all writes are atomic and idempotent".

**Fix.** Hold a **creation lock** across allocate-and-write. The per-entity lock path cannot
be used (the id does not exist yet), so the lock is a per-kind allocator lock —
`notes/.locks/_create.lock` and `tasks/.locks/_create.lock` — held from `_id_taken` through
`atomic_write`. Alternative considered and rejected: keeping the scan lock-free and making the
write itself exclusive via `O_EXCL` on the target file. It is narrower and cheaper, but
`atomic_write` is the one shared write primitive and threading an `exclusive=True` mode
through it re-encodes a per-caller difference into a shared helper — the failure mode
lessons.md § *"DRY only when the callers share one shape"* names. The allocator lock keeps
`atomic_write` untouched.

Contention cost is bounded: creates are short, the lock is held for one scan plus one write,
and `hold` already implements the bounded wait-retry.

### B3 — `LockError` and `OSError` reach no boundary

`storage/locks.py` raises `LockError` at :121 and :133. No CLI handler catches it — the
`except` inventory across `cli/*.py` covers `NoteNotFoundError`, `AmbiguousSlugError`,
`TaskNotFoundError`, `ClaimConflictError`, `SeedNotFoundError`, `ProjectNotFoundError`,
`ValidationError`, `ValueError`, and (in `admin.py`) `FileNotFoundError` /
`subprocess.CalledProcessError`. A live contended lock therefore hangs out the wait budget
(~15s, observed) and then tracebacks. `OSError` is equally unhandled: ENOSPC tracebacks with
exit 1, indistinguishable from a crash.

**Fix.** One boundary mapper in `cli/_errors.py`, applied at every command entry:

| Raised | Exit | Message shape |
|---|---|---|
| `LockError` | `4` | `busy: <entity id> is locked by another process` |
| `OSError` | `1` | `io error: <strerror> (<path>)` |

`LockError` maps to `4` (claim-conflict) because it *is* the generalized conflict signal — the
same "someone else holds this entity" condition `ClaimConflictError` reports, discovered one
layer down. It is not a validation error (`2`) and not a missing entity (`3`). See § Open
Questions if `4` should be reserved strictly for the durable claim.

### B4 — Exit codes are literals, not exception attributes

Root [tech.md](../../tech.md):105 states *"Codes live on the domain exception classes; the CLI
boundary maps them once."* No exception class carries a code; every handler hardcodes
`typer.Exit(2)` / `(3)` / `(4)`. The spec self-contradicts :91 ("each CLI handler maps its
domain exceptions to at the boundary") in the same document.

**Fix.** Give every domain exception a class-level `code: int`. The `cli/_errors.py` mapper
reads it. Handlers keep their per-target message (which is genuinely per-caller and must not
be pulled into the helper — that is the recorded DRY filter) but stop repeating the number.
Root tech.md:91 is then reworded to match :105 as a gated follow-up.

### B5 — Body search is dead at default configuration

`index/fallback.py:48` scores a body-only match `0.4`. `schemas/config.py:52` defaults
`[search].threshold` to `0.65`, and `fallback.py:118` drops everything below it. So with
`indexed` absent — the supported and common first-run state — body matches are never returned.
Verified: `search "eTA"` → `[]`; `search "eTA" --threshold 0.4` → the hit.

**Fix.** The fallback applies the threshold **only when the caller sets it explicitly**
(`--threshold`, or an explicit `[search].threshold` in the config file); with no explicit
value it uses its own floor, the lowest tier (`0.4`). Rationale: the tier matrix
(1.0 / 0.8 / 0.6 / 0.4) is *pinned* in root [tech.md](../../tech.md) § Implemented surfaces,
whereas the threshold-application rule is not — so fix the unpinned rule, not the pinned
numbers.

Alternative recorded and rejected: retune the matrix to sit above `0.65` (e.g.
1.0 / 0.9 / 0.8 / 0.7). It preserves tier ordering but changes a contract root tech.md pins,
and it squeezes the useful `--threshold` range into a 0.05 band.

Implementation note: `SearchConfig.threshold` needs an "explicitly set" signal. `msgspec`
gives no `fields_set`, so the load path records which `[search]` keys were present in the TOML
(the same place `[core].path` aliasing already happens, `schemas/config.py:100–102`).

### B6 — Dangling-link count covers notes only

`core/wikilinks.py::find_dangling` (:113–135) iterates `_iter_note_files` (:47). Task bodies
carry wikilinks too — `resolve_wikilinks` is called on every task write — so `shards status`
reports a dangling count that silently excludes half the corpus.

**Fix.** `find_dangling` walks the whole vault corpus (`notes/**` + `tasks/{open,done}/`),
reusing the shared iterator (§ Vault walks). The title index stays note-only (titles resolve
to notes by contract); only the *scan* for link targets widens.

---

## Decision — the daemon's warm index (item 7)

### Finding

`daemon/server.py:53–63` reserves nine methods; eight are permanent `503` stubs. Only `ping`
and `activity.recent` (:138, wired at config-ful startup) have real handlers.
`daemon/client.py:177–203` exposes `note_get` / `note_list` / `task_get` / `task_list`, all
four of which have **zero production callers** — only tests. So `task list` full-scans and
YAML-parses the vault while `VaultIndex` holds that exact parsed frontmatter in RAM. This
falsifies root [tech.md](../../tech.md):11 ("CLI and MCP are thin socket clients") and
invariant 7 (:54), and it is the regression lessons.md records as *fixed* under *"One
mechanic, one home — and read from the warm index"*.

### Decision: WIRE the list-shaped reads, DELETE the rest

Not a blanket "wire everything" and not a blanket "delete it all". Per method:

| Method | Verdict | Why |
|---|---|---|
| `ping` | keep | Liveness; already real. |
| `activity.recent` | keep | Already real, already warm-served. |
| `task.list` | **wire** | The expensive one — O(vault) walk + YAML parse per invocation, and the index holds every field `list_tasks` filters on. Called by `task list`, `session-start`, `status`, the project lens, and (soon) team-awareness's lenses. |
| `note.list` | **wire** | Same shape, same cost, same index. |
| `vault.status` | **wire** | Counts + freshness + dangling + lock count; all but the lock count are index projections. Today `status` walks the vault three separate times. |
| `search.tag_pull` | **wire** | `index/tagpull.py:190–204` re-walks the whole vault to filter on tags/type/owner/status — metadata the index already holds. It is a deterministic metadata filter with no body cost, so the warm answer is exact, not approximate. |
| `note.get` / `task.get` | **delete** | A point read is one `open()` on a path the id already determines. The warm index holds frontmatter, **not bodies**, so a warm `get` would still hit disk for the body — a socket round-trip bought nothing. Both client methods are dead; both go. |
| `search.query` | **delete** | Ranking lives in the `indexed` subprocess. The daemon adds a hop, not warmth. |
| `index.reindex` | **delete** | `shards reindex` shells out to `indexed full_rebuild` directly; the daemon is not in that path and should not be. |

### Why wire rather than delete wholesale

Deleting all eight is the smaller diff, and it is wrong here:

1. **The daemon's justification collapses without it.** Root [tech.md](../../tech.md) §
   Performance is built on "heavy work lives in the warm daemon". A daemon that only watches
   files and answers one lens is a watcher, and the spec would have to be rewritten down to
   that — a much larger, gated, spec-facing change than the wiring.
2. **The cost is asymmetric.** Wiring is four handlers over an index that is already built,
   already warmed before the socket binds, and already correct — plus swapping four call sites
   onto client methods that already exist with fallbacks already written and tested. Deleting
   means editing root tech.md:11, :54, and the method table, and repudiating a recorded lesson.
3. **Degradation is already solved.** `DaemonClient.call` (:150) catches `(OSError,
   json.JSONDecodeError)` and treats `503` as fallback-eligible, per the recorded lesson
   *"Daemon-down fallback must catch the whole transport-failure surface"*. Every wired method
   keeps its existing file-op fallback verbatim, so invariant 1 (accelerates, never gates) is
   preserved by construction and provable by running the whole suite with the daemon down.
4. **It is the growth path.** team-awareness's lenses (`--stale`, `session-start --team`,
   priority ordering) are all list-shaped reads over task metadata. They land on the warm
   index if this unit ships first, or on a disk walk that needs rework if it does not.

And the deletions are not concessions — they are the same judgement applied honestly. A
method the index cannot make faster should not pretend to be an accelerator, and a client
method with no caller is dead weight the tests are propping up.

### Contract

Handlers return the wire shape the existing fallbacks already produce, so the call sites do
not branch:

```python
# daemon/server.py — added handlers, all bound to the server's VaultIndex
"task.list"       (params: filters)  -> {"entries": [ <task frontmatter dict + path>, ... ]}
"note.list"       (params: filters)  -> {"entries": [ <note frontmatter dict + path>, ... ]}
"vault.status"    (params: {})       -> {"notes": n, "tasks": n, "dangling": [...], "locks": n, "fresh": iso}
"search.tag_pull" (params: filters)  -> {"results": [ <SearchResult dict>, ... ]}
```

Filtering/sorting/limiting stays in **one** place — the existing pure predicates in
`core/tasks.py` / `core/notes.py` — applied to index rows on the daemon side and to disk rows
on the fallback side. The predicate must not be reimplemented against the index; that would be
the copy-drift this whole feature exists to remove.

`VaultIndex` gains the projections needed for those rows and nothing more. Bodies stay off the
index (memory, and no reader needs them for a list).

### Consequence for the fallback scanners

`index/fallback.py:103` and `index/tagpull.py:190–204` both re-walk the vault even with the
daemon up. Tag-pull is fixed above (it is a pure metadata filter). The **substring fallback is
deliberately left on disk**: its body tier needs file contents the index does not hold, so a
warm version would either carry bodies in RAM or split into a warm-metadata pass plus a disk
body pass — two shapes behind one name, which the DRY filter rejects. Recorded as deferred,
not overlooked.

---

## Duplication — the DRY filter applied

Applying lessons.md § *"DRY only when the callers share one shape"* to every candidate.
Rejections are load-bearing: they are the reason this feature does not grow.

| # | Candidate | Verdict | Reason |
|---|---|---|---|
| 10 | `_LazyCommandGroup` (`cli/__main__.py:80–110`, :143–153, tables :38–76) | **Delete** | ~60 LOC of private-typer-internals plumbing (`typer._click.core` import at :29) buying ~6ms of a ~70ms startup. Verified it is **not** what protects the startup guard: importing the sibling verb modules leaks none of watchdog / fastmcp / rich / pydantic, which is the entirety of what `tests/test_startup_guard.py:39` asserts. Cost is a private-API coupling to typer; benefit is under measurement noise. |
| 11 | Five vault walks (`core/notes.py:96`, `core/tasks.py:98`, `core/wikilinks.py:47`, `index/warm.py:48`, `index/tagpull.py:63`) | **Merge, parameterised** | All five are "yield `*.md` under these roots". `warm.py`'s narrower task glob is load-bearing for reconcile — it becomes a **parameter** (which roots, recursive or not), not a discriminator inside a merged body. Passes the filter: one shape, one call signature, no per-caller branch. |
| 12 | Search routing duplicated CLI↔MCP (`cli/search.py:86–111` ≡ `mcp/server.py:168–190`) | **Merge** | Byte-equivalent routing logic. `core/search.py:15–17` deliberately stopped short of it; that stop is the drift. One `route_search()` in `core/search.py`; both surfaces call it and keep only their own rendering. |
| 13 | Output-flag read ×3 (`_output.py:29–39`, `admin.py:190–195`, `session.py:72–82`); four session lenses hand-rolling json/quiet/text emit (`session.py:122–138`, :171–190, :205–217, :246–250) | **Merge the flag read; merge the emit block** | The flag read is one shape (`ctx.obj` → three booleans) — trivially shared. The four emit blocks differ only in the payload and the text renderer, which is one callable argument, not a strategy object with branches — the same collapse that made `emit_mutation` work. |
| 14 | `indexed_client.register_hook` (:324–330), `reindex` alias (:319–321) | **Delete** | `register_hook` has no caller — the daemon inlines its own lambda at `server.py:137`. `reindex` is a one-line alias for `full_rebuild`. Dead code, not duplication. |
| 15 | `_is_tty` / `_list_obj` duplicated in `cli/note.py` and `cli/task.py` | **`_list_obj` merge; `_is_tty` REJECT** | `_list_obj` is one shape. `_is_tty` **stays duplicated**: the delete tests monkeypatch it per module by import path, which is precisely why `refuse_delete_if_non_interactive` takes `tty` as an argument (`_output.py:70–81`). Root [tech.md](../../tech.md):88 already records this as a deliberate per-module seam. Merging it would break the tests and erase a documented decision. |
| 9 | `build-context` ≡ `graph` (`core/context.py:83` `_bfs`; `build_context` is `_bfs(...)[0]` at :139–155) | **Merge — gated** | Two commands, two MCP tools, two test suites, ~390 LOC (incl. `tests/memory/test_build_context.py`, 317 LOC) over one traversal. Passes the filter cleanly — `graph`'s JSON already contains `build-context`'s entire output. But `build-context` is a shipped lens named in root [product.md](../../product.md) and root [tech.md](../../tech.md) § Session lenses, so removal needs a **gated root edit**: specified here, raised as a follow-up, not executed on this branch. |
| 16 | `search --health` vs `shards status`; `note get --related` | **Reject (out of scope)** | Both are user-facing surface changes. Owned by agent-usability. Flagged, not acted on. |

---

## Test gaps

The repo is green — 678 tests, 93% coverage. These are **gaps, not failures**.

| Area | Gap | Closure |
|---|---|---|
| `mcp/server.py` | 62% — worst in repo. 13 of 16 tool bodies never execute; **every mutating tool is untested**. `tests/memory/test_tools.py` mostly asserts registration metadata. | New `tests/mcp/` package: invoke every tool body against a temp vault; assert the on-disk result, not the signature. |
| `storage/locks.py` | 86%, and the uncovered lines are exactly the race branches the module exists for — `_reclaim_if_stale` CAS retries (:90–91, :96–97). Existing "concurrency" tests are thread barriers in **one PID** (`tests/tasks/test_claim.py:352`), so `_pid_alive` is always `True` and the stale path is structurally unreachable. | New `tests/storage/test_locks.py`: N-process claim race via `multiprocessing`/`subprocess` — exactly one winner, rest conflict; a lock file written by a **dead** PID is reclaimed; two reclaimers racing produce one holder (the CAS). |
| `storage/sandbox.py` | 3 direct tests. Untested vectors: absolute out-of-vault path via a CLI/MCP id, symlinked directory component mid-path, `..` inside a filename, hardlink. | New `tests/storage/test_sandbox.py`: one case per vector, driven through **both** the CLI and MCP entry points, not just the helper. |
| `daemon/` | `serve_forever` (`server.py:172–179`) never invoked; daemon dying mid-request and restart/reconnect untested; `client.py:121` EOF-before-newline untested. | Extend `tests/daemon/`: run `serve_forever` in a task and cancel it; kill mid-request and assert fallback; truncate a reply at EOF. |
| `indexed` | Every invocation is subprocess-mocked — nothing asserts against the real binary's contract, so upstream drift is invisible (root tech.md marks the drift risk "Resolved"). | Opt-in contract test, skipped when the binary is absent. Never a hard CI dependency: `indexed` absence is a supported runtime state. |
| Packages | No `tests/storage/`, `tests/index/`, `tests/cli/`, `tests/mcp/`. Locks and sandbox have **no owning test file**. | Create all four; the suite layout mirrors `src/shards/`. |

---

## CI gaps

`.github/workflows/ci.yml` is a single job on a single Python.

| Gap | Fix |
|---|---|
| No coverage gate — `pyproject.toml:56–57` declares no `fail_under` | `fail_under` at the current measured floor; ratchet, never lower |
| Single 3.11 matrix though `requires-python = ">=3.11"` | Matrix 3.11 / 3.12 / 3.13 |
| `pre-commit` is a declared dev dep (`pyproject.toml:27`) with **no config file** | Add `.pre-commit-config.yaml` (ruff + ruff-format + ty) or drop the dep — no phantom tooling |
| Tests never type-checked — `[tool.ty.environment] root = ["src"]` | `ty check src/ tests/` |
| Redundant startup-guard step (:42–43 duplicates the pytest run at :39–40) | Delete the duplicate |
| No `permissions:` / `concurrency:` / `timeout-minutes:` | Add: `permissions: contents: read`; cancel-in-progress concurrency; a timeout |
| Packaging never exercised — no `uv build` / wheel install, so a broken console script ships undetected | Build the wheel, install it into a clean venv, run `shards --version` and `shards --help` |
| No lockfile freshness check | `uv sync --locked` |

---

## Spec drift to reconcile

Each row is a disagreement between shipped code and spec text. Rows marked **gated** touch
root `.spec/*.md` and are raised as follow-ups in [plan.md](plan.md) § Root follow-ups — this
feature's branch does not edit root files.

| Drift | Evidence | Resolution |
|---|---|---|
| `shards project` ships as a top-level verb | `cli/__main__.py` `_LEAVES` vs `features/cli-toolset-rework/product.md:87` ("No `project` verb this branch") and its `plan.md:190` ("`shards --help` shows no new verb") | Correct the cli-toolset-rework spec: the verb shipped and root product.md already documents it. Editable — feature layer, not root. |
| shards-rebrand marked "in progress" | Root `plan.md:29` vs its own plan (units 1–5 DONE, tree renamed) | **Gated** — root plan row → DONE. |
| Rebrand gates demand `uv run mypy src/` | `features/shards-rebrand/tech.md:70`, `product.md:74`, `plan.md:153`, `:214` — mypy is not a dependency; the toolchain is `ty` | Rewrite those gates to `ty`. Feature layer — editable. An unrunnable gate is worse than no gate. |
| Both delete verbs unspec'd | Stance exists only in `AGENTS.md` § 6 | **Gated** — one line in root product.md Non-Goals / tech.md Contracts. |
| Per-command flag surface undocumented | Lost when the feature specs were compounded | Owned by **agent-usability**; noted here, not fixed here. |
| `[core].path` → `tolaria_path` alias undocumented | `schemas/config.py:100–102` | **Gated** — root tech.md § Contracts, Config row. |
| Timestamp format inconsistent | `Z` in model dumps vs `+00:00` in `cli/_output.py:64` and `core/search.py:165` | Pick `Z` (matches `core/tasks.py::_iso_utc` and the on-disk contract); fix the two renderers. Code fix, no root edit. |
| Stale code docs | `cli/admin.py:21–23` says the `indexed` module "is unbuilt" though it exists and is called at :322–329; `cli/note.py:4–6` and `cli/task.py:4–6` describe unlanded "sibling units" | Rewrite the docstrings. Code fix. |
| root tech.md:91 vs :105 self-contradiction on exit codes | See B4 | **Gated** — reword :91 after B4 lands. |

---

## Open Questions

1. **`LockError` → exit `4` or exit `1`?** `4` is documented as "claimed" and today means a
   *durable* claim conflict (`claimed_by` set). A contended lock is a transient conflict. Using
   `4` lets an agent retry on one code; splitting them needs a sixth code, which the exit-code
   table calls a fixed convention. Recommendation: `4`, with the message distinguishing the two.
2. **Explicit-threshold detection.** `msgspec.Struct` exposes no `fields_set`, so "did the user
   set `[search].threshold`?" must be recorded at TOML load. Recommendation: a private
   `_explicit: frozenset[str]` on `SearchConfig`, populated in `load_config` where the
   `[core].path` alias is already handled. Alternative: sentinel default (`threshold: float |
   None = None`) — cleaner, but changes a schema field's public type.
3. **Creation-lock granularity.** One allocator lock per kind serializes all creates of that
   kind. At fleet scale (dozens of agents) this could contend. Recommendation: ship the
   coarse lock — creates are sub-millisecond and `hold` already retries — and revisit only if
   measured. A finer scheme (lock the candidate id) reintroduces the TOCTOU the lock exists to
   close.
4. **Warm `vault.status` and lock counts.** Lock files are not vault Markdown, so the index
   does not track them; the status handler still stats `.locks/`. Acceptable — it is a
   directory listing, not a parse. Recorded so the next reader does not "fix" it.
