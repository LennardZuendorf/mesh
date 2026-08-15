---
type: feature-product
feature: agent-usability
sibling: tech.md
parent: ../../product.md
updated: 2026-08-15
---

# Feature: Agent Usability — Product

shards is built *for* a fleet of agents and ships nothing that teaches one how to use it. The
MCP server hands an agent 17 tools with one-line docstrings, no parameter descriptions, no
identity, no vault path, no roster, and no protocol. The consuming agent — a Claude Code or
Cowork session using shards as its shared substrate, **not** a shards developer — has to infer
the contract, and infers it wrong: it wipes tag lists, reads back `owner` values it cannot
interpret, and silently gets substring-fallback search believing it got hybrid recall.

This feature makes the runtime surface self-teaching: a config-derived MCP instructions block,
described tool schemas, honest flags, one shipped skill, and a first-run path.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The MCP `instructions` block and its composition from live config; tool/parameter self-description; the tag-mutation semantics contract; agent-legible identity and search-health readback; structured MCP error payloads; the **user-facing** CLI flag contract (which flag works on which command, and what `--owner` means); help-text truthfulness; the shipped `shards` skill + plugin bundle and its distribution story; first-run onboarding (`shards init`, example config, README). |
| **Does not own** | Correctness bugs, dead code, the mechanical de-duplication of the three output-flag readers, spec drift, test/CI gaps → **core-hardening**. Backlinks, `task append`, `task release`, `session-start --team`, priority ordering, the team board → **team-awareness**. The three-verb thesis (no new verbs; `init` is admin, not a verb). Tolaria's vault/Git. |
| **Deferred** | Per-agent instruction personalization beyond identity/roster/vault; an MCP `resources`/`prompts` surface; publishing the plugin to a third-party marketplace. |

---

## Why this, why now

Two consumer surfaces exist and neither is served:

- **MCP** is the surface every agent runtime actually reads. It is also the only surface that can
  be built at startup from *this* operator's config — the resolved `$SHARDS_AGENT`, the
  `[tasks].collections` roster, the vault path. No prose artifact, skill, or README can inject
  live state. `FastMCP("shards")` passes no `instructions`; that one omission is the single
  highest-leverage line in the repo.
- **Skills** are the surface that carries a *playbook* — vault coherence and the coordination
  protocol. They load only from `~/.claude/skills/`, `.claude/skills/`, or a plugin, so a
  `pip`/`uv` install cannot ship one, and Cowork sessions do not read local skill dirs at all.
  The skill is therefore secondary and the instructions block is primary.

The gap is visible in the vault, not just in the docs: an agent that follows `note new --type`
help can never create a `project` note, an agent that passes `tags="urgent"` to `note_update`
silently erases every other tag, and `shards --owner agent-b note new` writes `owner: agent-a`.

---

## Requirements

### Requirement: Server instructions built from live config

The MCP server SHALL pass an `instructions` block to `FastMCP`, composed **at startup from the
loaded config** — not a static string — carrying the resolved agent identity, the valid-owner
roster, the vault path, the tag-mutation trap, and the coordination protocol.

#### Scenario: An agent knows who it is before its first call

- **Given** a config with `[core].agent` (or `$SHARDS_AGENT`) resolving to `flights-agent` and
  `[tasks].collections = ["flights-agent", "tolaria-agent"]`
- **When** an MCP client connects and reads the server instructions
- **Then** the block names `flights-agent` as this session's identity, lists both valid owners,
  and gives the vault path — so the agent can interpret every `owner` / `claimed_by` value it
  later reads back without a tool call

#### Scenario: Config is absent or partial

- **Given** no config file, or a config with no agent and an empty roster
- **When** the server starts
- **Then** it still starts and still serves an instructions block — the identity and roster
  sections degrade to a named "not configured, run `shards init`" statement rather than being
  omitted or crashing the server

#### Scenario: Cooperation, not authorization

