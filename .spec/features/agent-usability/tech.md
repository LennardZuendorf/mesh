---
type: feature-tech
feature: agent-usability
sibling: product.md
parent: ../../tech.md
updated: 2026-08-16
---

# Feature: Agent Usability — Architecture

Four surfaces, ranked by leverage: **A** the MCP `instructions` block (built from live config —
nothing else can do that), **B** the tool schemas and error payloads, **C** the CLI flag/help
contract, **D** the plugin + skill bundle and first-run path. No change to `core`, `storage`, or
the on-disk frontmatter contract — this feature is entirely about what the two client surfaces
*say*.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Constraints

| Constraint | Source |
|---|---|
| Three verbs; `init` is admin, not a verb | root `product.md` R1 |
| MCP mirrors the *safe* verbs only; delete/daemon/reindex/status/`task_release` stay withheld | root `tech.md` § Implemented surfaces |
| Owner identity is trusted local input, not an authorization boundary | `AGENTS.md` § 6 |
| Instant CLI — nothing added to the CLI import path | root `tech.md` Invariant 6 |
| Agent content is inert data | root `tech.md` Invariant 5 |
| Skills load only from `~/.claude/skills/`, `.claude/skills/`, or a plugin | Claude Code docs |
| Cowork sessions load account-enabled skills, not local skill dirs | Claude Code docs |

---

## Surface A — The MCP instructions block

`src/shards/mcp/server.py:61` constructs `FastMCP("shards")` with no `instructions`. FastMCP
3.4.2 supports a server-level `instructions` kwarg; the string is delivered to the client at
initialize, ahead of any tool call.

### Composition

A pure function — `build_instructions(config: Config) -> str` — takes the loaded `Config` and
returns the block. Pure means testable: assert on the rendered text for a given config, with no
process or socket.

Sections, in order:

| Section | Source | Degrades to |
|---|---|---|
| What shards is | static — three verbs, one folder, Markdown is truth | — |
| **Your identity** | `config.agent` (already `$SHARDS_AGENT`-resolved at load, `schemas/config.py`) | "no agent identity configured — run `shards init`; `task_claim` will fail without one" |
| **Valid owners** | `config.tasks.collections` | "no roster configured; any owner string is accepted" |
| **Vault** | `config.core.tolaria_path` | — (required key; absent config never reaches here) |
| Recall | `config.search.collection` / `.hybrid` — whether hybrid is even configured | "substring fallback only" |
| Tag mutation trap | static, mirrors § Tag mutation below | — |
| Coordination protocol | static — the same seven rules as the skill | — |
| Reading results | static — how to interpret `owner` / `claimed_by` / `status` / `path` | — |

**Where it is built.** At module import, alongside `_register()`. A missing config must not stop
the server from starting (product R1, scenario 2), so the builder catches the config-load failure
and renders the fully-degraded block; the *tools* still fail per § Error contract when called.

**Budget.** Target ≤ 2 KB. It is prepended to every session's context; it is a briefing, not a
manual. Anything longer belongs in the skill.

**Phrasing constraint.** The ownership rules are cooperation conventions. The block says
"another agent holds this — pick a different task", never "you are not permitted". Same wording
as the skill (`AGENTS.md` § 6).

### Why this and not a skill

The block is the only artifact that can inject *this* operator's resolved identity, roster, and
vault path. A skill is a static file, and in Cowork it is the only artifact that loads at all —
which is why both exist and the block is primary.

---

## Surface B — Tool schemas, new tools, error payloads

### Parameter descriptions

`grep -c "Field(" src/shards/mcp/server.py` → `0`. Every parameter is bare. The fix is
`Annotated[T, Field(description=...)]` on each tool signature — FastMCP derives the JSON Schema
from the annotation, so the description ships in `tools/list` with no separate registry to keep
in sync.

Two rules keep it from drifting:

1. **Enumerations come from the schema module, never a re-typed string literal.** `note_type:
   str` (`server.py:229`) becomes `NoteType` (`schemas/note.py:26`); the task `status` filter
   becomes `TaskStatus` (`schemas/task.py`). The literal emits an enum in the JSON Schema for
   free, and a schema change propagates to the tool surface automatically.
2. **Descriptions state semantics, not restatements of the name.** `target` says "note id
   (`n-…`) or title slug"; `since` says "`7d`, `12h`, or ISO"; `depth` says what a larger value
   costs.

### Tag mutation

The trap: `tags` is `list[str]` on `note_new` / `task_new` (`server.py:227-237`) but `str` on
`note_update` / `task_update` (`server.py:283-291`), where `"+x,-y"` is a delta and `"x,y"` is a
**full replacement**. `tags="urgent"` wipes the list. Today that is documented only in CLI help
and a `core` docstring — nowhere an MCP agent can see it.

