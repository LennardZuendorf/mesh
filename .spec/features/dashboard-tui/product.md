---
type: feature-product
feature: dashboard-tui
sibling: tech.md
parent: ../../product.md
updated: 2026-09-25
---

# Feature: Dashboard TUI — Product

`mesh dashboard`: a live, read-only terminal dashboard over the vault — the agents and their
load, the task board, recent activity, and vault health, rendered in one screen and refreshed on
a tick. It runs in the foreground like `mesh watch`, every refresh reads the folder directly,
and it is for the human operator: not exposed over MCP, no mutations from inside the UI.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Design:** [design.md](design.md)

---

## Scope

| | |
|---|---|
| **Owns** | The `dashboard` admin verb; its rendering, key handling and refresh loop; the dashboard-specific composition of existing read surfaces (status, task lists, activity, health) |
| **Does not own** | Any mutation path (claims, writes, deletes — never, from inside the dashboard); the MCP surface (nothing added); the read surfaces it composes ([note-adoption](../note-adoption/product.md) and existing lenses feed it, they are not changed by it) |

---

## Requirements

### Requirement: One screen, four panes

The dashboard MUST show, in one screen: an **agents** pane (per identity: open / claimed / stale
task counts and owned / claimed note counts), a **tasks** pane (ready, blocked and claimed tasks
with ids and titles), a **recent activity** pane, and a **vault health** pane (dangling links,
stale locks, watcher and index status, foreign/adopted note counts).

#### Scenario: The vault at a glance

- **Given** a vault with two agents, one claimed task, one blocked task and a dangling link
- **When** the operator opens the dashboard
- **Then** all of it is visible on one screen without scrolling the terminal

#### Scenario: Empty vault

- **Given** a fresh vault with nothing in it
- **When** the dashboard opens
- **Then** every pane shows a calm empty state, never an error

### Requirement: Live refresh that never lies

The dashboard MUST poll on a tick (`--interval`, default ~2 seconds) and MUST behave identically
whether or not `mesh watch` or `indexed` are running — every refresh is a direct read of the
folder. `r` MUST trigger an immediate refresh. If a refresh fails, the previous frame stays and
the failure is shown as a status line, never as a crash.

#### Scenario: No watcher, still live

- **Given** no `mesh watch` running and an agent creating a task in another terminal
- **When** the dashboard's next tick lands
- **Then** the new task appears without any restart

### Requirement: Keys are minimal and boring

The dashboard MUST support: `q` quit, `r` refresh now, `m` toggle a mine-only filter, `Tab` move
pane focus, arrows scroll within the focused pane. v1 is read-only: no key claims, edits or
deletes anything.

#### Scenario: Mine-only filter

- **Given** a dashboard showing all agents' tasks
- **When** the operator presses `m`
- **Then** only the acting identity's tasks and notes remain, and the state is visible in the
  chrome

### Requirement: Human-only, and harmless to the instant CLI

The dashboard MUST NOT be exposed over MCP, and adding it MUST NOT slow any other command: the
UI crates load only on the dashboard path, and the cold-start wall-clock gate MUST stay green.

#### Scenario: Cold start is untouched

- **Given** the shipped binary with the dashboard linked in
- **When** the cold-start wall-clock test runs
- **Then** read commands still start under their budget

### Requirement: The terminal survives the dashboard

On quit, error or panic-escape, the dashboard MUST restore the terminal (cooked mode, cursor,
colors) — a crashed or interrupted dashboard never leaves the operator's shell broken.

#### Scenario: Interrupted dashboard

- **Given** a running dashboard
- **When** it is interrupted or hits an internal error
- **Then** the shell prompt returns usable, in cooked mode

---

## User Experience

The operator runs `mesh dashboard` in a spare terminal tab and leaves it open — the fleet's
pulse at a glance: who holds what, what's ready, what changed, what's unhealthy. See
[design.md](design.md) for layout and interaction detail.
