---
name: mesh
description: Vault-coherence and coordination playbook for mesh — search before you write, pick the right space, keep tags and links consistent, and claim/finish/cancel tasks correctly. Use whenever creating, updating, recalling, or coordinating notes, tasks, memories, scratch and assets in a shared mesh vault.
license: MIT
compatibility: Requires the mesh MCP server (mesh-mcp) or the mesh CLI, connected to a configured Markdown vault folder; this plugin bundles the MCP server via its own .mcp.json.
metadata:
  plugin: mesh
  unit: rust-rewrite
allowed-tools:
  - mcp__mesh__mesh_note_get
  - mcp__mesh__mesh_note_list
  - mcp__mesh__mesh_task_get
  - mcp__mesh__mesh_task_list
  - mcp__mesh__mesh_search
  - mcp__mesh__mesh_health
  - mcp__mesh__mesh_recent_activity
  - mcp__mesh__mesh_build_context
  - mcp__mesh__mesh_graph
  - mcp__mesh__mesh_project
  - mcp__mesh__mesh_session_start
  - mcp__mesh__mesh_note_new
  - mcp__mesh__mesh_note_append
  - mcp__mesh__mesh_task_new
  - mcp__mesh__mesh_task_append
  - mcp__mesh__mesh_note_update
  - mcp__mesh__mesh_task_claim
  - mcp__mesh__mesh_task_release
  - mcp__mesh__mesh_task_finish
  - mcp__mesh__mesh_task_update
  - mcp__mesh__mesh_memory_new
  - mcp__mesh__mesh_memory_append
  - mcp__mesh__mesh_memory_update
  - mcp__mesh__mesh_memory_get
  - mcp__mesh__mesh_memory_list
  - mcp__mesh__mesh_memory_recall
  - mcp__mesh__mesh_scratch_set
  - mcp__mesh__mesh_scratch_append
  - mcp__mesh__mesh_scratch_get
  - mcp__mesh__mesh_scratch_list
  - mcp__mesh__mesh_asset_get
  - mcp__mesh__mesh_asset_list
  - mcp__mesh__mesh_asset_attach
  - mcp__mesh__mesh_task_block
  - mcp__mesh__mesh_task_unblock
  - mcp__mesh__mesh_task_next
---

# mesh: vault coherence and coordination

mesh gives a fleet of agents one shared Markdown vault, divided into five **spaces** — notes,
tasks, memories, scratch and assets — each with its own verb family, over MCP (`mesh_*` tools)
or the `mesh` CLI. Each tool's own parameter descriptions cover what a single call does; this
skill covers what a parameter description cannot: the sequence and judgment that keeps the vault
coherent when several agents are writing to it at once.

If your client is MCP-connected, the `instructions` block it received on connect already named
your identity, the valid-owner roster, the vault path, and a coordination protocol — built fresh
from the live config, so it is always more current than anything written here. This skill does
not repeat that identity/roster/vault-path text (it cannot: only the running server knows it).
It restates and deepens the same protocol with the vault-coherence habits that make note-taking,
recall and task hand-off actually work across agents, whether or not you ever read that block.

## The eight rules

**1. Search before you write.** Before `note_new`/`task_new`/`memory_new`, run `mesh_search` (or
a tag pull, or `mesh_memory_recall` for the memories space) for the topic. A title collision
already comes back as a non-blocking `warnings` entry on the creation response — but a
near-duplicate under a *different* title never trips that check, so a search is the only thing
that catches it.

**2. Append rather than fork a near-duplicate.** If search turns up an existing note, task or
memory that already covers the ground, extend it with `note_append`/`task_append`/`memory_append`
instead of writing a second, competing version of the same knowledge. Three half-written notes on
one topic are worse than one you keep adding to.

**3. Tag from the existing vocabulary.** Pull the tags already in use (a `mesh_search` tag pull,
or `note_list`/`task_list`/`memory_list`) before inventing a new spelling. `wire-format` and
`wireformat` split one cluster into two invisible-to-each-other piles — tag pulls and filters
only work when spelling is shared.

**4. Link when a note continues another.** Put a `[[wikilink]]` to the prior note/task/memory in
the body; it populates `related`, which is exactly what `mesh_graph`/`mesh_build_context` (and
`direction: "in"` for backlinks) walk. An unlinked continuation is invisible to everyone but you.

**5. Claim before you work.** Check `claimed_by` before `task_claim`; if another agent already
holds the task, work a different one instead of claiming over them. `mesh_task_next` does the
whole selection for you — it picks a ready, unclaimed task and can claim it in the same call.
This is the instructions block's own first coordination rule, restated here because it is the one
most worth never skipping.

**6. Always finish with an outcome — `task finish --outcome` (CLI) / `task_finish`'s `outcome`
parameter (MCP).** Record what happened before a task moves to done, not just a status flip. A
later `task_get`/`mesh_graph` reader should be able to tell what the work produced without
re-deriving it from the body.

**7. Cancel is for tasks that shouldn't exist, not tasks you failed.** `task_cancel` removes a
task from the open queue for good, with a reason. Attempting a task and not finishing it is not
grounds to cancel it — append a note on the blocker (rule 2), record the real dependency with
`task_block`, or release your claim, so someone else (or you, later) can pick it up. Reach for
`cancel` only when the task itself was the mistake: a duplicate, no longer needed, overtaken by
events.

