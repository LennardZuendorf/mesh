---
type: feature-product
feature: memory
sibling: tech.md
parent: ../../product.md
updated: 2026-06-21
---

# Memory — Product

Phase 2 agent surface: MCP `brain_*` tools + memory lenses over existing index. **Not a store.**

**Links:** [product](../../product.md) · [tech](tech.md) · [plan](plan.md)

## Scope

| | |
|---|---|
| **Owns** | FastMCP, tool annotations, memory lenses, `session-start` hook |
| **Does not own** | core writes, daemon internals, `indexed` |

## Requirements

### R1: MCP tools
SHALL mirror safe CLI commands as annotated `brain_*` tools (JSON). Withhold delete + admin.

### R2: Recent activity
SHALL time-ordered changes in window; daemon-down → dir scan.

### R3: Build context
SHALL BFS over `related` from seed ID to `--depth` (default 1).

### R4: Warm start
SHALL `session-start` compose recent (`--since 7d --mine`) + all open/claimed `--mine` tasks; hook runs once at SessionStart.

## UX

```json
{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"brain session-start --meta-only --json"}]}]}}
```

**Not:** MCP-only writes, second memory store.
