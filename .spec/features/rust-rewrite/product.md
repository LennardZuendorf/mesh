---
type: feature-product
feature: rust-rewrite
sibling: tech.md
parent: ../../product.md
updated: 2026-09-05
---

# Feature: Rust Rewrite — Product

Mesh is re-implemented as a single Rust binary that works over one Markdown folder and exposes
that folder to agents as a set of named **spaces** — notes, tasks, memories, scratch, assets —
each configurable, each with its own verb family. The daemon is removed; every command reads
disk directly. The task dependency graph, deferred since Phase 1, ships. The surface grows from
three verbs to one verb family per space plus search, read-only lenses, admin and an MCP server,
while every vault and every command shape the Python implementation produced keeps working.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The Rust binary and its whole command surface (note, task incl. the dependency graph, memory, scratch, asset, search, the five lenses, admin, the stdio MCP server); the `[spaces]` configuration model; the space resolution and sandbox rules; the frontmatter read/write contract for all five spaces; the exit-code table and the JSON error envelope; the optional foreground watcher; compatibility with Python-era vaults and configs; removal of the Python implementation, its daemon and its packaging |
| **Does not own** | The `indexed` engine itself (mesh only wraps its CLI); the vault's contents, versioning, sync or backup; any notes application; the agent playbooks' *content* beyond the surface facts they must state; a package registry or install channel |
| **Deferred** | Windows support beyond best-effort (locks and the watcher are POSIX-shaped); an `indexed` library binding in place of the subprocess; a `mesh doctor` verb (`status` plus `config show` cover it); per-space retention or archival policy for memories |

---

## Requirements

### Requirement: R1 — One Rust binary, instant start

Mesh SHALL ship as a single self-contained binary with no runtime interpreter, no background
process and no service dependency. A read command MUST complete its startup work in under 10 ms
of wall clock on a warm filesystem, and MUST NOT pay the cost of any surface it does not use.

#### Scenario: A cold command on a large vault

- **Given** a vault of several thousand Markdown files and no process of mesh's running
- **When** an agent asks for a note by id
- **Then** the note is returned, the process exits 0, and startup cost is under 10 ms

#### Scenario: The MCP surface costs nothing on the CLI path

- **Given** any non-MCP command
- **When** it runs
- **Then** the MCP tool table, its schemas and its instructions text are never constructed

### Requirement: R2 — Configurable spaces

The vault SHALL be divided into five named spaces — notes, tasks, memories, scratch, assets —
each of which MUST be configurable to a folder relative to the vault root, an absolute folder,
the vault root itself, or disabled. Every key MUST be optional; an absent configuration MUST
resolve to the same layout the Python implementation used. Space folders MUST be created lazily
on first write, never at start-up.

#### Scenario: A Python-era config with no spaces block

- **Given** a config written before this feature existed
- **When** any command runs
- **Then** notes and tasks resolve to the folders they always did, and no new folder is created

#### Scenario: A disabled space

- **Given** the scratch space is configured as disabled
- **When** any scratch verb is invoked
- **Then** the command fails validation, names the disabled space, and writes nothing

#### Scenario: Two spaces pointing at one folder

- **Given** two enabled spaces that resolve to the same directory
- **When** the configuration is loaded
- **Then** the command fails validation before any file is read or written

#### Scenario: A space escaping the vault

- **Given** a relative space path that resolves outside the vault root
- **When** the configuration is loaded
- **Then** the command fails validation; an explicitly absolute space root is accepted and is
  reported by the status and config views

### Requirement: R3 — The notes space may be the whole vault

The notes space SHALL be configurable as the vault root so an existing Markdown vault is exposed
as-is. When it is, every other enabled space MUST be a strict subfolder and MUST be excluded from
the notes walk. Markdown files carrying no mesh frontmatter ("foreign" files) SHALL be readable
and searchable with a null id and a title derived from the first heading, else the filename, and
MUST remain invisible to every mutating verb and to the default listing.

#### Scenario: Listing an Obsidian vault as the notes space

- **Given** the notes space is the vault root and the tasks space is a subfolder of it
- **When** notes are listed
- **Then** task files are not listed as notes

#### Scenario: Reading a hand-written file

