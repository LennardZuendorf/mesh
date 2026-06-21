---
type: feature-plan
feature: memory
sibling: tech.md
parent: ../../plan.md
updated: 2026-06-21
---

# Feature: Memory — Implementation Plan

Memory delivers the Phase-2 agent surface: `brain_*` tools over the same daemon, the memory-lens read commands (`recent-activity`, `build-context`, `session-start`), and the SessionStart warm-start hook. It adds no new store — it is a lens over notes + search + daemon.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Starts when **search** is `DONE` (root [plan.md](../../plan.md) Feature Sequence) — every write tool maps to a working CLI command and the read shapes need the warm index. Does not depend on any other feature's units.

---

## Problem Frame

Agents should drive Brain natively and resume warm. This plan wraps the existing CLI/daemon paths as annotated MCP tools, adds two memory-lens read commands over the index, composes the session-start preamble, then ships the hook config.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Expose the verbs as annotated memory tools](product.md#requirement-expose-the-verbs-as-annotated-memory-tools) | memory/1 |
| R2 | [Recent-activity feed](product.md#requirement-recent-activity-feed) | memory/2 |
| R3 | [Context assembly over wikilinks](product.md#requirement-context-assembly-over-wikilinks) | memory/3 |
| R4 | [Warm-start context injection](product.md#requirement-warm-start-context-injection) | memory/4 |

Every unit cites the R-IDs it satisfies. Do not renumber R-IDs.

---

## Key Technical Decisions

1. **One client, two transports.** MCP reuses `daemon/client.py`, guaranteeing parity with the CLI.
2. **Memory is a lens, not a store.** `recent_activity`/`build_context` read the existing index/files; no new persistence.
3. **Annotated allow-list.** Tools carry behaviour annotations; hard-delete and admin ops are withheld.
4. **Two-source warm-start.** `session-start` merges recent activity with all open/claimed `--mine` tasks.

---

## Unit IDs

Units are `memory/n` — assigned once, never renumbered. Cite IDs in commits and tests (`feat(memory): memory/1 ...`).

---

### memory/1 — FastMCP server + annotated tool mapping

**Goal:** Expose the `brain_*` note/task/search tools over the same daemon, returning JSON, each carrying a `read-only`/`idempotent`/`write`/`destructive` annotation, with hard-delete/admin withheld and `task cancel` exposed.

**Requirements:** R1

**Dependencies:** —

**Files:** `src/brain/mcp/server.py`

**Test scenarios:**

- `brain_task_claim` runs the same atomic claim as the CLI and returns JSON.
- Every exposed tool carries an annotation; `brain_task_cancel` is present (`destructive`); `note delete`/`task delete`/admin are absent.

**Verification:** `uv run pytest tests/memory/test_tools.py`

---

### memory/2 — `recent-activity`

**Goal:** `recent-activity` CLI command + `brain_recent_activity` tool returning time-ordered changed notes/tasks within a window, with `--mine` and a no-daemon directory-scan fallback.

**Requirements:** R2

**Dependencies:** memory/1

**Files:** `src/brain/core/activity.py`, `src/brain/cli/session.py`, `src/brain/mcp/server.py`

**Test scenarios:**

- `recent-activity --since 24h` returns items with `updated` in window, newest-first.
- `--mine` restricts tasks to the caller's owner/claimed_by.
- Daemon-down falls back to directory scan.

**Verification:** `uv run pytest tests/memory/test_recent_activity.py`

---

### memory/3 — `build-context`

**Goal:** `build-context` CLI command + `brain_build_context` tool that follows a seed's `related` wikilinks breadth-first to `--depth`, deduplicated, returning compact JSON.

**Requirements:** R3

**Dependencies:** memory/1

**Files:** `src/brain/core/context.py`, `src/brain/cli/session.py`, `src/brain/mcp/server.py`

**Test scenarios:**

- `build-context n-a3f2 --depth 1` returns the seed plus directly-linked notes/tasks.
- Cycles and duplicate links are deduplicated; resolution runs with no daemon.

**Verification:** `uv run pytest tests/memory/test_build_context.py`

---

### memory/4 — `session-start` + SessionStart hook

**Goal:** `brain session-start` composes recent activity + open/claimed `--mine` tasks; ship `hooks/session_start.json` and setup docs.

**Requirements:** R4

**Dependencies:** memory/2

**Files:** `src/brain/cli/session.py`, `hooks/session_start.json`, `docs/memory-setup.md`

**Test scenarios:**

- `session-start --meta-only --json` includes recent items and open/claimed tasks (deduplicated).
- A task claimed 30+ days ago with no recent edits still appears via the task-list leg.
- Hook config points at `brain session-start --meta-only --json` (SessionStart, not PreToolUse).

**Verification:** `uv run pytest tests/memory/test_session_hook.py`

---

## DONE

All four units pass; MCP tools mirror CLI behaviour with annotations; `session-start` composes both warm-start feeds; hook config is valid.

## Progress

| Unit | Status |
|---|---|
| memory/1 | NOT STARTED |
| memory/2 | NOT STARTED |
| memory/3 | NOT STARTED |
| memory/4 | NOT STARTED |