- **Given** the instructions describe claiming and ownership
- **When** they state "do not claim a task another agent holds"
- **Then** it is phrased as a **cooperation convention among a trusted local fleet**, never as a
  security guarantee — matching `AGENTS.md` § 6 (owner identity is trusted local input, not an
  authorization boundary)

### Requirement: Self-describing tool schemas

Every `shards_*` tool SHALL expose a described, correctly-typed parameter schema — not a bare
name and an untyped scalar.

#### Scenario: Enumerated values arrive as enums

- **Given** `shards_note_new(note_type=...)` and the task `status` filter
- **When** an agent inspects the tool schema
- **Then** `note_type` presents the `NoteType` literal (`note | log | decision | reference |
  project`) and `status` the `TaskStatus` literal — an enum in the schema, no free-text guessing,
  no drift from `schemas/note.py` / `schemas/task.py`

#### Scenario: Every parameter carries a description

- **Given** any registered `shards_*` tool
- **When** its schema is inspected
- **Then** each parameter has a description stating what it accepts, its default, and any
  non-obvious semantics (id-vs-slug targets, recency window syntax, depth cost)

### Requirement: Tag mutation is unambiguous and non-destructive by default

The system SHALL make tag mutation semantics explicit at the call site, so no agent can silently
wipe a tag list.

#### Scenario: A bare tag string does not erase the others

- **Given** a note tagged `["infra", "urgent", "q3"]`
- **When** an agent calls the update tool with `tags="urgent"`
- **Then** the outcome is unambiguous from the schema before the call — the parameter's contract
  states delta-vs-replace in its description, and a replace is only reachable through an explicit
  opt-in, never through a value that reads as "add this tag"

#### Scenario: Create and update speak the same shape

- **Given** create takes `tags: list[str]` and update takes a delta string
- **When** an agent moves between them
- **Then** the asymmetry is documented in both schemas and in the instructions block; the
  on-disk contract does not change

### Requirement: Identity and recall health are readable over MCP

An agent SHALL be able to learn its resolved identity and whether its search results came from
`indexed` or the substring fallback, using MCP tools only.

#### Scenario: Warm start over MCP

- **Given** an agent starting a session with MCP but no shell
- **When** it calls the session-start tool
- **Then** it gets the same lens the CLI hook serves — my open/claimed tasks merged with recent
  activity — plus the resolved identity, so it can begin work without a CLI round-trip. This
  lens is a shipped read-only surface and is not on the withheld list ([../../tech.md](../../tech.md)
  § Implemented surfaces)

#### Scenario: Degradation is visible, not silent

- **Given** the daemon is down or `indexed` is not on `PATH`
- **When** an agent searches over MCP
- **Then** it can tell — a read-only health tool reports the same gates `search --health`
  reports (`mode`, `hybrid_configured`, `collection`, `daemon_up`, `indexed_binary_available`,
  `reason`), and the search result set itself marks which path produced it. The stderr notice the
  CLI prints has no MCP equivalent, so silence must not be the only signal

### Requirement: Errors are structured and actionable

A failing MCP tool call SHALL return a typed, machine-readable error naming the condition and the
next action — never a bare exception string, never a `BaseException` escaping the handler.

#### Scenario: Claim conflict

- **Given** `t-abc` is claimed by `tolaria-agent`
- **When** `flights-agent` calls the claim tool
- **Then** the error payload carries the conflict kind, the task id, the existing owner (already
  on `ClaimConflictError`), and the recommended next action — pick another task, do not steal

#### Scenario: First run with no config

- **Given** no config file exists
- **When** any MCP tool is called (or the server starts)
- **Then** the failure is a normal, message-carrying tool error pointing at `shards init` — not a
  silent `SystemExit(2)` with no message

### Requirement: One uniform CLI flag contract

Global output and identity flags SHALL behave the same on every command, or not be offered.

#### Scenario: `--json` works on both sides of the command name

- **Given** `shards task list --json` today exits 2 ("No such option") while
  `shards recent-activity --json` succeeds
- **When** an agent passes an output flag before or after any command name
- **Then** it takes effect uniformly; no command rejects a flag another command accepts

