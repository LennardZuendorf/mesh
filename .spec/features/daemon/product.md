---
type: feature-product
feature: daemon
sibling: tech.md
parent: ../../product.md
updated: 2026-06-21
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
| **Owns** | The local daemon (unix-socket server), the `watchdog` observer, incremental warm-index reconcile, the daemon-down fallback shim, admin commands (`daemon start\|stop\|status`, `reindex`, `status`), and the `on_vault_change` hook registration point |
| **Does not own** | Domain logic (notes/tasks features), ranking algorithm and `indexed_client` implementation (search feature); it hosts the warm index but does not define how results are scored |

---

## Requirements

### Requirement: Serve a warm index over a socket

The system SHALL run one local daemon that holds the parsed frontmatter index and the wikilink resolution graph, exposes a newline-delimited JSON RPC over a unix domain socket, and registers callbacks for vault change events (used by the search feature to drive `indexed index update`).

#### Scenario: Warm search

- **Given** the daemon is running and `indexed` is available
- **When** a client issues `brain search "ndc"`
- **Then** hybrid results are returned using the warm frontmatter index for enrichment without a full vault rescan

#### Scenario: List works without daemon

- **Given** the daemon is stopped
- **When** a client runs `brain note list` or `brain task list`
- **Then** results come from a direct directory scan (correct but slower) — list never requires the daemon

### Requirement: Keep the index fresh on file changes

The system SHALL watch `tolaria_path` and MUST reflect creates, modifies, and deletes — by humans, Tolaria, or other agents — without an explicit reindex, reconciling frontmatter with folder location.

#### Scenario: Hand-edited note is picked up

- **Given** the daemon is running
- **When** a user hand-edits a `.md` file in the vault
- **Then** the index reflects the change and `brain status` shows updated freshness

### Requirement: Degrade gracefully when down

The system SHALL fall back to direct file operations and substring-scan search when the daemon is unreachable, MUST keep all write paths working identically, and MUST print a one-line stderr notice on degraded search (suppressed under `--quiet`).

#### Scenario: Write with no daemon

- **Given** the daemon is stopped
- **When** a user runs `brain note new ...`
- **Then** the file is written directly and is indexed when the daemon next starts

### Requirement: Admin and vault health commands

The system SHALL provide `daemon start|stop|status`, `brain reindex` (delegates to search's `indexed_client` when present), and `brain status` reporting note/task counts, index freshness, dangling links, and stale lock count.

#### Scenario: Vault health report

- **Given** a populated vault
- **When** `brain status --json` runs
- **Then** counts, last watch timestamp, pending `indexed` update queue depth, dangling link count, and stale lock count are returned

Reference requirements as R1–R4 in the feature plan's Requirements Trace.

## User Experience

```
$ brain daemon start
$ brain status          # counts, index freshness, dangling links, stale locks
$ brain daemon stop
$ brain search "ndc"    # daemon down: substring-scan + stderr notice
```

## Non-Goals

- No system service / init integration — the daemon is user-launched.
- No multi-host or networked daemon — one local process per user.

## Prior Art & Inspiration

**Anchor — the Language Server (LSP) pattern:** a warm server holds expensive state (parsed index, graph) while thin clients connect over a socket and the editor still works if the server dies.

- **Borrow:** accelerator-not-gatekeeper; warm in-memory state that is expensive to rebuild per call; thin clients (CLI and MCP) over one local socket; kill it anytime.
- **Differ:** brain's daemon never becomes a system of record — contrast [GBrain](https://github.com/garrytan/gbrain), whose `serve` **syncs Markdown into Postgres/PGLite**. brain's warm state is ephemeral and disposable; the files on disk remain the only truth, and every write path runs identically with the daemon down.
