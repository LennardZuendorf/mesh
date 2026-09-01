---
type: entrypoint
scope: technical
children:
  - plan.md
updated: 2026-08-23
---

# Shards — Technical Architecture

Daemon-centric, never daemon-dependent. Warm index + watcher in the daemon; writes in `core`/`storage`; ranking in `indexed`. CLI and MCP are thin socket clients with file-op fallback.

---

## Stack

| Layer | Choice |
|---|---|
| Runtime | Python 3.11+, `uv` / `pipx` |
| CLI | `typer` |
| Data | Markdown + `python-frontmatter`, `msgspec` |
| Watcher | `watchdog` |
| Agents | `FastMCP` |
| Vault | Any Markdown folder (operator-owned; Obsidian vault works as-is) |
| Search engine | `indexed` (first-party hybrid; Shards wraps) |

**Added deps only:** typer, python-frontmatter, msgspec, watchdog, FastMCP. Shards code stays small — wrapper, daemon, locks, wikilinks.

**Vault requirement.** Shards needs only a directory it can write `notes/`/`tasks/` into — no notes application need be installed, running, or detected; `shards init` creates the folder when none exists. Obsidian is the maintainer's reference pairing, not a dependency: it is a supported value for `[core].vault_path`, not a requirement any code path checks for.

---

## Layout

```
src/shards/
├── cli/       # note, task, search, daemon, status, session (graph/project lenses)
├── mcp/       # shards_* tools
├── daemon/    # socket server, client shim, owns ChangeHooks
├── core/      # notes, tasks, ids, wikilinks, activity, context, search, lenses
├── index/     # indexed_client, tagpull, fallback, warm, watcher, reconcile
├── schemas/   # note, task, config, search (msgspec Structs)
└── storage/   # atomic files, locks, sandbox
```

---

## Invariants

1. Daemon accelerates; never gates writes. A write lands on disk first and only
   then best-effort-notifies a running daemon (`vault.touch`) so the writer's own
   next read sees it; every failure of that notification is swallowed, and with no
   daemon running it costs an immediately-failing connect. Freshness may degrade,
   a write never can.
2. Writes: temp-file + `os.replace`; idempotent where stated. The temp file takes
   the destination's mode when one exists (else the umask default) — shards never
   silently narrows a file another tool owns.
3. Clean Markdown; unknown frontmatter keys round-trip. Frontmatter is dumped
   anchor-free (`&id001`/`*id001` are valid YAML but unreadable to restricted
   parsers), so what shards writes stays plain for every other tool on the folder.
4. One folder, one search path; rank in `indexed`.
5. Agent content is inert data.
6. Instant CLI: daemon-only deps (`watchdog`) import lazily inside the daemon path, never at CLI import time.
7. Read-lenses serve from the warm index when the daemon is up; a disk re-parse happens only on the daemon-down fallback.
8. One vault, one socket. The daemon socket is named from a digest of the resolved
   `vault_path` and every reply names the vault it served; a mismatch degrades to
   the file-op fallback. One daemon per *vault*, never one per user.
9. `vault_path` is canonicalised once, at the config boundary (`expanduser` then
   `resolve`), so walkers, scope predicates, the sandbox and the watcher all speak
   one path space. A vault that does not exist yet is legal (created lazily); one
   that exists and is not a directory is a validation error.
10. Shards owns the interface, not the vault — versioning, sync, and backup are the vault owner's job. That is the basis for hard delete (`unlink`, no trash, no promised recovery) and for skipping/round-tripping any file or key shards did not write, for any tool sharing the folder.

---

## Performance

