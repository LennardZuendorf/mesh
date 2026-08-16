# shards

A mesh for multi-agent collaboration over a single Tolaria Markdown folder. Three verbs — `note`, `task`, `search` — give a fleet of agents and their human operator a shared substrate through low-level tools: a CLI and an MCP server.

- Notes + search = shared memory.
- Tasks = coordination + handoff (`claim` / `release` / `finish` / `cancel`; dependency graph deferred).
- Markdown is the source of truth; shards owns the interface (and writes), not the data.

Search delegates to the first-party [`indexed`](https://github.com/LennardZuendorf/indexed) engine; shards coexists with the Tolaria vault MCP on one folder — no database to run.

The spec is the source of truth: see [`.spec/`](.spec/). Working in here? Read [`AGENTS.md`](AGENTS.md) first.

## Install

Requires Python 3.11+ and [`uv`](https://docs.astral.sh/uv/).

```bash
uv sync --all-groups
uv run shards --help
```

This installs two console scripts: `shards` (the CLI) and `shards-mcp` (the MCP server entry point), both wired in `pyproject.toml`.

## First run

```bash
uv run shards init
```

`init` writes `~/.shards/config.toml` (or `$SHARDS_CONFIG_PATH` when set), creates the vault
directory, and prints the path it wrote. Re-running it is always safe: with an existing config
and no `--force`, it refuses and leaves the file untouched; `--force` rewrites it.

```bash
uv run shards init --help
```

```
--path             TEXT     Vault folder ([core].tolaria_path). Defaults to ~/.shards/vault.
--agent            TEXT     This agent's identity ([core].agent). Defaults to $SHARDS_AGENT, else 'agent'.
--collections      TEXT     Comma-separated roster of valid --owner identities ([tasks].collections).
                             Default: empty — an open roster, any owner string accepted.
--search-collection TEXT    indexed collection name ([search].collection). Default: unset.
--hybrid/--no-hybrid        Hybrid lexical+vector search via indexed ([search].hybrid). Default: on.
--threshold        FLOAT    Substring-fallback score floor ([search].threshold). Default: 0.65.
--force                     Overwrite an existing config. Default: refuse.
```

`init` is admin, not a fourth verb — see [Admin commands](#admin-commands) below. It never
appears on the MCP surface: it writes the very config every other command depends on, so
triggering it remotely would never be agent-safe.

Once a config exists:

```bash
uv run shards note new "hello" --body "first note" --type note
uv run shards task new "do something" --body "details"
uv run shards task list
```

## Config

Config lives at `~/.shards/config.toml`, overridable via `$SHARDS_CONFIG_PATH` (the test/
alternate-vault escape hatch). A missing config exits 2 with a message naming the resolved
path and the required key. A committed reference copy — every key `schemas/config.py` defines,
documented — lives at [`config.example.toml`](config.example.toml); `config.toml` itself is
gitignored, since it names a real vault path and identity.

```toml
[core]
tolaria_path = "~/shards-vault"   # required — the Tolaria vault folder; ~ is expanded at load
agent = "my-agent"                # optional — default owner/claimer; $SHARDS_AGENT overrides

[search]
collection = "my-vault"           # optional — indexed collection name
hybrid = true                     # default true — false or indexed absent -> substring fallback
threshold = 0.65                  # default 0.65 — min score to count a hit as hybrid-quality

[tasks]
collections = ["my-agent", "another-agent"]  # optional roster; empty = any --owner accepted
```

`[core].path` is also accepted as an alias for `tolaria_path` (the root spec spells it that
way); `$SHARDS_AGENT` always wins over `[core].agent` when both are set.

## CLI surface

Three verbs, plus session lenses, plus human-only admin. `shards --help` is always the source
of truth for the live command list; this is a summary.

**`note`** — `new` / `get` / `list` / `append` / `update` / `delete`. Types: `note`, `log`,
`decision`, `reference`, `project`.

**`task`** — `new` / `get` / `list` / `append` / `update` / `claim` / `release` / `finish` /
`cancel` / `delete`. `list` supports `--status`, `--owner`, `--mine`, `--tags`/`--any-tag`,
`--project`, `--since` (recency floor) and `--stale` (recency ceiling — the inverse of
`--since`), and `--available` (open + unclaimed, defaulting `--sort` to `priority`). `append`
adds body text to an existing task without rewriting what's there; `release` drops a claim
back to `open` (idempotent — releasing an already-open task is a no-op).

**`search`** — `shards search "<query>"` for hybrid recall (falls back to a substring scan when
`indexed` is unavailable or `[search].hybrid` is off), or `shards search --tags x,y` for an
exact frontmatter tag pull. `--health` reports which mode is actually live right now.

**Session lenses** (read-only) — `recent-activity`, `build-context`, `graph` (`--direction
out|in|both` — `in` walks backlinks, `both` walks either), `project`, `session-start`
(`--team` widens the activity half to every agent; the task-ownership half always stays
yours).

**Global flags** — `--json`, `--quiet`, `--owner`, `--mine` are accepted either before or
after the command name on every non-admin command, with identical effect. `--owner` means the
identity this invocation acts as: honoured on creation (defaults the written `owner`) and on
list filters, unchanged on `claim`/`release`/`session-start` (which already read it).

### Admin commands

`init`, `daemon start|stop|status`, `status`, `reindex` — human-only, out of the three-verb
surface, and withheld from MCP. `daemon` supervises the warm socket accelerator (every command
still works, just slower, with the daemon down); `status` reports vault health (counts,
freshness, dangling links, stale locks, per-agent claim breakdown); `reindex` rebuilds the
search index and degrades to a notice if `indexed` is missing.

```bash
uv run shards daemon start
uv run shards status
```

### Breaking change: `--tags` on update

As of this branch, `note update` / `task update`'s `--tags` grammar changed to prevent silent
data loss:

- **Bare `x,y`** now **merges** — adds tags, additive and idempotent. This used to *replace*
  the whole list; that behaviour is gone.
- **`=x,y`** replaces the whole list explicitly (a bare `=` clears all tags).
- **`+x,-y`** is a delta: add `x`, remove `y` (every token's own leading `+`/`-` counts, not a
  spec-wide prefix — `+c++` adds a tag literally named `c++`).

If a script relied on the old bare-`x,y`-replaces behaviour, switch it to `=x,y`.

## Daemon

```bash
uv run shards daemon start   # warm watcher + frontmatter index, backgrounded
uv run shards daemon status
uv run shards daemon stop
```

The daemon is an accelerator, never a gatekeeper: every command above degrades to a direct
filesystem scan when it is down, never fails because it's down.

## MCP + plugin wiring

`shards-mcp` runs the FastMCP server exposing the agent-safe `shards_*` tool surface — the same
`note`/`task`/`search` verbs (plus `task_release` and the read-only session lenses, including
`shards_session_start`), typed as real parameters rather than CLI flag strings. Delete and every
admin command (including `init`) are withheld; nothing that writes the config or touches the
daemon is reachable over MCP.

Register it by hand with any MCP-capable client, e.g. in Claude Code:

```json
{
  "mcpServers": {
    "shards": {
      "command": "uv",
      "args": ["run", "shards-mcp"]
    }
  }
}
```

or, once installed outside a `uv` project, simply `"command": "shards-mcp"`.

On connect, the server sends an `instructions` block built from your live config — your
resolved identity, valid-owner roster, vault path, and current search mode — so a client gets
oriented before making any tool call, with no separate skill required. A bundled Claude Code
plugin (packaging this server plus a playbook skill as one install) ships separately; until
then, the snippet above plus `shards init` is the whole first-run path.