- **Given** a Markdown file with no frontmatter in the notes space
- **When** it is requested with the foreign flag
- **Then** its body is returned with a null id and a derived title

#### Scenario: A foreign file is never mutated

- **Given** the same file
- **When** an append, update or delete addresses it
- **Then** the command reports not found and the file is untouched

#### Scenario: Hidden and hostile files are skipped

- **Given** the notes space contains dot-directories, a 50 MB Markdown-named file and a
  non-UTF-8 file
- **When** any walk runs
- **Then** all three are skipped silently — never listed, never searched, never an error

### Requirement: R4 — Note verbs

The note family SHALL provide create, append, update, get, list and delete over the notes space,
preserving every flag, default, output row, ordering rule and exit code of the Python surface.
Update MUST additionally accept a new title, and when a rename would strand title-form links it
MUST emit one advisory naming the affected entities without refusing the rename and without
rewriting any body.

#### Scenario: A note is created and read back

- **Given** an empty notes space
- **When** a note is created with a title and a body and then requested
- **Then** the same id, title and body come back, and machine output carries the frontmatter in
  the declared key order

#### Scenario: Renaming a linked note

- **Given** two notes whose bodies link to a third by title
- **When** the third is renamed
- **Then** the rename succeeds and one advisory on the error stream names the count and the
  linking ids, and no payload contains it

### Requirement: R5 — Task lifecycle verbs

The task family SHALL provide create, update, append, claim, release, finish, cancel, get, list
and delete with the Python semantics preserved exactly: claim is an atomic test-and-set, a
second claim by the same holder is a no-op that rewrites nothing, a claim held by another agent
is a conflict, finish and cancel are idempotent and append outcome sections, and a terminal file
stranded in the open folder is reconciled on the next write.

#### Scenario: Two agents race one task

- **Given** eight processes claiming the same open task simultaneously
- **When** they all run
- **Then** exactly one succeeds and the rest report a claim conflict, and the file records one
  holder

#### Scenario: Finishing twice

- **Given** a finished task
- **When** finish runs again
- **Then** the command exits 0, no second outcome section is written and the modification
  timestamp does not move

### Requirement: R6 — Live task dependency graph

Task readiness SHALL be derived at read time and never stored. A task is ready when it is open,
unclaimed, and every blocker named by either edge direction is done, cancelled or absent.
Dedicated block and unblock verbs MUST add and remove edges additively and idempotently, MUST
reject self-edges and cycles before any write, and MUST NOT hold two entity locks at once. No
verb may rewrite another entity's file as part of its own transaction: the unblock cascade is
**reported**, not written. Listings MUST expose ready and blocked filters, and a strict claim on
a blocked task MUST fail with the dedicated blocked exit code, writing nothing.

#### Scenario: Finishing a blocker

- **Given** two tasks blocked by a third
- **When** the third is finished
- **Then** the two become ready without their files being rewritten, and the finish reports the
  newly ready ids

#### Scenario: A one-sided hand-written edge

- **Given** a task whose blocked-by list was hand-edited to name a blocker that itself lists
  nothing
- **When** readiness is computed
- **Then** the task is correctly reported as blocked

#### Scenario: A blocker that does not exist

- **Given** a task blocked by an id no file carries
- **When** readiness is computed
- **Then** the missing blocker counts as satisfied, the task is ready, and the dangling blocker
  is counted by the status view

#### Scenario: A cycle is refused, not created

- **Given** an edge that would close a dependency cycle
- **When** it is added
- **Then** the command fails validation naming the cycle path and no file is written

#### Scenario: A strict claim on a blocked task

- **Given** an open task with an unsatisfied blocker
- **When** it is claimed strictly
- **Then** the command exits with the blocked code, names the unsatisfied blockers, and the task
  stays unclaimed

### Requirement: R7 — A single "give me work" primitive

The task family SHALL provide a next verb that selects the highest-ranked ready task under the
caller's filters and can claim it in the same invocation, re-selecting across further candidates
when a claim races another agent. With nothing ready it MUST exit with the not-found code so a
caller can branch on the status.

#### Scenario: Two agents draining one queue

- **Given** two ready tasks and two agents invoking next with claim simultaneously
- **When** both run
- **Then** each ends up holding a different task and neither reports a conflict

