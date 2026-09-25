---
type: feature-plan
feature: dashboard-tui
sibling: tech.md
parent: ../../plan.md
updated: 2026-09-25
---

# Feature: Dashboard TUI — Implementation Plan

Composition first (a pure, headless-testable frame model), then the terminal shell around it,
then the budget pin that proves it taxed nobody. The render layer stays a trivial mapping so
everything meaningful is testable without a tty.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)
**Design:** [design.md](design.md)

**Feature gate:** none — independent of [note-adoption](../note-adoption/plan.md) and
[project-envelope](../project-envelope/plan.md); may run in parallel. Its agents pane is richer
once note-adoption lands, but it reads whatever the census reports.

---

## Problem Frame

Everything the dashboard shows already exists as a domain read; the engineering problem is the
terminal lifecycle (raw mode must never leak into the operator's shell) and the discipline that
two new crates must not tax every other command's startup — which finally gets pinned by the
test the trace found missing.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [One screen, four panes](product.md#requirement-one-screen-four-panes) | dashboard-tui/1, dashboard-tui/2 |
| R2 | [Live refresh that never lies](product.md#requirement-live-refresh-that-never-lies) | dashboard-tui/1, dashboard-tui/2 |
| R3 | [Keys are minimal and boring](product.md#requirement-keys-are-minimal-and-boring) | dashboard-tui/2 |
| R4 | [Human-only, and harmless to the instant CLI](product.md#requirement-human-only-and-harmless-to-the-instant-cli) | dashboard-tui/3 |
| R5 | [The terminal survives the dashboard](product.md#requirement-the-terminal-survives-the-dashboard) | dashboard-tui/2 |

---

### dashboard-tui/1 — Snapshot composition

**Goal:** `snapshot()` — the pure frame model over existing domain reads, unit-tested headless.

**Requirements:** R1, R2

**Dependencies:** —

**Files:** `src/cli/dashboard.rs` (module created here with `snapshot` only),
`Cargo.toml` (ratatui + crossterm added, imported nowhere yet)

**Test scenarios:** counts per pane from a fixture vault; mine filter narrows agents/tasks/note
panes; empty vault → calm empty states; one unreadable file → fail-soft, rest of frame good;
every number traceable to status/list/activity calls (no second source)

**Verification:** `cargo test --lib` (snapshot units) green; new fixture vault renders expected
frame model.

---

### dashboard-tui/2 — Terminal shell, loop, keys

**Goal:** `mesh dashboard [--interval]`: tty guard, alternate-screen lifecycle with Drop-guard
restore, event loop with tick deadline, `q`/`r`/`m`/`Tab`/arrows, fixed four-pane layout per
[design.md](design.md).

**Requirements:** R1, R2, R3, R5

**Dependencies:** dashboard-tui/1

**Files:** `src/cli/dashboard.rs`, `src/cli/mod.rs`, `src/main.rs`

**Test scenarios:** non-tty → exit 2 `dashboard needs a terminal`; `--help` renders; a scripted
quit path restores cooked mode (assert termios state after); failed refresh keeps last frame +
status line; `m` toggle visible in chrome; `r` forces immediate tick

**Verification:** `cargo test --test dashboard_cli`; interactive smoke against the brain vault
(manual).

---

### dashboard-tui/3 — Budget pin and docs

**Goal:** The missing cold-start wall-clock test finally exists; the admin surface docs
(README/AGENTS structure notes) mention the new verb; no MCP exposure asserted.

**Requirements:** R4

**Dependencies:** dashboard-tui/2

**Files:** `tests/foundation_cli.rs`, `README.md`, `AGENTS.md` (structure list only)

**Test scenarios:** warm read command starts under the pinned bound with ratatui linked;
`mesh dashboard` absent from the MCP tool table (count pin already covers it — assert the
dashboard is not a tool)

**Verification:** `cargo test --test foundation_cli --test mcp_cli`; gates
(`fmt`/`clippy -D warnings`/full suite) green.

---

## Progress

| Unit | Status |
|---|---|
| dashboard-tui/1 | NOT STARTED |
| dashboard-tui/2 | NOT STARTED |
| dashboard-tui/3 | NOT STARTED |
