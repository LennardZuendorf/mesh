---
type: feature-tech
feature: daemon
sibling: product.md
parent: ../../tech.md
updated: 2026-06-10
---

# Feature: Daemon — Architecture

The daemon is an `asyncio` unix-socket server that owns warm state and a `watchdog` observer. CLI and MCP are thin clients; on startup each tries to connect to the socket and, on failure, transparently falls back to direct file operations plus in-process BM25. The daemon accelerates and coordinates but never gates: write primitives live in `core`/`storage`, so the fallback path is the same code.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/brain/daemon/server.py   # asyncio unix-socket server, request dispatch, lifecycle
src/brain/daemon/client.py   # socket client + daemon-down fallback shim
src/brain/index/watch.py     # watchdog observer -> incremental reindex
src/brain/cli/admin.py       # daemon start|stop|status, reindex, status
```

---

## Contract / API

- **Transport.** Unix domain socket in a per-user runtime dir, created `0600`, owned by the running user, so other local users cannot drive it. Requests/responses are JSON framed over the socket. (Windows: loopback TCP or named pipe fallback — see open questions.)
- **Warm state held:** parsed frontmatter index, wikilink/ID resolution graph, BM25 term index, embedding cache keyed off the embedder.
- **Admin commands:** `daemon start|stop|status`; `reindex` (full rebuild from the folder); `status` (counts: notes, tasks by status, index freshness — last event and pending re-embeds — and dangling links).

## Implementation Detail

- **Fallback shim.** `daemon/client.py` attempts the socket connect; on failure (down / stale socket) it routes calls to the same `core`/`storage` functions the daemon would, with a BM25/substring search pass. Embeddings are unavailable in this mode, so `search` returns lexical results and prints a one-line stderr notice unless `--quiet`.
- **Watcher.** A `watchdog` observer on `tolaria_path` triggers incremental reindex on create/modify/delete: re-parse frontmatter, update BM25 terms, re-embed the changed document, and reconcile folder location against `type`/`status`.
- **Freshness.** `brain status` reports the last watch event and any pending re-embeds.

<!-- merge -->
Graceful degradation is a project-wide invariant, not a daemon feature detail: availability never depends on the daemon. The connect-then-fallback contract and the rule that all write primitives live outside the daemon belong in root tech.md.
<!-- /merge -->

## Performance Budget

- Search latency: **< 50 ms** lexical-only, **< 200 ms** hybrid against a warm daemon for a few-thousand-doc vault.
- CLI startup must feel instant — heavy state lives in the daemon, not at process start. `note get`/`task get` are single-file reads.

## Open Questions

1. **Windows transport.** Unix domain sockets aren't native on older Windows. *Default:* loopback TCP (127.0.0.1, token-gated) or named pipe when a unix socket is unavailable.