#### Scenario: An empty queue

- **Given** no ready task
- **When** next runs
- **Then** it exits with the not-found code and says so on the error stream

### Requirement: R8 — Memory verbs

The memories space SHALL hold note-shaped Markdown files addressed by id or title slug, with a
family of create, append, update, get, list, recall and forget. Every memory MUST carry a kind,
a scope, an importance, an optional source, an optional expiry and an optional supersession
pointer alongside the shared base fields. Private-scope memories MUST be hidden from listings
whose effective identity differs — a courtesy filter, never an authorisation boundary.

#### Scenario: An agent records and lists a belief

- **Given** an empty memories space
- **When** a memory is created with a kind and an importance and then listed
- **Then** it appears with those values and the id is addressable by its title slug

#### Scenario: A peer's private memory

- **Given** a private memory owned by another identity
- **When** memories are listed as this identity
- **Then** the memory is absent from the listing

### Requirement: R9 — Recall ranking

Recall SHALL rank matches by relevance weighted by importance and decayed by recency measured
from the last update, and MUST emit the same hit shape the search verb emits so one parser
serves both. Decay MUST affect ranking only and MUST NOT remove or delete anything, and MUST be
suppressible for audit.

#### Scenario: A re-confirmed memory outranks a stale one

- **Given** two equally matching memories of equal importance, one updated today and one a year
  ago
- **When** recall runs
- **Then** the recently updated one ranks first

#### Scenario: Auditing without decay

- **Given** the same two memories
- **When** recall runs with decay disabled
- **Then** ordering is by match and importance alone

### Requirement: R10 — Supersession and expiry, never auto-deletion

Creating a memory that supersedes another MUST mark the old one superseded and keep it on disk.
Expired and superseded memories MUST be excluded from recall and from default listings and MUST
remain retrievable behind explicit flags. Nothing SHALL ever delete a memory except an explicit
forget under the delete guard.

#### Scenario: Revising a belief

- **Given** an existing memory
- **When** a new memory supersedes it
- **Then** both files exist, the old one is marked superseded, and only the new one is recalled

#### Scenario: An expiry passes

- **Given** a memory whose expiry is in the past
- **When** memories are listed or recalled
- **Then** it is absent by default, present with the include-expired flag, and still on disk

### Requirement: R11 — Scratch

The scratch space SHALL hold per-agent, name-addressed working files with set, append, get, list
and clear. Set MUST overwrite the whole body idempotently; get MUST return the body verbatim with
no truncation; a name that normalises to empty MUST fail validation. Scratch MUST be excluded
from the default search corpus and from every lens, and MUST be addressable across agents by an
explicit identity flag.

#### Scenario: Session state written and read back

- **Given** an agent identity
- **When** a scratch file is set and then read
- **Then** the exact body comes back, with no preview truncation

#### Scenario: Scratch stays out of the lenses

- **Given** scratch files exist
- **When** recent activity, graph, build-context, session-start or a default search runs
- **Then** no scratch file appears

### Requirement: R12 — Content-addressed assets

Asset ingest SHALL copy the source bytes into the assets space under an id derived from the
content digest, writing the blob before its Markdown sidecar, and MUST be idempotent by content:
identical bytes return the existing id, write nothing and do not move any timestamp. The original
filename MUST be preserved as data and MUST never form part of a path. A crash between the two
writes MUST leave at most a blob with no sidecar, never a sidecar with no blob.

#### Scenario: The same file added twice

- **Given** an asset already stored
- **When** the identical file is added again
- **Then** the same id is returned, nothing is written, the command exits 0 and an advisory says
  so

#### Scenario: A hostile filename

- **Given** a source file whose name contains path separators or traversal segments
- **When** it is added
- **Then** the stored blob path is derived only from the id, and the original name survives as
  metadata

### Requirement: R13 — Attaching assets to entities

Assets SHALL be attachable to a note, task or memory by appending an embed to the target body
through the ordinary append path, so link-derived relations and the graph lenses pick the pair up
with no special case, and detachable by removing the relation without editing the body. Removal
of an asset still referenced by another entity MUST be refused as a validation failure unless it
is forced, and a garbage-collect view MUST report orphaned blobs and orphaned sidecars read-only
unless explicitly applied.

