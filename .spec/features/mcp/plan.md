---
type: feature-plan
feature: mcp
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-10
---

# Feature: MCP — Implementation Plan

MCP delivers the Phase-2 agent surface: `brain_*` tools over the same daemon, plus the SessionStart warm-start hook. It is a thin transport — a closed, testable box that adds no new primitives.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Starts when **search** is `DONE` (root [plan.md](../../plan.md) Feature Sequence) — every tool maps to a working CLI command, including search. Does not depend on any other feature's units.

---

## Problem Frame

Agents should drive Brain natively, not by shelling out. This plan wraps the existing CLI/daemon paths as MCP tools with an explicit exposed/withheld policy, then adds the one-shot SessionStart context injection so sessions resume warm.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Expose safe commands as MCP tools](product.md#requirement-expose-safe-commands-as-mcp-tools) | mcp/1 |
| R2 | [Warm-start context injection](product.md#requirement-warm-start-context-injection) | mcp/2 |

Every unit cites the R-IDs it satisfies. Do not renumber R-IDs.

---

## Key Technical Decisions

1. **One client, two transports.** MCP reuses `daemon/client.py`, guaranteeing parity with the CLI.
2. **Explicit allow-list.** Only safe, reversible commands are exposed; destructive/admin ops are withheld by policy.

---

## Unit IDs

Units are `mcp/n` — assigned once, never renumbered. Cite IDs in commits and tests (`feat(mcp): mcp/1 ...`).

---

### mcp/1 — FastMCP server + tool mapping

**Goal:** Expose the `brain_*` note/task/search tools over the same daemon, returning JSON, with destructive/admin ops withheld.

**Requirements:** R1

**Dependencies:** —

**Files:**

```
src/brain/mcp/server.py
```

**Test scenarios:**

- `brain_task_claim` runs the same atomic claim as the CLI and returns JSON.
- `task cancel`/`task delete`/admin commands are absent from the tool list.

**Verification:** `uv run pytest tests/mcp/test_tools.py`

---

### mcp/2 — SessionStart context hook

**Goal:** A Claude Code `SessionStart` hook that injects a token-budgeted top-N search result once at session start.

**Requirements:** R2

**Dependencies:** mcp/1

**Files:**

```
src/brain/mcp/server.py
```

**Test scenarios:**

- The hook command emits compact `--meta-only --json` output suitable for injection.

**Verification:** `uv run pytest tests/mcp/test_session_hook.py`

---

## Progress

| Unit | Status |
|---|---|
| mcp/1 | NOT STARTED |
| mcp/2 | NOT STARTED |