**Goal:** instant CLI. **Target:** ~150–180ms cold start (the msgspec path, below) — the honest
floor, not the aspiration. ~100ms is the "feels instant" UX threshold, not a number literally
reachable while keeping `typer` + a schema-validator class. **Principle:** heavy work lives in the
warm daemon; the CLI import path pays only for what it uses (invariant 6). `watchdog` stays
lazy-imported (daemon-only). Measured non-levers, no action needed: `FastMCP` is already off the
CLI hot path (separate `shards-mcp` console script); `python-frontmatter` already uses PyYAML's C
loader. The dominant cost was `pydantic` v2's one-time schema-compile tax, paid the moment the
first `BaseModel` subclass is defined — unavoidable via lazy-import alone, since any real command
needs config. Fixed by swapping `schemas/` to **msgspec** (~90ms saved), gated on a
round-trip-fidelity spike protecting invariant 3. Keep-and-optimize: the runtime stays Python
3.11+, the existing daemon + hybrid-search architecture is tuned, not restructured — a Rust
rewrite was evaluated and **shelved** (see [Risks](#risks)).

**Optimization tactics — measured, decided, shipped:** stop wrapping hot invocations in
`uv run`; swap `schemas/` pydantic → msgspec (gated on the round-trip-fidelity spike, which
passed); decompose eager CLI sub-verb imports (hygiene, not perf); CI startup-time regression
guard. `daemon/client.py` is lazy-imported from `core` — a write pays for the socket client only
when it actually writes.

---

## Shared primitives (DRY)

A task is a note with `type: task`, so every cross-cutting mechanic has **one** implementation the two verbs share — never per-verb copies (copies drift; the spec's whole thesis is a small, single-surface core):

- **Lock-hold** — the bounded wait-retry over the `O_EXCL` lock lives in `storage/locks.py`; `core/notes.py` and `core/tasks.py` import it.
- **Safe read** — read-text + `frontmatter.loads` guarding **both** `OSError` and `yaml.YAMLError` (foreign/corrupt files skip silently) is one reader (`storage/files.read_post`); every scanner (`tagpull`, `activity`, `wikilinks`, `scan_recent`) routes through it.
- **Vault walk** — one iterator over `notes/**` + `tasks/{open,done}/`; all scanners consume it.
- **CLI output** — one `cli/_output.py` surface (`emit_mutation`, `preview`, `is_json`/`is_quiet`/`is_machine`, delete-guard); `note` and `task` render through it. The interactive-tty check stays a per-module seam so the delete tests can fake it.
- **Frontmatter is serialized from the schema model** (msgspec `Struct`), never a parallel hand-built dict — the schema is the single on-disk contract.
- **Terminalize** — `finish`/`cancel` are thin wrappers over one `_terminate_task(*, heading, status, text)`.
- **Exit codes** are a fixed convention — `2` validation/ambiguous, `3` not-found, `4` claim-conflict — that each CLI handler maps its domain exceptions to at the boundary.

---

## Contracts

| Contract | Module | Rule |
|---|---|---|
| Schemas | `schemas/note.py`, `task.py` | Task = note + `type: task`. See field tables below. `updated` bumps on writes; watcher reconcile does not. |
| IDs | `core/ids.py` | `n-`/`t-` + Crockford b32 hash of `created_iso\0title` (4+ chars, extend on collision). Never sequential. |
| Folders | `storage/files.py` | `type`/`status` drives path; watcher reconciles mismatch. Notes: `note→notes/`, `log/decision/reference→notes/{logs,decisions,references}/`, `project→notes/projects/`. Tasks: `open\|claimed→tasks/open/`, `done\|cancelled→tasks/done/`. |
| Atomic write | `storage/files.py` | temp + `os.replace`. `finish`/`cancel` order write-then-move so a crash mid-op never strands the file in an unrecoverable state (a terminal status must not sit in `tasks/open/` where the idempotent no-op refuses to move it). |
| Locks | `storage/locks.py` | `O_EXCL` per entity; stale if PID dead or >300s; `shards status` reports count. **Both** removals are compare-and-swaps, never blind unlinks: reclaiming a peer's stale lock and releasing your own each hold an open descriptor, take an exclusive `flock`, and unlink only if the file at the path is still that same (device, inode) file. An open descriptor is what makes the inode comparison mean something — an inode cannot be recycled while a descriptor refers to it; a swap that predates the descriptor is caught by the second `_is_stale` re-check instead. Both guards are load-bearing, for different windows. The watcher's reconcile move takes the same per-entity lock, non-blocking. |
| Sandbox | `storage/sandbox.py` | `realpath` must stay in `vault_path` |
| Exit codes | `cli/` | 0 ok · 1 error · 2 validation · 3 not found · 4 claimed · 5 blocked (`--strict` only, Phase 3). Codes live on the domain exception classes; the CLI boundary maps them once. |
| Config | `~/.shards/config.toml` | `[core]` `vault_path`+agent · `[search]` collection, hybrid, threshold · `[tasks]` collections. `vault_path` is `expanduser()`'d **then** `resolve()`'d at the parse boundary; `path` and `tolaria_path` are permanent input aliases (precedence: `vault_path` > `path` > `tolaria_path`, no warning). A non-directory vault root → exit 2; a not-yet-existing one is created lazily by the first write. `$SHARDS_AGENT` overrides agent. Missing config → exit 2. Test override: `SHARDS_CONFIG_PATH`. `[search].threshold` applies **only when explicitly set** — `shards init` therefore omits the key rather than baking in the default, which would re-disable the body/tag tiers of the substring fallback. |
| Socket | `daemon/server.py`, `client.py` | NDJSON RPC over a `0600` unix socket at `$XDG_RUNTIME_DIR/shards-<sha256(vault)[:12]>.sock` (else `~/.shards/run/`). Named per-vault and every reply carries the served vault, so a daemon on another vault can never answer this one's reads. Reads hold a single monotonic deadline (not a per-`recv` timeout) and a reply ceiling. Startup refuses to unlink a socket a live daemon still answers on. Envelope + method table under [Implemented surfaces](#implemented-surfaces) |

### Note fields

`id`, `type` (note|log|decision|reference|project), `title`, `tags`, `owner`, `created`, `updated`, `related`

### Task adds

`status` (open|claimed|done|cancelled), `priority`, `claimed_by` (`~`), `blocks`, `blocked_by` (inert v1), `project` (`str | None`, optional — id of a `type: project` note; round-tripped like any optional key)

**Appends:** finish → `## Outcome` + ISO; cancel → `## Cancelled` + ISO (optional text).

---

## Build order

`notes → tasks → daemon → search → memory` — **all implemented (Phase 1–2).** `tasks-graph` deferred to Phase 3.

Implementation is the source of truth: `src/shards/` + `tests/` (1455 tests, branch coverage on,
`ty` clean, `ruff` clean).

---

## Implemented surfaces

Contracts compounded from the (now-deleted) feature specs. Full detail lives in the code + tests cited.

- **Wikilinks** — `core/wikilinks.py`. `[[Title]]` → id by title match; `[[n-id]]`/`[[t-id]]` passthrough; `related` deduped; unresolvable → dangling, counted by `shards status`.
- **RPC** — `daemon/server.py`, `client.py`. NDJSON, one JSON object per line. Request `{id,method,params}` · ok `{id,ok:true,result}` · err `{ok:false,error:{code,message}}`. Connect-then-fallback: every read has a daemon-down fallback, writes bypass the socket entirely. A `503` (reserved-but-unwired method) makes the client run its file-op fallback; a `404` (unknown) propagates. Every reply also names the vault the daemon serves; a mismatch is treated exactly like a transport failure. Methods: `ping`, the four wired reads (`note.list`, `task.list`, `activity.recent`, `vault.status`, `search.tag_pull`) and `vault.touch` — the one *write-side* method, by which a writer tells the daemon which path it just changed so its own next read is not racing inotify delivery. Point reads, ranking and rebuilds are deliberately absent: none gets faster for crossing a socket.
- **Search** — `index/indexed_client.py`, `fallback.py`, `tagpull.py`. Result `{id,type,title,score,tags?,owner?,updated?,snippet?,path}`; foreign files surface with `id:null`. Hybrid via `indexed` when daemon-up **and** `[search].hybrid`, else the substring fallback scored title-exact 1.0 · title-substring 0.8 · tag 0.6 · body 0.4, sorted score desc then `updated` desc (recency tiebreak within 0.02). `indexed_client.incremental_update` runs on the watcher hook; `shards reindex` → `full_rebuild`.
- **MCP** — `mcp/server.py`, launched via `shards-mcp` (or `python -m shards.mcp.server`). Typed `shards_*` tools mirroring the *safe* verbs plus the read-only lenses (`shards_graph`, `shards_project`), each carrying an annotation: read-only / idempotent / write / destructive (`task_cancel`). `task_release` **is** exposed (idempotent, no `force` parameter — force is a human's call). Withheld from agents: both delete verbs, `daemon`, `reindex`, `status`, and `init`, which writes the very config every other command depends on. Every tool parameter carries a non-empty description in the generated JSON Schema, enums render domain literals, and the server sends a config-derived `instructions` block on connect (identity, valid-owner roster, vault path, live recall mode) that degrades to naming `shards init` rather than failing when no config exists. Domain failures cross the boundary as structured `{kind, message, next_action}` payloads (`claim_conflict`, `lock_conflict`, `not_found`, `config_missing`), never stack traces.
- **Session lenses** — grouped in `core/lenses.py` (re-exporting `core/activity.py` + `core/context.py`), plus `cli/session.py`, `hooks/session_start.json`. `recent-activity` (warm index, or an equivalent folder scan when down); `build-context` (daemon-free BFS over `related` to `--depth`, cycle/diamond-deduped, seed first); **`graph`** (`shards graph <id>` / `shards_graph`, `core/context.py::graph_query`) — the same BFS promoted to a first-class "what's connected to X" query, one traversal rendered as JSON `{seed, nodes, edges}` or a readable tree; **`project`** (`shards project <id>` / `shards_project`, `core/lenses.py::project_view`) — a `type: project` note plus every task whose `project` field points at it, daemon-free, `{project, tasks}` JSON or text; `session-start` (merge `recent_activity(7d, mine)` with my open/claimed tasks **and inbound mentions of me**, dedupe by id, tasks first then newest-first; `--team` widens only the activity half, `--owner` sets the identity the payload is built for) wired to a `SessionStart` hook.

- **Team awareness** — `core/context.py::inbound_ids` inverts `related` at read time (`_inbound_index` batches it in one vault pass), surfaced as `graph --direction in|out|both` on both CLI and `shards_graph`. `task append` adds a comment row without rewriting the body; `task release` returns a claim to `open` and is idempotent (releasing an already-open task is a no-op, releasing someone else's is exit 4 unless `--force`). `task list` carries `--status` (CSV), `--stale` (the inverse of `--since`), `--available`, and `--sort priority`. Stamps name the *acting* agent, not the file's owner. Duplicate titles warn at create (slug-normalized, non-blocking).

- **Wikilink dialect** — `[[Title]]`, `[[Title|alias]]`, `[[Title#Heading]]` and `[[Title^block]]` all resolve to the same target: the alias and anchor are stripped at the lookup boundary, so links a host Markdown app resolves are neither missed in `related` nor miscounted as dangling by `shards status`.

---

## Risks

| Risk | Mitigation |
|---|---|
| `indexed` drift | **Resolved.** NDJSON hit contract pinned with a shared msgspec schema (`index/indexed_client.py`); `search --health` reports `indexed`-reachable vs. substring-fallback distinctly → `index/indexed_client.py`, `tests/search/test_health.py` |
| Windows sockets | Loopback TCP / named pipe |
| Claim race | `O_EXCL`, works daemon-less |
| Stale locks | TTL + dead PID |
| Untrusted content | Sandbox + socket `0600` |
| Python cold-start floor (~150–180ms after the msgspec swap) | **Resolved — no Rust rewrite.** Evaluated and shelved: the ~2–10ms Rust floor is below human-perceptibility for shards's usage (agent tool calls + human CLI, not a hot loop). Runtime stays Python 3.11+, optimized. Only future fallback (not scheduled): a thin Rust client over the existing daemon socket, if a hot-loop use ever emerges → `.spec/lessons.md` |
| Warm index diverging from disk (symlinked vault, duplicate ids, in-place id change, directory move) | **Resolved.** The index is keyed by realpath, `vault_path` is canonicalised at the config boundary, reconcile keeps the caller's path space on a no-move, and directory moves evict/re-index the subtree. Pinned by warm-vs-cold parity tests that mutate the vault *while the daemon is up* — the region the original parity suite never covered → `tests/daemon/test_warm_reads.py` |
| A malformed file killing the watcher thread | **Resolved.** The watchdog adapter is guarded so no exception reaches the observer thread, and every `reconcile.py` guard is pinned by a test that drops a hostile file and asserts the watcher still indexes → `tests/daemon/test_watcher_resilience.py` |
| msgspec `Struct`s reject unknown keys by default | **Resolved.** Invariant 3 ("unknown frontmatter keys round-trip") is load-bearing; the round-trip-fidelity spike that gated the swap **passed** (a `_Frontmatter` stash preserves unknown keys byte-for-byte), so the swap shipped and was not reverted → `.spec/lessons.md` |