#### Scenario: Attach then traverse

- **Given** an asset and a note
- **When** the asset is attached to the note
- **Then** the note's relations name the asset, and the graph lens shows the pair in both
  directions

#### Scenario: Removing a referenced asset

- **Given** an asset attached to a note
- **When** removal is attempted without force
- **Then** the command fails validation, names the reference count, and removes nothing

### Requirement: R14 — Search across spaces and engines

Search SHALL keep its current contract — always one JSON array on standard output, the same hit
keys, the same conditional keys, the same tag-pull behaviour with no query, the same health
short-circuit — and MUST gain a space filter, a memory-kind filter and an explicit engine
selector. The default corpus MUST be byte-identical to today's for a vault that has no memories
or assets folder. The built-in engine MUST rank better than a substring scan while provably never
losing a hit the substring scan would have returned, and a substring mode MUST restore the legacy
scoring exactly.

#### Scenario: A legacy vault searches identically

- **Given** a Python-era vault with only notes and tasks
- **When** a query runs with no new flags
- **Then** the hit key set and the ordering match the legacy contract

#### Scenario: Ranked multi-word query

- **Given** a multi-word query that matches no single literal substring
- **When** search runs on the default engine
- **Then** relevant entities are returned, ranked, instead of an empty array

#### Scenario: Searching one space

- **Given** notes, tasks and memories all matching a query
- **When** search is restricted to memories
- **Then** only memories are returned and each hit names its space

### Requirement: R15 — The `indexed` wrapper

When a search collection is configured, hybrid search is enabled and the engine binary is
reachable, search SHALL delegate ranking to it by subprocess with byte-identical arguments and
no shell, decode its streamed results tolerantly — skipping malformed records rather than
failing — re-check every returned path against the sandbox and the caller's filters, and report
the branch actually taken. An engine invocation MUST be bounded by a wall clock; a timeout,
a missing binary or a failure MUST degrade to the built-in engine with one advisory and MUST
never be an error.

#### Scenario: The engine hangs

- **Given** an engine binary that never returns
- **When** a query runs
- **Then** mesh degrades to the built-in engine within the time bound, emits the standard
  advisory, and exits 0

#### Scenario: The engine returns junk

- **Given** a result stream containing blank lines, a malformed record and a record whose score
  is a boolean
- **When** a query runs
- **Then** those records are skipped and the remaining valid ones are returned

### Requirement: R16 — Read-only lenses

The five lenses — recent activity, build context, graph, project and session start — SHALL keep
every flag, default, row format, ordering rule, dedup rule and edge orientation of the Python
surface, MUST accept a space filter, and MUST never write to the vault. The stale infrastructure
notice about a missing daemon MUST be removed.

#### Scenario: A lens on a vault with a memory relation

- **Given** a note whose relations name a memory
- **When** the graph lens traverses from the note
- **Then** the memory is a node and the edge is present

#### Scenario: Lenses never write

- **Given** any vault state
- **When** every lens runs
- **Then** no file's bytes, modification time or lock changes

### Requirement: R17 — Session start with memories and a budget

Session start SHALL compose the caller's tasks, mentions, memories and recent activity into one
flat array in that order, deduped by id with the earlier section winning and a reason on every
entry. Memory entries MUST be capped, selected by importance then recency over non-expired,
non-superseded, visible memories, and MUST be suppressible. A character budget MUST trim bodies
before whole entries and, when it trims, MUST append one synthetic entry recording how many were
dropped so the array shape stays constant.

#### Scenario: A warm start with memories

- **Given** open tasks, mentions and several memories
- **When** session start runs
- **Then** the array carries tasks first, then mentions, then at most the memory cap, then
  activity, each entry naming its reason

#### Scenario: A budget is exceeded

- **Given** a payload larger than the requested budget
- **When** session start runs under that budget
- **Then** bodies are dropped before entries, and the final entry records the number dropped

### Requirement: R18 — Admin surface

Mesh SHALL provide init, status, reindex, config and shell completions. Init MUST render a
config, MUST create only the vault root, and MUST refuse an existing config without force
without ever opening it for writing. Status MUST be strictly read-only and MUST report every
space, its resolved location, per-space counts, dependency health and watcher liveness, with the
existing keys keeping their positions. Config MUST expose the effective configuration including
every resolved space path and the sandbox roots, and MUST edit in place preserving comments and
ordering.