#### Scenario: `--owner` means one thing

- **Given** `shards --owner agent-b note new "x"`
- **When** the note is written
- **Then** either `owner: agent-b` is honoured everywhere the flag is accepted, or the flag is
  not accepted there at all — the current "parsed, then ignored by everything except `task claim`
  and `recent-activity`" behaviour is not a valid outcome

### Requirement: Help text matches validation

Every command's help SHALL enumerate exactly what validation accepts.

#### Scenario: The `project` note type is reachable

- **Given** `note new --type` help lists "note | log | decision | reference" while validation
  accepts `project` too
- **When** an agent reads help and picks a type
- **Then** every accepted value is listed, and the listing is derived from the schema literal so
  it cannot drift again

### Requirement: One shipped skill, installable with its tools

The repo SHALL ship exactly one skill, named `shards`, carrying the vault-coherence playbook and
the coordination protocol, bundled as a plugin so it cannot be installed without the tools it
describes.

#### Scenario: The playbook is behavioural, not referential

- **Given** an agent with the skill loaded
- **When** it captures knowledge or coordinates work
- **Then** it follows: search before write · append rather than fork a near-duplicate · tag from
  the existing vocabulary · link when a note continues another · claim before work · always
  `finish --outcome` · cancel is for tasks that shouldn't exist, not tasks you failed

#### Scenario: The skill never ships without the tools

- **Given** the plugin bundle
- **When** it is installed from the repo-as-marketplace
- **Then** the MCP server declaration installs with it, so a session can never hold instructions
  for tools it does not have

#### Scenario: The same file works in Cowork

- **Given** Cowork sessions load account-enabled skills, not local skill directories
- **When** the same `SKILL.md` is uploaded to claude.ai
- **Then** it is accepted — its frontmatter stays inside the six-field subset (`name`,
  `description`, `license`, `compatibility`, `metadata`, `allowed-tools`)

### Requirement: A first run that ends in a working install

A new operator or agent SHALL reach a working shards install without reading source.

#### Scenario: `shards init`

- **Given** a machine with shards installed and no `~/.shards/config.toml`
- **When** `shards init` runs
- **Then** it writes a valid config (vault path, agent identity, roster, search settings),
  reports the path it wrote, and is safe to re-run — it never clobbers an existing config without
  an explicit flag

#### Scenario: A reference config exists in the repo

- **Given** `.gitignore` excludes `config.toml`, so nothing ships as reference
- **When** someone looks for the config shape
- **Then** a committed `config.example.toml` documents every key, and the README covers install,
  config, daemon start, and the MCP/plugin wiring

---

## Non-Goals

- No fourth verb. `init` is admin (alongside `daemon` / `status` / `reindex`), human-only, and
  stays off the MCP surface.
- No onboarding *skill* — the missing piece is `shards init`, not more prose.
- No separate search skill and no notes/tasks skill split — one `shards` skill or none.
- No new write primitives, no changes to the on-disk frontmatter contract.
- No fixes to the correctness/dead-code/duplication punch list (**core-hardening**) or the
  team-visibility surface (**team-awareness**).

---

## Open Questions

1. **Tag delta vs. replace — which becomes the default?** Making a bare `"urgent"` mean *add* is
   the safe default but changes existing CLI behaviour; keeping replace and forcing an explicit
   `+`/`-` prefix is non-breaking but rejects a natural call. Decision recorded in
   [tech.md](tech.md) § Tag mutation; the product requirement only fixes "no silent wipe".
2. **Is `--owner` honoured or narrowed?** Honouring it globally makes identity spoofable per
   command (already true locally, per `AGENTS.md` § 6); narrowing it to the two commands that use
   it is smaller but loses a legitimate multi-agent-on-one-shell workflow.
3. **Does the plugin bundle a `SessionStart` hook?** `hooks/session_start.json` exists un-wired;
   wiring it into the plugin gives every session a warm start but assumes the daemon and config
   are already up on a cold machine.
