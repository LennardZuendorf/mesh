---
type: feature-tech
feature: daemon
sibling: product.md
parent: ../../tech.md
updated: 2026-06-21
---

# Feature: Daemon — Architecture

The daemon is an `asyncio` unix-socket server that owns warm state and a `watchdog` observer. CLI and MCP are thin clients; on startup each tries to connect to the socket and, on failure, transparently falls back to direct file operations plus the search feature's substring scan. The daemon accelerates and coordinates but never gates: write primitives live in `core`/`storage`, so the fallback path is the same code.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/brain/daemon/server.py   # asyncio unix-socket server, request dispatch, lifecycle
src/brain/daemon/client.py   # socket client + daemon-down fallback shim
src/brain/index/watch.py     # watchdog observer -> warm index reconcile + on_vault_change hook
src/brain/cli/admin.py       # daemon start|stop|status, reindex, status
```

---

## Contract / API

### Transport

Unix domain socket at `$XDG_RUNTIME_DIR/brain.sock` (fallback: `~/.brain/run/brain.sock`), created `0600`, owned by the running user. Requests/responses are **newline-delimited JSON** (one object per line). (Windows: loopback TCP or named pipe fallback — see open questions.)

### Request / response envelope

```json
{"id": "550e8400-e29b-41d4-a716-446655440000", "method": "note.list", "params": {"tags": ["ndc"], "limit": 20}}
```

Success:

```json
{"id": "550e8400-e29b-41d4-a716-446655440000", "ok": true, "result": {"items": [], "degraded": false}}
```

Error:

```json
{"id": "550e8400-e29b-41d4-a716-446655440000", "ok": false, "error": {"code": 3, "message": "not found"}}
```

`error.code` mirrors CLI exit codes (`0`–`5`) where applicable; transport failures use `code: 1`.

### RPC methods (v1)

| Method | Purpose | Fallback when daemon down |
|---|---|---|
| `ping` | Liveness + protocol version | N/A (client uses direct path) |
| `note.get` | Warm get by id/slug | Direct file read + frontmatter parse |
| `note.list` | Warm list with filters | Directory scan of `notes/` |
| `task.get` | Warm get by id | Direct file read |
| `task.list` | Warm list with filters | Directory scan of `tasks/` |
| `search.query` | Hybrid search (delegates to search feature) | Substring scan via `fallback.py` |
| `search.tag_pull` | Deterministic tag pull | Frontmatter scan |
| `activity.recent` | `recent_activity` shape | Directory scan |
| `vault.status` | Counts, freshness, dangling links, stale locks | On-demand scan (slower) |
| `index.reindex` | Trigger full rebuild | No-op with stderr notice; runs on next daemon start |

Write operations (`note new`, `task claim`, etc.) **do not** use the socket — they always run through `core`/`storage` directly (daemon may observe the resulting file event).

### Warm state held

Parsed frontmatter index (powers warm `list`/`get`/tag-pull/`recent_activity`) and the wikilink/ID resolution graph (powers `related`/`build_context`). Ranking state (lexical + vectors) lives in `indexed`, not the daemon.

### Admin commands

- `daemon start|stop|status` — process lifecycle (`daemon status` reports socket path and PID).
- `reindex` — calls `indexed_client.full_rebuild()` when the search feature is present; otherwise queues for next start.
- `brain status` — vault health via `vault.status` RPC or direct scan: note/task counts, index freshness (last watch event, pending `indexed` updates), dangling links, stale lock count. **Not** a team dashboard — use `task list --mine`.

### Watcher reconcile

On create/modify/delete under `tolaria_path`: re-parse frontmatter into the warm index, refresh the wikilink graph, reconcile folder location against `type`/`status` (move file if frontmatter and folder disagree; does **not** bump `updated`), and invoke registered `on_vault_change(path)` callbacks (the search feature's `indexed_client.incremental_update` registers here).

## Implementation Detail

- **Fallback shim.** `daemon/client.py` attempts the socket connect; on failure (down / stale socket) it routes warm-read calls to the same `core`/`storage` scan paths the daemon would rebuild. Hybrid search is unavailable when the daemon is down — `search` uses substring scan per the degradation matrix in [search tech](../search/tech.md).
- **Freshness hook.** The watcher owns *when* to notify; the **search** feature's `indexed_client` owns *how* to call `indexed index update` / `indexed index create`. Daemon does not import ranking logic.

<!-- merge -->
Graceful degradation is a project-wide invariant, not a daemon feature detail: availability never depends on the daemon. The connect-then-fallback contract and the rule that all write primitives live outside the daemon belong in root tech.md.
<!-- /merge -->

## Performance Budget

- Search latency: **< 50 ms** substring/tag-pull, **< 200 ms** hybrid against a warm daemon + `indexed` for a few-thousand-doc vault.
- CLI startup must feel instant — heavy state lives in the daemon, not at process start. `note get`/`task get` are single-file reads.

## Open Questions

1. **Windows transport.** Unix domain sockets aren't native on older Windows. *Default:* loopback TCP (127.0.0.1, token in `~/.brain/run/token`) or named pipe when a unix socket is unavailable.