#### Scenario: Initialising over an existing config

- **Given** a config file already exists
- **When** init runs without force
- **Then** it fails validation, says so, and the existing file is not opened for writing

#### Scenario: An agent asks where a write will land

- **Given** any configuration
- **When** the effective config is shown
- **Then** every space's resolved location and every sandbox root is listed

### Requirement: R19 — The daemon is removed; the watcher is optional

The daemon, its socket, its warm index and its client-fallback matrix SHALL be removed. Every
command MUST read disk directly and MUST behave identically whether or not any watcher process
exists. A foreground watcher MAY be run purely to keep the external search index and folder
routing fresh; it MUST be a singleton per vault, MUST debounce per path, and MUST swallow every
error rather than dying. The legacy daemon commands MUST survive as a hidden, non-spawning shim
that preserves their keys, strings and exit codes so no existing script breaks.

#### Scenario: Every command works with nothing running

- **Given** no watcher and no other mesh process
- **When** any command runs
- **Then** it behaves exactly as it does with a watcher running

#### Scenario: A second watcher

- **Given** a watcher already running on this vault
- **When** another is started
- **Then** it exits with the conflict code and names the running process

#### Scenario: A legacy script calls the daemon verbs

- **Given** a script that starts, queries and stops the daemon
- **When** it runs
- **Then** every invocation exits 0 with the documented keys, nothing is spawned, and one
  advisory points at the watcher

### Requirement: R20 — MCP server

Mesh SHALL serve the agent surface over stdio as an MCP server exposing 37 tools: the 21 legacy
tools unchanged in name, parameters, descriptions and return shapes, plus 16 covering memories,
scratch, assets and the task graph. Every tool MUST carry explicit read-only, idempotent and
destructive hints, and exactly one tool may be destructive. Every removal verb and every admin
verb MUST be withheld. Failures MUST cross the boundary as the same structured payload the CLI
emits, never as a crash.

#### Scenario: A client lists the tools

- **Given** a connected MCP client
- **When** it lists tools
- **Then** 37 tools are returned, each with a described parameter set and explicit hints, and no
  tool name refers to deleting, the daemon, reindexing or status

#### Scenario: A tool fails with no configuration

- **Given** no config file anywhere
- **When** any tool is called
- **Then** the call returns the structured config-missing payload and the server keeps serving

### Requirement: R21 — Python-era vaults keep working

Everything the Python implementation wrote SHALL be readable: its frontmatter spellings,
timestamp forms, quoted and bare scalars, unknown keys including one literally named `extra`,
its folder layout and its ids. Mesh MUST NOT rewrite a file it was not asked to change, MUST NOT
recompute an existing id, and when it does amend a file MUST change only the fields the command
touches while preserving every unknown key and its order.

#### Scenario: Reading a legacy vault

- **Given** a vault written by the Python implementation containing every note type, tasks in
  every status, unknown keys, bare dates, naive and offset timestamps and unicode titles
- **When** every listing and read verb runs
- **Then** every entity is surfaced with the same values and in the same order as before

#### Scenario: A read never writes

- **Given** the same vault
- **When** any read verb runs over every file
- **Then** no file's bytes or modification time change

#### Scenario: An amend is minimal

- **Given** a legacy file with unknown keys
- **When** a body is appended to it
- **Then** only the update timestamp, the derived relations and the appended block differ, and
  every unknown key keeps its value and position

### Requirement: R22 — Exit codes

Mesh SHALL map every outcome onto a fixed status: 0 success, 1 infrastructure failure or a
declined confirmation, 2 validation, 3 not found, 4 claim conflict or contended lock, 5 blocked.
A file whose frontmatter cannot be parsed MUST read as not found on every read and amend verb
while remaining deletable. No failure path may ever print a runtime crash trace.

#### Scenario: The full matrix

- **Given** one invocation per documented failure
- **When** each runs
- **Then** each exits with its mapped status and no output contains a crash trace

#### Scenario: A corrupt file

- **Given** an entity whose frontmatter is malformed
- **When** it is read or amended
- **Then** the command reports not found; when it is deleted, the deletion succeeds

