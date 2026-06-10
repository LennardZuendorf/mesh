---
type: feature-product
feature: daemon
sibling: tech.md
parent: ../../product.md
updated: 2026-06-10
---

# Feature: Daemon — Product

The daemon is the warm accelerator that makes Brain feel instant. It watches the Tolaria folder, keeps the index fresh, and serves both the CLI and the MCP server from one process — but it is never required: every command still works against files directly when the daemon is down. This feature owns the watcher, the index lifecycle, the socket transport, and the graceful-degradation path.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The local daemon (unix-socket server), the `watchdog` observer, incremental reindex, the daemon-down fallback shim, and the admin commands `daemon start\|stop\|status`, `reindex`, `status` |
| **Does not own** | Domain logic (notes/tasks features) and the ranking algorithm (search feature); it hosts the index but does not define how results are scored |

---

## Requirements

### Requirement: Serve a warm index over a socket

The system SHALL run one local daemon that holds the parsed frontmatter index, the resolution graph, and the search indices, serving CLI and MCP clients over a unix domain socket.

#### Scenario: Warm read

- **Given** the daemon is running
- **When** a client issues a search or list
- **Then** it is answered from warm in-memory state without rebuilding the index

### Requirement: Keep the index fresh on file changes

The system SHALL watch `tolaria_path` and MUST reflect creates, modifies, and deletes — by humans, Tolaria, or other agents — without an explicit reindex, reconciling frontmatter with folder location.

#### Scenario: Hand-edited note is picked up

- **Given** the daemon is running
- **When** a user hand-edits a `.md` file in the vault
- **Then** the index reflects the change and `brain status` shows updated freshness

### Requirement: Degrade gracefully when down

The system SHALL fall back to direct file operations and lexical-only search when the daemon is unreachable, MUST keep all write paths working identically, and MUST print a one-line stderr notice on degraded search (suppressed under `--quiet`).

#### Scenario: Write with no daemon

- **Given** the daemon is stopped
- **When** a user runs `brain note new ...`
- **Then** the file is written directly and is indexed when the daemon next starts

Reference requirements as R1, R2, R3 in the feature plan's Requirements Trace.

## User Experience

```
$ brain daemon start
$ brain status          # counts, index freshness, dangling links
$ brain daemon stop
$ brain search "ndc"    # still works: lexical-only + stderr notice
```

## Non-Goals

- No system service / init integration — the daemon is user-launched.
- No multi-host or networked daemon — one local process per user.
