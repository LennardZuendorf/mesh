---
type: feature-product
feature: daemon
sibling: tech.md
parent: ../../product.md
updated: 2026-06-21
---

# Daemon — Product

Warm accelerator: watcher, frontmatter index, NDJSON socket. Never required — all commands work daemon-down.

**Links:** [product](../../product.md) · [tech](tech.md) · [plan](plan.md)

## Scope

| | |
|---|---|
| **Owns** | socket server, watcher, fallback shim, admin, `on_vault_change` hook |
| **Does not own** | domain logic, `indexed_client` (search) |

## Requirements

### R1: Warm reads
SHALL hold frontmatter + wikilink index; NDJSON RPC. List/get work via dir scan when down.

### R2: Freshness
SHALL watch vault; reconcile folder vs frontmatter; fire change hooks.

### R3: Degrade
SHALL fallback to file ops + substring search; one stderr notice on degraded search.

### R4: Admin
SHALL `daemon start|stop|status`, `brain status` (counts, freshness, dangling links, stale locks), `reindex` (delegates to search when present).

## UX

```bash
brain daemon start
brain status --json
brain search "ndc"   # substring + notice if daemon down
```

**Not:** systemd service, multi-host daemon.