**Decision — additive default, explicit replace.** A bare comma-list is treated as a delta-add;
replacement requires an explicit opt-in (a `replace_tags: bool` parameter, or a `=`-prefixed
value — settled in unit 3). Rationale: the destructive reading of an ambiguous input is the wrong
default, and this is the one place in shards where a plausible call silently destroys data. This
**is** a behaviour change to the CLI's `--tags` on update, which is why it is its own unit with
its own tests, and why the alternative (keep replace, reject un-prefixed input with exit 2) is
recorded below as the fallback if the break proves unacceptable.

Whichever wins: the semantics appear in the parameter description, in the instructions block, and
in the CLI help — one sentence, three places, identical wording.

### New tools

| Tool | Annotation | Returns | Why |
|---|---|---|---|
| `shards_session_start` | read-only | the CLI `session-start` lens payload + resolved identity | The lens is shipped and **not** on the withheld list (root `tech.md` § Implemented surfaces); MCP-only agents currently cannot warm-start |
| `shards_health` | read-only | `core/search.py::search_health(config)` payload + vault path + agent identity | `search_health` exists precisely to stop silent degradation and is CLI-only today |

Both route through existing `core` functions — no parallel implementation, per the module's own
docstring contract. Registration follows the existing `_register()` pattern (`server.py:356-378`),
taking the surface from 17 to 19 tools. Neither is a new verb: both are read-only lenses over
shipped behaviour.

**Search degradation marker.** `shards_search` calls `query_search(..., quiet=True)`
(`server.py:179-189`), so the stderr notice the CLI prints is swallowed and no field distinguishes
a hybrid hit from a fallback hit. The result envelope gains a mode marker sourced from the same
gates `search_health` checks. Payload shape (a sibling `mode` field vs. wrapping the list) is
settled in unit 4; the requirement is only that a fallback result is distinguishable without a
second call.

### Error contract

Two failures escape today:

- `ClaimConflictError` (`core/tasks.py:58-67`) carries `existing_owner`, but over MCP it
  surfaces as a raw exception string — the structured field is lost and there is no next action.
- `load_config` raises `SystemExit(2)` with no message (`schemas/config.py:88-92`). `SystemExit`
  is a `BaseException`; escaping a FastMCP tool handler it is neither a normal tool error nor a
  caught one, and on first run it is a silent exit-2.

**Contract.** Every MCP tool handler maps its domain exception to a FastMCP tool error carrying a
structured payload: `{kind, message, next_action}` plus the domain fields the exception already
holds (`task_id`, `existing_owner`, …). The mapping lives at the MCP boundary — the same
one-place-mapping discipline the CLI already uses for exit codes (root `tech.md` § Shared
primitives). No `BaseException` reaches the handler boundary.

**Dependency — `load_config`.** Replacing `SystemExit(2)` with a typed, message-carrying
`ConfigMissingError` (mapped to exit 2 at the CLI boundary, to a tool error at the MCP boundary)
is a shared edit with **core-hardening**, which owns correctness fixes. This track owns the
*required behaviour* — an actionable message naming `shards init`; whichever track lands the
exception type first, the other adapts. Flagged in [plan.md](plan.md) § Dependencies.

---

## Surface C — The CLI contract

This track owns the **user-facing contract** — which flags work where and what they mean.
**core-hardening** owns the internal refactor that collapses the three parallel readers
(`cli/_output.py:29-39`, `cli/admin.py:190-195`, `cli/session.py:72-82`) into one. State the
contract; do not spec their refactor.

### Flag contract

| Flag | Contract |
|---|---|
| `--json` / `--quiet` | Accepted **before or after** any command name, on every non-admin command, with identical effect. Today the lens commands coalesce both sides (`cli/session.py:72-81`) while the note/task leaves read `ctx.obj` only (`cli/task.py:264-278`) — so `shards task list --json` exits 2 ("No such option") while `shards recent-activity --json` works. |
| `--owner` | One meaning: the identity this invocation acts as. **Decision (settled in unit 6):** honoured per role, not a single global-vs-narrowed binary — **write-on-creation** (`note new`/`task new`: defaults the written `owner` when no local `--owner` is given) and **filter** (`note list`/`task list`/`search`) both coalesce the global flag; **identity resolution** (`task claim`/`task release`/`session-start`) already read it and are unchanged. **Not** coalesced into `task update`'s opt-in reassignment `--owner` — an update only changes what is explicitly asked, and folding the ambient global identity into an unrelated `--priority`/`--tags` update would silently reassign accountability nobody asked to change (the same non-destructive-default precedent as the tag-mutation decision above); an explicit *local* `--owner` on `task update` still reassigns. A local `--owner` always wins over the global one wherever both exist. `shards --owner agent-b note new` writing `owner: agent-a` was the bug this closes. (Not an auth boundary — `AGENTS.md` § 6.) |
| `--mine` | Unchanged: owner-or-`claimed_by` equals the resolved identity. |

