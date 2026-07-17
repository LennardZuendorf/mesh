---
type: entrypoint
scope: technical
children:
  - plan.md
updated: 2026-07-17
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
| Vault | Tolaria folder + MCP (inherited) |
| Search engine | `indexed` (first-party hybrid; Shards wraps) |

**Added deps only:** typer, python-frontmatter, msgspec, watchdog, FastMCP. Shards code stays small — wrapper, daemon, locks, wikilinks.

---

## Layout

```
src/shards/
├── cli/       # note, task, search, daemon, status, session
├── mcp/       # shards_* tools
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

**Optimization tactics — measured and decided:** stop wrapping hot invocations in `uv run`; swap
`schemas/` pydantic → msgspec (gated on the round-trip-fidelity spike); decompose eager CLI
sub-verb imports (hygiene, not perf); CI startup-time regression guard. Full plan:
[features/cli-toolset-rework/tech.md](features/cli-toolset-rework/tech.md) § Workstream B, §
Decisions.

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
| Folders | `storage/files.py` | `type`/`status` drives path; watcher reconciles mismatch. Notes: `note→notes/`, `log/decision/reference→notes/{logs,decisions,references}/`. Tasks: `open\|claimed→tasks/open/`, `done\|cancelled→tasks/done/`. |
| Atomic write | `storage/files.py` | temp + `os.replace`. `finish`/`cancel` order write-then-move so a crash mid-op never strands the file in an unrecoverable state (a terminal status must not sit in `tasks/open/` where the idempotent no-op refuses to move it). |
| Locks | `storage/locks.py` | `O_EXCL` per entity; stale if PID dead or >300s; `shards status` reports count. Stale reclaim must be race-safe — re-`O_EXCL` rather than unlink-then-create, so a reclaimer never unlinks a fresh lock a peer just took. |
| Sandbox | `storage/sandbox.py` | `realpath` must stay in `tolaria_path` |
| Exit codes | `cli/` | 0 ok · 1 error · 2 validation · 3 not found · 4 claimed · 5 blocked (`--strict` only, Phase 3). Codes live on the domain exception classes; the CLI boundary maps them once. |
| Config | `~/.shards/config.toml` | `[core]` path+agent · `[search]` collection, hybrid, threshold · `[tasks]` collections. Every path value is `expanduser()`'d (`~` resolves). `$SHARDS_AGENT` overrides agent. Missing config → exit 2. Test override: `SHARDS_CONFIG_PATH`. |
| Socket | `daemon/server.py`, `client.py` | NDJSON RPC over a `0600` unix socket; envelope + method table under [Implemented surfaces](#implemented-surfaces) |

### Note fields

`id`, `type` (note|log|decision|reference), `title`, `tags`, `owner`, `created`, `updated`, `related`

### Task adds

`status` (open|claimed|done|cancelled), `priority`, `claimed_by` (`~`), `blocks`, `blocked_by` (inert v1)

**Appends:** finish → `## Outcome` + ISO; cancel → `## Cancelled` + ISO (optional text).

---

## Build order

`notes → tasks → daemon → search → memory` — **all implemented (Phase 1–2).** `tasks-graph` deferred to Phase 3.

Implementation is the source of truth: `src/shards/` + `tests/` (591 tests, ty clean, ruff clean).

---

## Implemented surfaces

Contracts compounded from the (now-deleted) feature specs. Full detail lives in the code + tests cited.

- **Wikilinks** — `core/wikilinks.py`. `[[Title]]` → id by title match; `[[n-id]]`/`[[t-id]]` passthrough; `related` deduped; unresolvable → dangling, counted by `shards status`.
- **RPC** — `daemon/server.py`, `client.py`. NDJSON, one JSON object per line. Request `{id,method,params}` · ok `{id,ok:true,result}` · err `{ok:false,error:{code,message}}`. Connect-then-fallback: every read has a daemon-down fallback, writes bypass the socket entirely. A `503` (reserved-but-unwired method) makes the client run its file-op fallback; a `404` (unknown) propagates. Methods: `ping`, `note.get/list`, `task.get/list`, `activity.recent`, `search.*`, `vault.status`, `index.reindex`.
- **Search** — `index/indexed_client.py`, `fallback.py`, `tagpull.py`. Result `{id,type,title,score,tags?,owner?,updated?,snippet?,path}`; foreign files surface with `id:null`. Hybrid via `indexed` when daemon-up **and** `[search].hybrid`, else the substring fallback scored title-exact 1.0 · title-substring 0.8 · tag 0.6 · body 0.4, sorted score desc then `updated` desc (recency tiebreak within 0.02). `indexed_client.incremental_update` runs on the watcher hook; `shards reindex` → `full_rebuild`.
- **MCP** — `mcp/server.py`, launched via `shards-mcp` (or `python -m shards.mcp.server`). Typed `shards_*` tools mirroring the *safe* verbs, each carrying an annotation: read-only / idempotent / write / destructive (`task_cancel`). Withheld from agents: both delete verbs, `daemon`, `reindex`, `status`, and the Phase-3 `task_release`.
- **Session lenses** — `core/activity.py`, `core/context.py`, `cli/session.py`, `hooks/session_start.json`. `recent-activity` (warm index, or an equivalent folder scan when down); `build-context` (daemon-free BFS over `related` to `--depth`, cycle/diamond-deduped, seed first); `session-start` (merge `recent_activity(7d, mine)` with my open/claimed tasks, dedupe by id, tasks first then newest-first) wired to a `SessionStart` hook.

---

## Risks

| Risk | Mitigation |
|---|---|
| `indexed` drift | Contract pinned in search tech; `search --health` signal tracked as a gap → [features/cli-toolset-rework/tech.md](features/cli-toolset-rework/tech.md) § Gaps |
| Windows sockets | Loopback TCP / named pipe |
| Claim race | `O_EXCL`, works daemon-less |
| Stale locks | TTL + dead PID |
| Untrusted content | Sandbox + socket `0600` |
| Python cold-start floor (~150–180ms after the msgspec swap) | **Resolved — no Rust rewrite.** Evaluated and shelved: the ~2–10ms Rust floor is below human-perceptibility for shards's usage (agent tool calls + human CLI, not a hot loop). Runtime stays Python 3.11+, optimized. Only future fallback (not scheduled): a thin Rust client over the existing daemon socket, if a hot-loop use ever emerges → [features/cli-toolset-rework/tech.md](features/cli-toolset-rework/tech.md) § Decisions |
| msgspec `Struct`s reject unknown keys by default | Invariant 3 ("unknown frontmatter keys round-trip") is load-bearing; a round-trip-fidelity spike gates the swap — if it fails, the swap reverts and Python stays at the pre-swap ~230–250ms floor → [features/cli-toolset-rework/tech.md](features/cli-toolset-rework/tech.md) § Decisions |
