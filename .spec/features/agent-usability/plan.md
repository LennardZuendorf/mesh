---
type: feature-plan
feature: agent-usability
sibling: tech.md
parent: ../../plan.md
updated: 2026-08-16
---

# Feature: Agent Usability — Implementation Plan

Eight units across four surfaces. The MCP block, the schemas, the tag contract, the CLI contract
and onboarding are independently shippable; the plugin bundle lands last because it documents the
finished surface. Nothing here touches `core`, `storage`, or the on-disk frontmatter contract.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Independent of **team-awareness** (no shared files). Shares two files with
**core-hardening** — `cli/_output.py` (they own the reader de-duplication, this track owns the
flag contract) and `schemas/config.py` (`load_config`'s `SystemExit(2)`). Whichever track lands
first owns the shape; the other adapts. Not a blocking gate — a merge-order note.

**Root plan row:** root `.spec/plan.md` § Sequence gains an `agent-usability` row at the human
gate. Root specs are gated and are not edited from this folder.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Server instructions built from live config](product.md#requirement-server-instructions-built-from-live-config) | agent-usability/1 |
| R2 | [Self-describing tool schemas](product.md#requirement-self-describing-tool-schemas) | agent-usability/2 |
| R3 | [Tag mutation is unambiguous and non-destructive by default](product.md#requirement-tag-mutation-is-unambiguous-and-non-destructive-by-default) | agent-usability/3 |
| R4 | [Identity and recall health are readable over MCP](product.md#requirement-identity-and-recall-health-are-readable-over-mcp) | agent-usability/4 |
| R5 | [Errors are structured and actionable](product.md#requirement-errors-are-structured-and-actionable) | agent-usability/5 |
| R6 | [One uniform CLI flag contract](product.md#requirement-one-uniform-cli-flag-contract) | agent-usability/6 |
| R7 | [Help text matches validation](product.md#requirement-help-text-matches-validation) | agent-usability/6 |
| R8 | [One shipped skill, installable with its tools](product.md#requirement-one-shipped-skill-installable-with-its-tools) | agent-usability/8 |
| R9 | [A first run that ends in a working install](product.md#requirement-a-first-run-that-ends-in-a-working-install) | agent-usability/7 |

---

## Key Technical Decisions

1. **The instructions block is built from live config, not written as prose.** Only the running
   server knows the resolved identity, roster, and vault path. → tech.md § Surface A.
2. **Enum parameters are typed from `schemas/`, never re-typed.** The literal emits the JSON
   Schema enum; drift is removed structurally. → tech.md § Surface B.
3. **Additive tags by default; replace is an explicit opt-in.** A breaking, tested change,
   preferred over silent data loss. Fallback recorded. → tech.md § Tag mutation.
4. **One skill, bundled as a plugin with `.mcp.json`.** The playbook can never install without
   the tools it describes. → tech.md § Surface D.
5. **`init` is admin, not a verb** — beside `daemon` / `status` / `reindex`, withheld from MCP.
   The three-verb thesis holds. → tech.md § Onboarding.
6. **This track owns the flag *contract*; core-hardening owns the reader refactor.** → tech.md §
   Surface C.

---

## Unit IDs

Units are `agent-usability/n`. Cite in commits (`feat(mcp): agent-usability/1 …`).

---

### agent-usability/1 — MCP instructions block

**Goal:** `build_instructions(config) -> str` and `FastMCP("shards", instructions=…)`. Sections
per tech.md § Composition: identity, roster, vault, recall mode, tag trap, coordination protocol,
how to read results. ≤ 2 KB. Degrades to a named "run `shards init`" statement when config is
absent or partial; a missing config never stops the server starting.

**Requirements:** R1

**Dependencies:** —

**Files:**

```
src/shards/mcp/instructions.py   # new — pure builder, no I/O
src/shards/mcp/server.py         # :61 — pass instructions=, guarded config load at import
```

**Test scenarios:**

- A config with agent `flights-agent` and roster `["flights-agent","notes-agent"]` renders a
  block naming the identity, both owners, and the vault path.
- No config / no agent / empty roster → block still renders, carries the degraded statements,
  and the server object still constructs.
- Ownership language is cooperative — the block contains no permission/authorization phrasing
  (asserted against a denylist of terms).
- Rendered block is ≤ 2 KB.

**Verification:** `tests/memory/test_instructions.py`; `uv run pytest -q` green; `uv run ty check
src/` clean; `shards-mcp` starts with and without `SHARDS_CONFIG_PATH` pointing at a real file.

---

### agent-usability/2 — Tool schema self-description

**Goal:** `Annotated[T, Field(description=…)]` on every parameter of all registered `shards_*`
tools; `note_type: str` → `NoteType`, task `status` → `TaskStatus`. Descriptions state semantics
(id-vs-slug, `since` syntax, `depth` cost), not restatements of the parameter name.

**Requirements:** R2

**Dependencies:** —

**Files:**

```
src/shards/mcp/server.py   # every tool signature; :229 note_type, task status filter
```

**Test scenarios:**

- Every parameter of every registered tool has a non-empty description (introspect the app's
  tool table; no tool exempt).
- `note_type` and `status` schemas present the full literal as an enum, and the enum equals
  `typing.get_args(NoteType)` / `get_args(TaskStatus)` — a schema change fails the test if the
  tool is not updated.
- Tool count and annotation classes unchanged (still 17 at this unit; read-only/idempotent/
  write/destructive assignments untouched).

**Verification:** extend `tests/memory/test_tools.py` plus a new
`tests/memory/test_tool_schemas.py`; `grep -c "Field(" src/shards/mcp/server.py` > 0;
`uv run pytest -q` green, `uv run ty check src/` clean.

---

### agent-usability/3 — Tag mutation contract

**Goal:** A bare comma-list on update is additive; replacement requires an explicit opt-in.
Identical one-sentence semantics in the MCP parameter description, the instructions block, and
the CLI `--tags` help.

**Requirements:** R3

**Dependencies:** —
**Amends:** agent-usability/1's tag-trap section wording (text only, no gate).

**Files:**

```
src/shards/core/notes.py   # update_note — tag-delta parsing
src/shards/core/tasks.py   # update_task — same
src/shards/mcp/server.py   # note_update / task_update :283-291 — param + description
src/shards/cli/note.py     # --tags help
src/shards/cli/task.py     # --tags help
```

**Test scenarios:**

- A note tagged `["infra","urgent","q3"]` updated with `tags="urgent"` retains all three (add is
  idempotent) — the silent-wipe regression, locked.
- `+x,-y` still adds and removes; removing an absent tag is a no-op.
- The explicit replace path replaces, and only that path replaces.
- The same semantics sentence appears in the MCP schema, the instructions block, and CLI help
  (asserted, so the three cannot drift).
- Unknown/foreign frontmatter keys still round-trip through the update path (root `tech.md`
  Invariant 3).

**Verification:** `tests/notes/` + `tests/tasks/` tag-update cases; `tests/memory/test_tools.py`;
`uv run pytest -q` green. Behaviour change noted in the commit body.

---

### agent-usability/4 — `shards_session_start`, `shards_health`, search-mode marker

**Goal:** Two read-only tools routing through existing `core` functions (`core/lenses.py`
session-start lens, `core/search.py::search_health`), each returning the resolved identity; and a
mode marker on `shards_search` results so a fallback hit is distinguishable without a second
call. Surface goes 17 → 19 tools; the withheld list is unchanged.

**Requirements:** R4

**Dependencies:** agent-usability/2 (both tools follow the annotation convention)
**Amends:** agent-usability/1's tool inventory line (text only, no gate).

**Files:**

```
src/shards/mcp/server.py   # two tool functions + _register() entries (:356-378);
                           # shards_search :179-189 gains the mode marker
```

**Test scenarios:**

- `shards_session_start` returns the same rows as the CLI `session-start` lens for one vault
  fixture, plus the resolved agent identity.
- `shards_health` reports `mode: "indexed"` with every gate open, and `mode: "fallback"` with the
  matching `reason` for each closed gate (hybrid off, no collection, daemon down, binary absent) —
  mirroring `core/search.py::search_health`.
- A search run on the fallback path is distinguishable from a hybrid run by payload alone.
- 19 tools register; the withheld set (both deletes, `daemon`, `reindex`, `status`,
  `task_release`) is still absent.

**Verification:** `tests/memory/test_tools.py` (count + withheld assertions),
`tests/memory/test_session_hook.py` parity case, new health cases in `tests/search/`;
`uv run pytest -q` green.

---

### agent-usability/5 — Structured MCP errors, first-run config failure

**Goal:** Map every domain exception to a FastMCP tool error carrying `{kind, message,
next_action}` plus the exception's own fields (`task_id`, `existing_owner` from
`core/tasks.py:58-67`). Replace `load_config`'s bare `SystemExit(2)`
(`schemas/config.py:88-92`) with a typed, message-carrying error — exit 2 at the CLI boundary, a
tool error at the MCP boundary, pointing at `shards init`. No `BaseException` escapes a handler.

**Requirements:** R5

**Dependencies:** agent-usability/7 (the message names `shards init`)
**Coordination:** shares `schemas/config.py` with **core-hardening** — see § Feature gate.

**Files:**

```
src/shards/schemas/config.py   # ConfigMissingError replaces SystemExit(2)
src/shards/mcp/server.py       # boundary mapping for every tool
src/shards/cli/__main__.py     # config error → exit 2, with a message
```

**Test scenarios:**

- Claiming a task held by another agent over MCP yields a structured payload naming the kind,
  the task id, the existing owner, and a next action — not a bare string.
- Every tool, called with no config present, raises a normal tool error mentioning `shards init`;
  no `SystemExit`/`BaseException` reaches the handler boundary (asserted per tool).
- The CLI still exits 2 on a missing config, and now prints a message.
- Existing exit-code contract (2 / 3 / 4) unchanged (root `tech.md` § Contracts).

**Verification:** new `tests/memory/test_errors.py`; existing exit-code tests in `tests/tasks/`
stay green; `uv run pytest -q` green.

---

### agent-usability/6 — CLI flag contract and help truthfulness

**Goal:** `--json` / `--quiet` accepted on either side of every non-admin command with identical
effect (today `shards task list --json` exits 2 while `shards recent-activity --json` works);
`--owner` given one meaning per tech.md § Flag contract (open question 2 settled at this unit);
help strings that enumerate schema values derived from the schema literal, so
`note new --type` can no longer omit `project` (`cli/note.py:39` vs `:83`).

**Requirements:** R6, R7

**Dependencies:** —
**Coordination:** **core-hardening** owns collapsing the three readers (`cli/_output.py:29-39`,
`cli/admin.py:190-195`, `cli/session.py:72-82`). This unit specifies the observable contract and
locks it with tests; it does not spec their refactor.

**Files:**

```
src/shards/cli/note.py       # leaf flags; --type help from get_args(NoteType)
src/shards/cli/task.py       # leaf flags (:264-278)
src/shards/cli/session.py    # already coalesces (:72-81) — align, don't fork
src/shards/cli/__main__.py   # --owner resolution (:137,:144)
```

**Test scenarios:**

- A parametrised walk over the command table: every non-admin command accepts `--json` and
  `--quiet` before **and** after the command name, with byte-identical output either way.
- `shards --owner agent-b note new "x"` produces the contracted owner (honoured, or the flag
  rejected there) — never silently `agent-a`.
- `shards note new --help` lists every value `get_args(NoteType)` yields; creating a `project`
  note by following help succeeds.
- Existing exit codes and output shapes unchanged for every already-working invocation.

**Verification:** new `tests/cli/test_flag_contract.py` (new folder — cross-verb CLI contract
tests have no home today); `uv run pytest -q` green; `uv run ruff check .` clean.

---

### agent-usability/7 — `shards init`, example config, README

**Goal:** `shards init` writes a valid `~/.shards/config.toml` (vault path, agent identity,
roster, search settings), honours `$SHARDS_CONFIG_PATH`, prints the path written, refuses to
overwrite without `--force`, and is safe to re-run. Committed `config.example.toml` documenting
every key in `schemas/config.py`. README grows install / config / daemon / MCP + plugin wiring.

**Requirements:** R9

**Dependencies:** —

**Files:**

```
src/shards/cli/admin.py      # init command, beside daemon/status/reindex
src/shards/cli/__main__.py   # register init (admin, withheld from MCP)
config.example.toml          # new, committed
README.md                    # install, config, daemon, MCP + plugin wiring
```

**Test scenarios:**

- `init` into an empty `$SHARDS_CONFIG_PATH` writes a file that `load_config` accepts without
  error, with every `Config` field populated or defaulted.
- Re-running without `--force` leaves the existing file byte-identical and exits non-zero with a
  message; `--force` rewrites.
- `config.example.toml` parses and loads through `load_config` — the example cannot rot.
- `init` is absent from the MCP tool table.
- CLI startup-time guard still passes (`tests/test_startup_guard.py`) — no new import on the hot
  path.

**Verification:** new `tests/cli/test_init.py`; `uv run pytest -q` green; `shards --help` shows
`init` and no new verb beyond it.

---

### agent-usability/8 — Plugin bundle and the `shards` skill

**Goal:** Ship `plugins/shards/` — `.claude-plugin/plugin.json`, `skills/shards/SKILL.md`,
`.mcp.json` (`command: shards-mcp`), `hooks/hooks.json` (per open question 3) — plus a repo-root
`.claude-plugin/marketplace.json`. One skill only. SKILL.md frontmatter stays in the six-field
subset so the same file uploads to claude.ai for Cowork. Content: the vault-coherence playbook
and the coordination protocol; ownership rules phrased as cooperation, never authorization.

**Requirements:** R8

**Dependencies:** agent-usability/1, /3, /4, /6, /7 — the skill documents the finished surface;
writing it earlier guarantees a rewrite.

**Files:**

```
plugins/shards/.claude-plugin/plugin.json
plugins/shards/skills/shards/SKILL.md
plugins/shards/.mcp.json
plugins/shards/hooks/hooks.json
.claude-plugin/marketplace.json
README.md                              # plugin install section
```

**Test scenarios:**

- `plugin.json`, `.mcp.json`, `hooks.json`, `marketplace.json` are valid JSON and reference only
  console scripts that exist (`shards-mcp`, `shards`).
- SKILL.md frontmatter keys are a subset of {`name`, `description`, `license`, `compatibility`,
  `metadata`, `allowed-tools`}; `name` is `shards`.
- The skill body states all seven protocol rules and contains no authorization phrasing (same
  denylist as unit 1).
- The skill does not duplicate the instructions block's live-config sections (identity, roster,
  vault path) — those are runtime-only by construction.
- Exactly one skill directory exists under `plugins/`; the developer-facing `.agents/skills/spec`
  and `.claude/skills/spec` are untouched.

**Verification:** new `tests/test_plugin_bundle.py`; `uv run pytest -q` green; manual install of
the plugin from the repo-as-marketplace in a scratch Claude Code session, with `shards_*` tools
and the skill both present.

---

## Dependencies

| Unit | Blocks | Blocked by |
|---|---|---|
| agent-usability/1 | 8 | — |
| agent-usability/2 | 4 | — |
| agent-usability/3 | 8 | — |
| agent-usability/4 | 8 | agent-usability/2 |
| agent-usability/5 | — | agent-usability/7 |
| agent-usability/6 | 8 | — |
| agent-usability/7 | 5, 8 | — |
| agent-usability/8 | — | agent-usability/1, /3, /4, /6, /7 |

Soft amendments (text only, no gate): /3 and /4 update wording inside /1's block.

---

## Progress

Complete — all 8 units shipped (status corrected by the core-hardening spec-reconciliation unit,
from `git log b008ce8..HEAD`).

| Unit | Status | Evidence |
|---|---|---|
| agent-usability/1 | DONE | `a06fb26` feat(mcp): add config-driven instructions block; `460202e` fix(mcp): correct owner-vs-identity claim in instructions block |
| agent-usability/2 | DONE | `7f3403b` feat(mcp): describe every tool parameter in its schema; `e795a97` fix(mcp): type task priority and graph direction as Literal |
| agent-usability/3 | DONE | `befed8d` feat(core): make bare tag update additive, not a wipe; `dda3ded` fix(core): reject mixed-prefix tag specs instead of guessing |
| agent-usability/4 | DONE | `f1d0f25` feat(mcp): add shards_health + search-mode marker; `b540bdf` fix(search): make search-mode marker observed, not predicted (round-1 finding); `cf12aaa` test(mcp): update stale recall-gap comment |
| agent-usability/5 | DONE | `42246b2` feat(mcp): structured errors, typed missing-config |
| agent-usability/6 | DONE | `efbdeb4` fix(cli): uniform --json/--quiet/--owner flag contract; `b7117bc` docs(spec): settle --owner scope decision |
| agent-usability/7 | DONE | `ae17229` feat(cli): add shards init, example config, README onboarding |
| agent-usability/8 | DONE | `ca397c7` feat(plugin): add shards plugin bundle and vault-coherence skill; `c93e4c1` fix(skill): correct false tag-delta claim (round-1 finding) |

---

## Open Questions

Carried from [product.md](product.md) § Open Questions and [tech.md](tech.md) § Open Questions;
each is settled inside the unit that first depends on it, not before implementation starts.

| # | Question | Settled in |
|---|---|---|
| 1 | Tag delta vs. replace default | agent-usability/3 |
| 2 | `--owner` honoured globally or narrowed | agent-usability/6 |
| 3 | Does the plugin bundle a `SessionStart` hook | agent-usability/8 |
| 4 | Search mode marker shape (per-hit vs. envelope) | agent-usability/4 |