The contract is a *test* obligation, not just prose: a parametrised test walks the command table
and asserts each command accepts the flags the contract says it accepts.

### Help truthfulness

`cli/note.py:83` lists "note | log | decision | reference" while `cli/note.py:39` validates
`project` too. Help strings that enumerate schema values are **derived from the schema literal**
(`typing.get_args(NoteType)`), not retyped — the class of drift is removed, not the instance.

---

## Surface D — Plugin, skill, onboarding

### Why one skill

| Candidate | Verdict |
|---|---|
| `shards` — vault coherence + coordination protocol | **Ship.** Behavioural rules an agent applies while working; not derivable from tool schemas. |
| setup / onboarding skill | Rejected — the gap is a missing `shards init`, not missing prose. |
| search skill | Rejected — `search` is one tool with described parameters; a skill adds nothing. |
| notes skill + tasks skill (split) | Rejected — the interesting rules are exactly the cross-verb ones (search before write, link when a note continues another). Splitting them loses the point. |

### Layout

```
plugins/shards/
├── .claude-plugin/plugin.json     # name, version, description
├── skills/shards/SKILL.md         # the one skill
├── .mcp.json                      # {"shards": {"command": "shards-mcp"}}
└── hooks/hooks.json               # SessionStart → shards session-start (open question 3)
.claude-plugin/marketplace.json    # repo doubles as its own marketplace
```

Bundling `.mcp.json` is the load-bearing choice: the skill can never be installed without the
tools it describes. `shards-mcp` is an existing console script (`pyproject.toml:19`).

`SKILL.md` frontmatter stays inside the six-field spec subset — `name`, `description`, `license`,
`compatibility`, `metadata`, `allowed-tools` — so the same file uploads to claude.ai for Cowork,
where local skill dirs are not read.

**Existing skill machinery in-repo is developer-facing and untouched:** `.agents/skills/spec/`,
the `.claude/skills/spec` symlink, `skills-lock.json`. `hooks/session_start.json` is an un-wired
snippet; there is no `.claude/settings.json`.

### Distribution matrix

| Consumer | Gets instructions from | Gets the playbook from |
|---|---|---|
| Claude Code, plugin installed | MCP `instructions` | bundled skill |
| Claude Code, MCP configured by hand | MCP `instructions` | — (block must stand alone) |
| Cowork session | MCP `instructions` | account-enabled skill (uploaded `SKILL.md`) |
| Human at a shell | `--help`, README | `config.example.toml`, README |

Row 2 is why the instructions block must be self-sufficient and why the skill may not assume it.

### Onboarding

No `shards init` exists (`note task search daemon status reindex recent-activity build-context
graph project session-start`), no `config.example.toml`, and the README is 11 lines with no
install or config section. `.gitignore` excludes `config.toml`, so nothing ships as reference.

- `shards init` — admin command beside `daemon` / `status` / `reindex`; **withheld from MCP**.
  Prompts (or takes flags) for vault path, agent identity, roster; writes
  `~/.shards/config.toml`; honours `$SHARDS_CONFIG_PATH`; refuses to overwrite without `--force`;
  prints the path it wrote. Idempotent and non-destructive by default.
- `config.example.toml` — committed, every key documented, matches `schemas/config.py`.
- README — install, config, `daemon start`, MCP wiring, plugin install.

---

## Tradeoffs

- **Instructions block vs. context budget.** Every token is paid on every session. Capped at
  ~2 KB; depth goes in the skill, which loads on demand.
- **Additive tags by default breaks existing CLI behaviour.** Accepted: silent data loss is
  worse than a documented, tested behaviour change. Fallback recorded above.
- **Deriving help from schema literals couples CLI help to `schemas/`.** Accepted — that
  coupling is the point; the alternative is the drift already shipped.
- **Two more MCP tools widen the surface.** Both are read-only lenses over shipped `core`
  functions, both were already CLI-visible, neither is a new verb. The withheld list is unchanged.
- **Repo-as-marketplace is low ceremony but not discoverable.** Fine for a trusted local fleet;
  third-party publication is out of scope.

---

## Open Questions

1. **Tag default** — additive (chosen, breaking) vs. explicit-prefix-required (non-breaking,
   rejects a natural call). → product.md § Open Questions 1.
2. **`--owner` scope** — honour globally or narrow to the commands that use it. → product.md § 2.
3. **Hook in the plugin bundle** — a `SessionStart` hook assumes a configured, running install on
   a cold machine. → product.md § 3.
4. **Search mode marker shape** — a per-hit field, a sibling `mode` on the envelope, or
   health-tool-only. Settled in unit 4; affects the `hit_dict` contract shared with the CLI.
