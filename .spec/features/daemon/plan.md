---
type: feature-plan
feature: daemon
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-10
---

# Feature: Daemon — Implementation Plan

Daemon delivers the warm accelerator and the graceful-degradation contract. It is a closed, testable box: with the daemon up, reads are warm; with it down, everything still works. It also provides the index host that the search feature builds ranking on.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Starts when **tasks** is `DONE` (root [plan.md](../../plan.md) Feature Sequence). Reuses the note/task primitives it watches and indexes. Does not depend on any other feature's units.

---

## Problem Frame

Notes and tasks already work over files; the daemon makes them fast and keeps the index live, without becoming a dependency. This plan builds the socket server and fallback shim first (so the degradation contract is provable), then the watcher/incremental index, then admin/status surfacing.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Serve a warm index over a socket](product.md#requirement-serve-a-warm-index-over-a-socket) | daemon/1 |
| R2 | [Keep the index fresh on file changes](product.md#requirement-keep-the-index-fresh-on-file-changes) | daemon/2 |
| R3 | [Degrade gracefully when down](product.md#requirement-degrade-gracefully-when-down) | daemon/1, daemon/3 |

Every unit cites the R-IDs it satisfies. Do not renumber R-IDs.

---

## Key Technical Decisions

1. **Connect-then-fallback in the client.** The same `core`/`storage` code path serves both modes, so degradation is free and provable.
2. **`watchdog` for freshness.** No manual reindex on the hot path; the observer reconciles frontmatter with folder routing.

---

## Unit IDs

Units are `daemon/n` — assigned once, never renumbered. Cite IDs in commits and tests (`feat(daemon): daemon/1 ...`).

---

### daemon/1 — Socket server + fallback shim

**Goal:** `asyncio` unix-socket server with a `0600` socket and a client that falls back to direct file ops.

**Requirements:** R1, R3

**Dependencies:** —

**Files:**

```
src/brain/daemon/server.py
src/brain/daemon/client.py
```

**Test scenarios:**

- A request is served warm when the daemon is up.
- With the daemon stopped, the same client call writes/reads via direct file ops.
- The socket is created `0600` in the per-user runtime dir, owned by the running user.

**Verification:** `uv run pytest tests/daemon/test_socket_fallback.py`

---

### daemon/2 — Watcher + incremental reindex

**Goal:** `watchdog` observer that reparses, re-embeds, and reconciles folder routing on file events.

**Requirements:** R2

**Dependencies:** daemon/1

**Files:**

```
src/brain/index/watch.py
```

**Test scenarios:**

- A hand-edited file updates the index without an explicit reindex.

**Verification:** `uv run pytest tests/daemon/test_watch.py`

---

### daemon/3 — Admin + status

**Goal:** `daemon start|stop|status`, `reindex`, and `status` (counts, freshness, dangling links); stderr degradation notice.

**Requirements:** R3

**Dependencies:** daemon/1

**Files:**

```
src/brain/cli/admin.py
```

**Test scenarios:**

- `brain status` reports counts and index freshness.
- Degraded search prints a one-line stderr notice, suppressed under `--quiet`.

**Verification:** `uv run pytest tests/daemon/test_admin_status.py`

---

## Progress

| Unit | Status |
|---|---|
| daemon/1 | NOT STARTED |
| daemon/2 | NOT STARTED |
| daemon/3 | NOT STARTED |