### Requirement: R23 — JSON error envelope

Under machine output a failure SHALL write exactly one JSON object to the error stream, carrying
a kind, a message, a suggested next action and the structured fields relevant to that failure,
and MUST exit with the same status the human path would. A not-found failure MUST offer nearest
candidates; a lock conflict MUST say when to retry. No suggested action may read as an
authorisation decision. The MCP surface MUST render the identical object.

#### Scenario: A mistyped id

- **Given** an id close to two existing ids
- **When** it is requested with machine output
- **Then** the error object names the kind, the message, a next action and the candidate ids,
  and the command exits with the not-found status

#### Scenario: Human output is unchanged

- **Given** the same failure without machine output
- **When** it runs
- **Then** the error stream carries the same plain-text message it always did

### Requirement: R24 — Identity and roster across every space

The caller's identity SHALL come from the environment or an explicit flag, MUST default the
owner on create and filter on list, and MUST be validated against the configured roster at the
single write boundary for every space — notes, tasks, memories, assets and the scratch namespace.
An empty roster MUST disable the check. Identity is a spelling convention, never an
authorisation boundary, and every identity that becomes part of a path MUST be normalised first.

#### Scenario: A typo'd identity

- **Given** a non-empty roster
- **When** a memory is created with an identity that is not in it
- **Then** the command fails validation before any write

#### Scenario: An open roster

- **Given** an empty roster
- **When** any identity is used
- **Then** no validation is applied

### Requirement: R25 — Safe writes

Every write SHALL be atomic and MUST preserve an existing file's mode; every mutation of an
entity MUST hold that entity's lock and MUST re-resolve the target inside the lock; a lock MUST
be reclaimed only when provably stale and MUST be released only when it is still the same file.
Every path a command touches MUST resolve inside the union of the enabled space roots. Agent
content is data and MUST never be interpreted as a command or shell input.

#### Scenario: Concurrent appends

- **Given** several processes appending to one entity simultaneously
- **When** they all run
- **Then** every block lands and no content is lost

#### Scenario: A path escaping the sandbox

- **Given** a target that resolves, through symlinks or traversal, outside every enabled space
- **When** it is addressed
- **Then** the command fails validation naming the root it was checked against

---

## Outputs

- One binary, plus a thin compatibility binary for the MCP server entrypoint.
- Per-space folders under the operator's vault, created lazily, containing plain Markdown.
- A configuration file gaining an optional spaces table; existing configs remain valid unedited.
- Human text on standard output, machine JSON on demand, infrastructure and advisories on the
  error stream, a fixed exit-code contract, and a stdio MCP server.
- The Python implementation, its daemon, its packaging and its test suite are deleted.

---

## Non-Goals

- **No daemon.** No background process, no socket, no warm index, no client-fallback matrix.
  The watcher is optional and only keeps the external index and folder routing fresh.
- **No database and no memory subsystem.** Memories are Markdown files in a folder; there is no
  store, no embedding cache mesh owns, and no synthesis or enrichment loop.
- **No automatic deletion.** Expiry and decay affect visibility and ranking only. Nothing removes
  a memory but an explicit forget.
- **No move or link ingest for assets.** Ingest copies. Mesh never unlinks an operator's source
  file and never stores a reference to a path outside the vault.
- **No byte-level compatibility with the old serialisers.** Mesh reads everything the Python
  implementation wrote, but writes its own canonical frontmatter (declaration key order, RFC
  3339 UTC timestamps, a trailing newline) and compact machine JSON. Compatibility is semantic —
  same keys, same values, same ordering contracts — not whitespace-identical.
- **No new top-level primitive.** The spaces are five folders behind one binary; there is no
  handoff, queue, dashboard, permission model or sync layer.

---

## Open Questions

1. **Windows.** Locks, the watcher and the identity-to-path normalisation are POSIX-shaped.
   Recommendation: ship POSIX-first, treat Windows as best-effort, and revisit if an operator
   actually asks.
2. **Memory volume.** Nothing prunes memories. A fleet writing memories in a hot loop grows the
   space without bound. Recommendation: watch it in practice; a retention lens is cheaper to add
   later than a deletion policy is to take back.