**8. Put it in the space it belongs to.** mesh checks nothing here; the choice is yours, and it
is what keeps the vault readable:

- **note** — durable knowledge about the world or the work; the operator reads it.
- **memory** — an agent's belief about the operator or the fleet; another agent recalls it. Give
  it a `kind`, an `importance` and, when it is only true for a while, an `expires`; supersede an
  old belief instead of overwriting it.
- **scratch** — this session's working state, keyed by name under your own agent namespace.
  Nobody else should ever need it: no lens and no default search will show it.
- **asset** — bytes that are not Markdown. Store the file once, then attach it to the note, task
  or memory it belongs to.

A note and a memory with the same title never warn about each other, because the duplicate-title
advisory is same-space by construction. Choosing the wrong space is silent, so choose on purpose.

## Ownership is a cooperation convention, not proof of identity

`owner` and `claimed_by` are plain strings a caller supplies. `owner` is checked for spelling
against `[tasks].collections` when that roster is non-empty — a value check, never a check on who
is actually calling — and `claimed_by` is never checked against the roster either way. Any agent
with local access can pass any `--owner`/`claimer` value; nothing in mesh verifies the caller is
who it says it is. That is by design for a trusted local fleet on one operator's machine (see the
project's own `AGENTS.md` §6) and it is not a security boundary.

Treat "claimed by someone else" as a social signal to route around — the same move rule 5 asks
for: pick a different task rather than take theirs. `task release`'s `--force` flag is CLI-only,
deliberately never exposed on `mesh_task_release`, and it can break another agent's claim; it
exists precisely because this is a convention that can be overridden, not a lock the system holds
for you. Reaching for it is choosing to override a peer, never something the system stopped
someone else from doing in the first place.

## Reading and acting on results

A `warnings` entry on a `note_new`/`task_new`/`memory_new` response names the id of a prior entry
with a matching title — go read it (rule 1) before assuming your new one is needed. A
`mesh_search` hit's `mode` field (`"indexed"` or `"fallback"`) tells you whether that result came
from ranked recall or the built-in engine; call `mesh_health` for the same answer without running
a query. A task's `ready` field on `mesh_task_get` tells you whether its blockers are all
satisfied — readiness is computed at read time from `blocked_by`, never stored, so it is always
current. `mesh_session_start` is the fastest way to see your own open/claimed queue plus inbound
mentions and the memories that matter at the start of a session — when this plugin's
`SessionStart` hook is active it already primed that view for you
(`session-start --meta-only --json`); read it before assuming nothing is waiting for you.

## Command surface this playbook assumes

CLI, one verb family per space:

- `note {new,get,list,append,update,delete}` — types `note`, `log`, `decision`, `reference`,
  `project`.
- `task {new,get,list,append,update,claim,release,finish,cancel,delete,block,unblock,next}`.
  `task list` takes `--stale`/`--available`/`--ready`/`--blocked` alongside
  `--status`/`--owner`/`--mine`/`--tags`; `task claim` takes `--strict` (exit 5 rather than
  claiming blocked work); `task finish` takes `--outcome`; `task block --on` and
  `task unblock --on|--all` edit the dependency edges; `task next` selects the next ready task
  and `--claim` takes it in the same call.
- `memory {new,get,list,append,update,recall,forget}` — `--kind`, `--scope`, `--importance`,
  `--source`, `--expires`, `--supersedes` on write; `memory recall` ranks by match, importance
  and recency and takes `--no-decay` for an audit.
- `scratch {set,get,list,append,clear}` — name-addressed per agent; `--agent` addresses a peer's
  namespace.
- `asset {add,get,path,list,attach,detach,remove,gc}` — `asset add --attach` stores and links in
  one call; `asset path` prints the blob path to pipe into another tool.

Plus `search` (`--space`, `--engine`, `--tags`, `--health`), the session lenses
`recent-activity`, `build-context`, `graph --direction out|in|both`, `project`,
`session-start --team|--budget`, and the human-only admin commands `init`, `status`,
`config {path,show,get,set}` and `watch` (a foreground watcher that keeps the search index and
folder routing fresh; mesh has no daemon, and every command works without a watcher running).

`--tags` on `note update`/`task update`/`memory update` merges by default (bare `x,y` adds;
`+x,-y` is a delta — `-y` removes exactly the tags you name; `=x,y` replaces the whole list, so
anything left out of the new list is discarded).

MCP mirrors the safe subset as 37 typed `mesh_*` tools — this skill's `allowed-tools` list, plus
`mesh_task_cancel`, which is destructive and left out of that pre-approved list on purpose so it
always asks first. Every removal verb (`note delete`, `task delete`, `memory forget`,
`scratch clear`, `asset remove`), asset ingest (`asset add`, which reads an arbitrary path on
your filesystem), `asset gc` and every admin command (`init`, `status`, `reindex`, `watch`,
`config`, `completions`) are CLI-only, withheld from MCP entirely — a hard `unlink` with no trash
is never something this skill pre-approves, and never something an MCP client can reach.
