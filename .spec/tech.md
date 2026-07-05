---
type: entrypoint
scope: technical
children:
  - plan.md
updated: 2026-07-05
---

# Brain — Technical Architecture

Daemon-centric, never daemon-dependent. Warm index + watcher in the daemon; writes in `core`/`storage`; ranking in `indexed`. CLI and MCP are thin socket clients with file-op fallback.

---

## Stack

| Layer | Choice |
|---|---|
| Runtime | Python 3.11+, `uv` / `pipx` |
| CLI | `typer` |
| Data | Markdown + `python-frontmatter`, `pydantic` v2 |
| Watcher | `watchdog` |
| Agents | `FastMCP` |
| Vault | Tolaria folder + MCP (inherited) |
| Search engine | `indexed` (first-party hybrid; Brain wraps) |

**Added deps only:** typer, python-frontmatter, pydantic, watchdog, FastMCP. Brain code stays small — wrapper, daemon, locks, wikilinks.

---

## Layout

```
src/brain/
├── cli/       # note, task, search, daemon, status, session
├── mcp/       # brain_* tools
├── daemon/    # socket server, client shim
├── core/      # notes, tasks, ids, wikilinks, activity, context
├── index/     # indexed_client, tagpull, fallback, watch
├── schemas/   # note, task, config
└── storage/   # atomic files, locks, sandbox
```

---

## Invariants

1. Daemon accelerates; never gates writes.
2. Writes: temp-file + `os.replace`; idempotent where stated.
3. Clean Markdown; unknown frontmatter keys round-trip.
4. One folder, one search path; rank in `indexed`.
5. Agent content is inert data.
6. Instant CLI: daemon-only deps (`watchdog`) import lazily inside the daemon path, never at CLI import time.
7. Read-lenses serve from the warm index when the daemon is up; a disk re-parse happens only on the daemon-down fallback.

---

## Shared primitives (DRY)

