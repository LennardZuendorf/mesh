---
type: feature-tech
feature: daemon
sibling: product.md
parent: ../../tech.md
updated: 2026-06-21
---

# Daemon — Tech

`asyncio` unix socket (`$XDG_RUNTIME_DIR/brain.sock`, mode `0600`). Writes bypass socket — always `core`/`storage`.

**Links:** [product](product.md) · [plan](plan.md)

## Files

`daemon/server.py` · `daemon/client.py` · `index/watch.py` · `cli/admin.py`

## NDJSON RPC

Request: `{"id":"<uuid>","method":"<name>","params":{...}}`  
OK: `{"id":"…","ok":true,"result":{...}}` · Err: `{"ok":false,"error":{"code":N,"message":"…"}}`

| Method | Fallback (daemon down) |
|---|---|
| `ping` | — |
| `note.get` / `note.list` | file read / `notes/` scan |
| `task.get` / `task.list` | file read / `tasks/` scan |
| `search.query` / `search.tag_pull` | fallback.py / scan |
| `activity.recent` | dir scan |
| `vault.status` | on-demand scan |
| `index.reindex` | no-op + notice |

Watcher: reparse index, reconcile folders (no `updated` bump), call `on_vault_change(path)` — search registers `indexed_client` here.

Hybrid unavailable daemon-down → [search tech](../search/tech.md) degradation matrix.

**Perf:** <50ms scan paths; <200ms hybrid (warm + `indexed`). Windows: loopback TCP token in `~/.brain/run/token`.

<!-- merge -->
Connect-then-fallback; writes never gated by daemon.
<!-- /merge -->
