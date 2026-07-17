---
type: feature-product
feature: shards-rebrand
sibling: tech.md
parent: ../../product.md
updated: 2026-07-05
---

# Feature: Shards Rebrand — Product

Rename the project from **brain** to **shards** end-to-end (package, CLI command, MCP
surface, env vars, on-disk paths, docs, GitHub repo, local directory) and reframe the
product story from a personal knowledge-coordination tool into **a mesh for multi-agent
collaboration over low-level tools (CLI + MCP)**. The three verbs and every mechanic stay
exactly as built — this is a nominal + narrative change, not a behaviour change.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | Every `brain`/`Brain`/`BRAIN` → `shards`/`Shards`/`SHARDS` token in code, tests, docs, config keys, env vars, paths, socket/PID names, MCP tool names, CLI command names; the narrative reframe of root `.spec/{product,tech,design,plan,lessons}.md`, `README.md`, `AGENTS.md`; regenerated `uv.lock`; GitHub repo rename + local directory move. |
| **Does not own** | Verb semantics, mechanics, exit codes, contracts, the deferred tasks-graph — all unchanged. The external `GBrain`/`gbrain` reference (`github.com/garrytan/gbrain`). The vendored `.agents/skills/spec/**` (its only "brain" hits are the unrelated word *brainstorming*). The `indexed` contract. |
| **Deferred** | None. |

---

## Requirements

### Requirement: Single brand token

The system SHALL present as **shards** across every user- and agent-facing surface —
package/import name, CLI command (`shards`, `shards-mcp`), MCP server name and tool prefix
(`shards_*`), environment variables (`$SHARDS_AGENT`, `$SHARDS_CONFIG_PATH`), and on-disk
paths (`~/.shards/`, `shards.sock`, `shards.pid`). No user- or agent-facing `brain` token
MUST remain.

#### Scenario: No residual brand token

- **Given** the rebrand is complete
- **When** `git grep -in brain` is run over the tree
- **Then** the only matches are the external `GBrain`/`gbrain` link in `product.md` and the word *brainstorming* under `.agents/skills/spec/`

#### Scenario: Command and tools respond under the new name

- **Given** the package is installed after the rename
- **When** `shards --help` and the `shards-mcp` entry point are invoked
- **Then** both resolve and expose the same verbs/tools as before under the new names

### Requirement: Narrative reframe

The product story MUST lead with "a mesh for multi-agent collaboration over CLI + MCP,"
while keeping the three verbs (`note`, `task`, `search`) and the Tolaria coupling unchanged.

#### Scenario: Positioning updated, mechanics preserved

- **Given** root `.spec/product.md`, `README.md`, and `AGENTS.md`
- **When** a reader opens them after the rebrand
- **Then** the opening framing is the multi-agent collaboration mesh, and the documented verbs, phases, and non-goals are unchanged in substance

### Requirement: Behaviour parity

The rename SHALL be purely nominal. Verbs, mechanics, exit codes, schemas, and socket
contracts MUST NOT change in behaviour.

#### Scenario: Test suite stays green

- **Given** the full rename is applied
- **When** `uv run pytest -q`, `uv run ruff check .`, and `uv run mypy src/` run
- **Then** all pass with no behavioural test edits beyond renamed identifiers/paths/env vars

### Requirement: Repo identity

The GitHub repository and local working directory SHALL become `shards`.

#### Scenario: Repo and directory renamed

- **Given** the in-repo rename is committed and green
- **When** the GitHub repo is renamed and the local directory moved
- **Then** the remote resolves at `LennardZuendorf/shards` and the working tree lives at `Development/shards`

---

## Non-Goals

- No change to verb count, verb semantics, or the deferred Phase-3 tasks-graph.
- No renaming of the external `GBrain` project reference.
- No touching the vendored spec skill under `.agents/skills/`.
- No migration shim for old `~/.brain/` state — pre-release, the old name never shipped (see [tech.md](tech.md) § Accepted side effects).