A task is a note with `type: task`, so every cross-cutting mechanic has **one** implementation the two verbs share — never per-verb copies (copies drift; the spec's whole thesis is a small, single-surface core):

- **Lock-hold** — the bounded wait-retry over the `O_EXCL` lock lives in `storage/locks.py`; `core/notes.py` and `core/tasks.py` import it.
- **Safe read** — read-text + `frontmatter.loads` guarding **both** `OSError` and `yaml.YAMLError` (foreign/corrupt files skip silently) is one reader (`storage/files.read_post`); every scanner (`tagpull`, `activity`, `wikilinks`, `scan_recent`) routes through it.
- **Vault walk** — one iterator over `notes/**` + `tasks/{open,done}/`; all scanners consume it.
- **CLI output** — one `cli/_output.py` surface (`emit_mutation`, `preview`, `is_json`/`is_quiet`/`is_machine`, delete-guard); `note` and `task` render through it. The interactive-tty check stays a per-module seam so the delete tests can fake it.
- **Frontmatter is serialized from the pydantic model** (`model_dump`), never a parallel hand-built dict — the schema is the single on-disk contract.
- **Terminalize** — `finish`/`cancel` are thin wrappers over one `_terminate_task(*, heading, status, text)`.
- **Exit codes** are a fixed convention — `2` validation/ambiguous, `3` not-found, `4` claim-conflict — that each CLI handler maps its domain exceptions to at the boundary.

---

## Contracts

| Contract | Module | Rule |
|---|---|---|
| Schemas | `schemas/note.py`, `task.py` | Task = note + `type: task`. See field tables below. `updated` bumps on writes; watcher reconcile does not. |
| IDs | `core/ids.py` | `n-`/`t-` + Crockford b32 hash of `created_iso\0title` (4+ chars, extend on collision). Never sequential. |
| Folders | `storage/files.py` | `type`/`status` drives path; watcher reconciles mismatch. Notes: `note→notes/`, `log/decision/reference→notes/{logs,decisions,references}/`. Tasks: `open\|claimed→tasks/open/`, `done\|cancelled→tasks/done/`. |
| Atomic write | `storage/files.py` | temp + `os.replace`. `finish`/`cancel` order write-then-move so a crash mid-op never strands the file in an unrecoverable state (a terminal status must not sit in `tasks/open/` where the idempotent no-op refuses to move it). |
| Locks | `storage/locks.py` | `O_EXCL` per entity; stale if PID dead or >300s; `brain status` reports count. Stale reclaim must be race-safe — re-`O_EXCL` rather than unlink-then-create, so a reclaimer never unlinks a fresh lock a peer just took. |
| Sandbox | `storage/sandbox.py` | `realpath` must stay in `tolaria_path` |
| Exit codes | `cli/` | 0 ok · 1 error · 2 validation · 3 not found · 4 claimed · 5 blocked (`--strict` only, Phase 3). Codes live on the domain exception classes; the CLI boundary maps them once. |
| Config | `~/.brain/config.toml` | `[core]` path+agent · `[search]` collection, hybrid, threshold · `[tasks]` collections. Every path value is `expanduser()`'d (`~` resolves). `$BRAIN_AGENT` overrides agent. Missing config → exit 2. Test override: `BRAIN_CONFIG_PATH`. |
| Socket | `daemon/server.py`, `client.py` | NDJSON RPC over a `0600` unix socket; envelope + method table under [Implemented surfaces](#implemented-surfaces) |

### Note fields

`id`, `type` (note|log|decision|reference), `title`, `tags`, `owner`, `created`, `updated`, `related`

### Task adds

`status` (open|claimed|done|cancelled), `priority`, `claimed_by` (`~`), `blocks`, `blocked_by` (inert v1)

**Appends:** finish → `## Outcome` + ISO; cancel → `## Cancelled` + ISO (optional text).

---

## Build order

`notes → tasks → daemon → search → memory` — **all implemented (Phase 1–2).** `tasks-graph` deferred to Phase 3.

Implementation is the source of truth: `src/brain/` + `tests/` (578 tests, mypy strict, ruff clean).

---

## Implemented surfaces

Contracts compounded from the (now-deleted) feature specs. Full detail lives in the code + tests cited.

- **Wikilinks** — `core/wikilinks.py`. `[[Title]]` → id by title match; `[[n-id]]`/`[[t-id]]` passthrough; `related` deduped; unresolvable → dangling, counted by `brain status`.
- **RPC** — `daemon/server.py`, `client.py`. NDJSON, one JSON object per line. Request `{id,method,params}` · ok `{id,ok:true,result}` · err `{ok:false,error:{code,message}}`. Connect-then-fallback: every read has a daemon-down fallback, writes bypass the socket entirely. A `503` (reserved-but-unwired method) makes the client run its file-op fallback; a `404` (unknown) propagates. Methods: `ping`, `note.get/list`, `task.get/list`, `activity.recent`, `search.*`, `vault.status`, `index.reindex`.
- **Search** — `index/indexed_client.py`, `fallback.py`, `tagpull.py`. Result `{id,type,title,score,tags?,owner?,updated?,snippet?,path}`; foreign files surface with `id:null`. Hybrid via `indexed` when daemon-up **and** `[search].hybrid`, else the substring fallback scored title-exact 1.0 · title-substring 0.8 · tag 0.6 · body 0.4, sorted score desc then `updated` desc (recency tiebreak within 0.02). `indexed_client.incremental_update` runs on the watcher hook; `brain reindex` → `full_rebuild`.
- **MCP** — `mcp/server.py`, launched via `brain-mcp` (or `python -m brain.mcp.server`). Typed `brain_*` tools mirroring the *safe* verbs, each carrying an annotation: read-only / idempotent / write / destructive (`task_cancel`). Withheld from agents: both delete verbs, `daemon`, `reindex`, `status`, and the Phase-3 `task_release`.
- **Session lenses** — `core/activity.py`, `core/context.py`, `cli/session.py`, `hooks/session_start.json`. `recent-activity` (warm index, or an equivalent folder scan when down); `build-context` (daemon-free BFS over `related` to `--depth`, cycle/diamond-deduped, seed first); `session-start` (merge `recent_activity(7d, mine)` with my open/claimed tasks, dedupe by id, tasks first then newest-first) wired to a `SessionStart` hook.

---

## Risks

| Risk | Mitigation |
|---|---|
| `indexed` drift | Contract pinned in search tech |
| Windows sockets | Loopback TCP / named pipe |
| Claim race | `O_EXCL`, works daemon-less |
| Stale locks | TTL + dead PID |
| Untrusted content | Sandbox + socket `0600` |
