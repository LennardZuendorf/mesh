---
type: feature-design
feature: dashboard-tui
sibling: product.md
parent: ../../design.md
updated: 2026-09-25
---

# Feature: Dashboard TUI — Design

A calm operations screen, not a control panel. The dashboard is glanceable from across the room:
counts big enough to read at arm's length, ids and titles aligned in columns, nothing that
blinks. It borrows the terminal's native furniture — box-drawing borders, dim labels, bright
values — and adds no color beyond status semantics.

**Parent:** [../../design.md](../../design.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

---

## Design Intent

- **Operator-first.** This is the human's window on an agent fleet, not an agent surface. No
  mutations, no prompts, no confirmations — reading only.
- **Stable layout.** Panes never move or reorder; content changes under them. A glance should
  land on the same place every time.
- **Quiet when healthy.** Zero counts and "watcher: stopped" render dim. Color appears only for
  meaning: ready (green), blocked/claimed-by-another (yellow), stale locks and dangling links
  (red), foreign counts (dim/informational).

---

## Interaction Patterns

| Pattern | Use When | Notes |
|---|---|---|
| Tick refresh | Always | Poll interval in the chrome ("2s"), so the operator always knows what they're looking at |
| Focus ring | Multiple panes | One pane has a bright border; `Tab` cycles, arrows scroll only within it |
| Mine toggle | Operator filters the noise | `m` flips a persistent chrome badge; the filter applies to tasks + notes panes |
| Fail-soft refresh | A read errors mid-tick | Keep last good frame, dim a status line "refresh failed: <reason>" |

### Layout

```
┌─ agents ────────────────┐┌─ tasks ─────────────────────────────┐
│ flights-agent   2o 1c   ││ ready    t-AB12  fix NDC pricing      │
│ notes-agent     0o 3c   ││ blocked  t-CD34  awaiting auth keys   │
│ product-analytics 1o 0c ││ claimed  t-EF56  (flights-agent)      │
│ lennarddib      1o 0o   ││                                        │
├─ recent activity ───────┤├─ vault health ───────────────────────┤
│ 14:02 t-EF56 claimed    ││ notes 655 foreign / 1 native          │
│ 13:58 n-K3ZP adopted    ││ links 3 dangling · locks 0 stale      │
│ 13:41 m-0001 recalled   ││ watcher stopped · index substring     │
└─────────────────────────┘└────────────────────────────────────────┘
 2s · mine:off · q quit · r refresh · m mine · tab focus · arrows scroll
```

Left column: the fleet (top) and what changed (bottom). Right column: the work (top) and the
machine's health (bottom). Four quadrants, fixed positions.

### Empty states

Every pane has one: `no agents yet`, `no tasks — all clear`, `no recent activity`, `healthy —
nothing to report`. Never an error, never a spinner.

---

## Language & Copy

- Identity and ids render exactly as the CLI prints them (`flights-agent`, `t-AB12`) — the
  dashboard is a view of the same surface, not a rebranding.
- Status words match the CLI's vocabulary verbatim: open, claimed, ready, blocked, done,
  stale, dangling. No synonyms.
- The help line is the only chrome prose, and it stays one line.

---

## Do's and Don'ts

- Do render a frame fully from one read pass — the panes of a single tick are a consistent
  snapshot, never half-old half-new.
- Don't add a second source of truth: every number on screen comes from the same domain reads
  the CLI uses.
- Don't animate values or flash on change; the tick line is the only motion.
