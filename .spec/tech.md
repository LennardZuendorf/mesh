---
type: entrypoint
scope: technical
children:
  - features/notes/plan.md
  - features/tasks/plan.md
  - features/daemon/plan.md
  - features/search/plan.md
  - features/memory/plan.md
updated: 2026-06-21
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

---

## Contracts

| Contract | Module | Rule |
|---|---|---|
| Schemas | `schemas/note.py`, `task.py` | Task = note + `type: task`. See field tables below. `updated` bumps on writes; watcher reconcile does not. |
| IDs | `core/ids.py` | `n-`/`t-` + Crockford b32 hash of `created_iso\0title` (4+ chars, extend on collision). Never sequential. |
| Folders | `storage/files.py` | `type`/`status` drives path; watcher reconciles mismatch. Notes: `note→notes/`, `log/decision/reference→notes/{logs,decisions,references}/`. Tasks: `open\|claimed→tasks/open/`, `done\|cancelled→tasks/done/`. |
| Atomic write | `storage/files.py` | temp + `os.replace` |
| Locks | `storage/locks.py` | `O_EXCL` per entity; stale if PID dead or >300s; `brain status` reports count |
| Sandbox | `storage/sandbox.py` | `realpath` must stay in `tolaria_path` |
| Exit codes | `cli/` | 0 ok · 1 error · 2 validation · 3 not found · 4 claimed · 5 blocked (`--strict` only, Phase 3) |
| Config | `~/.brain/config.toml` | `[core]` path+agent · `[search]` collection, hybrid, threshold · `[tasks]` collections. `$BRAIN_AGENT` overrides agent. Missing config → exit 2. Test override: `BRAIN_CONFIG_PATH`. |
| Socket | `daemon/*.py` | NDJSON RPC; envelope in [daemon/tech.md](features/daemon/tech.md) |

### Note fields

`id`, `type` (note|log|decision|reference), `title`, `tags`, `owner`, `created`, `updated`, `related`

### Task adds

`status` (open|claimed|done|cancelled), `priority`, `claimed_by` (`~`), `blocks`, `blocked_by` (inert v1)

**Appends:** finish → `## Outcome` + ISO; cancel → `## Cancelled` + ISO (optional text).

---

## Build order

`notes → tasks → daemon → search → memory → tasks-graph (Phase 3)`

Feature detail: `features/<name>/tech.md`.

---

## Risks

| Risk | Mitigation |
|---|---|
| `indexed` drift | Contract pinned in search tech |
| Windows sockets | Loopback TCP / named pipe |
| Claim race | `O_EXCL`, works daemon-less |
| Stale locks | TTL + dead PID |
| Untrusted content | Sandbox + socket `0600` |
