---
name: shards
description: Vault-coherence and coordination playbook for the shards mesh — search before you write, keep tags and links consistent, and claim/finish/cancel tasks correctly. Use whenever creating, updating, or coordinating notes and tasks in a shared shards vault.
license: MIT
compatibility: Requires the shards MCP server (shards-mcp) or the shards CLI, connected to a configured Markdown vault folder; this plugin bundles the MCP server via its own .mcp.json.
metadata:
  plugin: shards
  unit: agent-usability/8
allowed-tools:
  - mcp__shards__shards_note_get
  - mcp__shards__shards_note_list
  - mcp__shards__shards_note_new
  - mcp__shards__shards_note_append
  - mcp__shards__shards_note_update
  - mcp__shards__shards_task_get
  - mcp__shards__shards_task_list
  - mcp__shards__shards_task_new
  - mcp__shards__shards_task_append
  - mcp__shards__shards_task_claim
  - mcp__shards__shards_task_release
  - mcp__shards__shards_task_finish
  - mcp__shards__shards_task_update
  - mcp__shards__shards_search
  - mcp__shards__shards_health
  - mcp__shards__shards_recent_activity
  - mcp__shards__shards_build_context
  - mcp__shards__shards_graph
  - mcp__shards__shards_project
  - mcp__shards__shards_session_start
---

# shards: vault coherence and coordination

shards gives a fleet of agents one shared Markdown vault through three verbs — `note`,
`task`, `search` — over MCP (`shards_*` tools) or the `shards` CLI. Each tool's own parameter
descriptions cover what a single call does; this skill covers what a parameter description
cannot: the sequence and judgment that keeps the vault coherent when several agents are writing
to it at once.

If your client is MCP-connected, the `instructions` block it received on connect already named
your identity, the valid-owner roster, the vault path, and a five-rule coordination protocol —
built fresh from the live config, so it is always more current than anything written here. This
skill does not repeat that identity/roster/vault-path text (it cannot: only the running server
knows it). It restates and deepens the same protocol with the vault-coherence habits that make
note-taking and task hand-off actually work across agents, whether or not you ever read that
block.

## The seven rules

**1. Search before you write.** Before `note_new`/`task_new`, run `shards_search` (or a tag
pull) for the topic. A title collision already comes back as a non-blocking `warnings` entry on
the creation response — but a near-duplicate under a *different* title never trips that check,
so a search is the only thing that catches it.

**2. Append rather than fork a near-duplicate.** If search turns up an existing note or task
that already covers the ground, extend it with `note_append`/`task_append` instead of writing a
second, competing version of the same knowledge. Three half-written notes on one topic are worse
than one you keep adding to.

**3. Tag from the existing vocabulary.** Pull the tags already in use (a `shards_search` tag
pull, or `note_list`/`task_list`) before inventing a new spelling. `wire-format` and
`wireformat` split one cluster into two invisible-to-each-other piles — tag pulls and filters
only work when spelling is shared.

**4. Link when a note continues another.** Put a `[[wikilink]]` to the prior note/task in the
body; it populates `related`, which is exactly what `shards_graph`/`shards_build_context` (and
`direction: "in"` for backlinks) walk. An unlinked continuation is invisible to everyone but you.

**5. Claim before you work.** Check `claimed_by` before `task_claim`; if another agent already
holds the task, work a different one instead of claiming over them. This is the instructions
block's own first coordination rule, restated here because it is the one most worth never
skipping.

**6. Always finish with an outcome — `task finish --outcome` (CLI) / `task_finish`'s `outcome`
parameter (MCP).** Record what happened before a task moves to done, not just a status flip. A
later `task_get`/`shards_graph` reader should be able to tell what the work produced without
re-deriving it from the body.

**7. Cancel is for tasks that shouldn't exist, not tasks you failed.** `task_cancel` removes a
task from the open queue for good, with a reason. Attempting a task and not finishing it is not
grounds to cancel it — append a note on the blocker (rule 2) and leave the task open, or release
your claim, so someone else (or you, later) can pick it up. Reach for `cancel` only when the
task itself was the mistake: a duplicate, no longer needed, overtaken by events.

## Ownership is a cooperation convention, not proof of identity

`owner` and `claimed_by` are plain strings a caller supplies. `owner` is checked for spelling
against `[tasks].collections` when that roster is non-empty — a value check, never a check on
who is actually calling — and `claimed_by` is never checked against the roster either way. Any
agent with local access can pass any `--owner`/`claimer` value; nothing in shards verifies the
caller is who it says it is. That is by design for a trusted local fleet on one operator's
machine (see the project's own `AGENTS.md` §6) and it is not an auth boundary.

Treat "claimed by someone else" as a social signal to route around — the same move rule 5 asks
for: pick a different task rather than take theirs. `task release`'s `--force` flag is CLI-only,
deliberately never exposed on `shards_task_release`, and it can break another agent's claim; it
exists precisely because this is a convention that can be overridden, not a lock the system
holds for you. Reaching for it is choosing to override a peer, never something the system
stopped someone else from doing in the first place.

## Reading and acting on results

A `warnings` entry on a `note_new`/`task_new` response names the id of a prior note/task with a
matching title — go read it (rule 1) before assuming your new one is needed. A `shards_search`
hit's `mode` field (`"indexed"` or `"fallback"`) tells you whether that result came from ranked
recall or the plain substring scan; call `shards_health` for the same answer without running a
query. `shards_session_start` is the fastest way to see your own open/claimed queue plus inbound
mentions of your notes/tasks at the start of a session — when this plugin's `SessionStart` hook
is active it already primed that view for you (`session-start --meta-only --json`); read it
before assuming nothing is waiting for you.

## Command surface this playbook assumes

CLI: `note {new,get,list,append,update,delete}`,
`task {new,get,list,append,update,claim,release,finish,cancel,delete}`, `search`, the session
lenses `recent-activity`, `build-context`, `graph --direction out|in|both`, `project`,
`session-start --team`, and admin `shards init` (never exposed over MCP). `task list` takes
`--stale`/`--available` alongside `--status`/`--owner`/`--mine`/`--tags`. `--tags` on
`note update`/`task update` merges by default (bare `x,y` adds; `+x,-y` is a delta — `-y`
removes exactly the tags you name; `=x,y` is the only form that can drop tags you *didn't*
name, since anything left out of the new list is discarded).

MCP mirrors the safe subset as typed `shards_*` tools — this skill's `allowed-tools` list, plus
`shards_task_cancel`, which is destructive and left out of that pre-approved list on purpose so
it always asks first. Both delete verbs (`note delete`, `task delete`) and every admin command
(`daemon`, `status`, `reindex`, `init`) are CLI-only, withheld from MCP entirely — a hard
`unlink` with no trash is never something this skill pre-approves, and never something an MCP
client can reach.
