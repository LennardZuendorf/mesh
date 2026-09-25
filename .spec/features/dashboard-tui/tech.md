---
type: feature-tech
feature: dashboard-tui
sibling: product.md
parent: ../../tech.md
updated: 2026-09-25
---

# Feature: Dashboard TUI — Architecture

A thin, self-contained foreground verb: one loop, one snapshot function, one render mapping.
The snapshot is a pure composition of existing domain reads (the exact surfaces the CLI uses);
the TUI crates touch only the dashboard dispatch path. No locks (read-only, multiple instances
harmless — the deliberate contrast with `mesh watch`), no writes, no MCP.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Design:** [design.md](design.md)

---

## Files

```
Cargo.toml               # + ratatui, + crossterm (new deps — see Decision 1)
src/cli/dashboard.rs     # the whole verb: tty guard, terminal lifecycle, loop, keys
src/cli/mod.rs           # Dashboard subcommand (admin family, human-only)
src/main.rs              # dispatch arm
tests/dashboard_cli.rs   # tty guard, --help, non-tty behaviour
src/cli/dashboard.rs #[cfg(test)]  # snapshot composition unit tests
tests/foundation_cli.rs  # the cold-start wall-clock pin (see Decision 3)
```

---

## Contract / API

```rust
// src/cli/dashboard.rs
pub struct DashOpts { interval: Duration, /* mine: bool lives in ctx globals */ }
fn snapshot(cfg: &Cfg, mine: bool) -> DashFrame;   // pure: one read pass → frame model
struct DashFrame { agents: Vec<AgentRow>, tasks: TasksByState, activity: Vec<ActivityRow>,
                   health: HealthSummary }
fn run(ctx: Ctx, opts: DashOpts) -> Result<()>;   // never returns until quit
```

- **`snapshot`** composes: `status_report` (census + health + foreign counts), `tasks::list`
  with `--ready` / `--blocked` / claimed-state filters, `activity` (recent-activity), and the
  search-health line. Every number on screen comes from these calls — the dashboard adds **no
  second source of truth**. With [note-adoption](../note-adoption/tech.md) landed, the census
  rows already carry note ownership/claims.
- **Terminal lifecycle**: enter (alternate screen, raw mode) → loop → restore. Restoration is
  a **Drop guard**, so quit, error, and the `main` panic-catch all restore cooked mode, cursor
  and colors — a crashed dashboard never leaves the shell broken.
- **Loop**: `crossterm::event::poll` until the tick deadline (`--interval`, default 2 s) →
  re-render from a fresh `snapshot`. `r` forces an immediate tick; a failed refresh keeps the
  last good frame with a dim status line (the safe-reader discipline: a bad file degrades one
  frame, never the loop). `m` toggles the mine filter (reuses `ctx.coalesce_mine`).
- **Tty guard**: no tty on stdin/stdout → exit 2 `dashboard needs a terminal` (headless agents
  get an error, not a hang).

---

## Implementation Detail

### Decisions

1. **ratatui + crossterm are new dependencies** — recorded against the deliberately minimal
   stack: an interactive dashboard is impossible without a TUI layer, and hand-rolling one is
   exactly the kind of second implementation the repo forbids. They are imported **only** by
   `src/cli/dashboard.rs`; the static link cost is borne by the binary image, not by any other
   verb's startup.
2. **No signal crate, no singleton lock.** Quit is a key event (`q`, Ctrl-C read as a key event
   in raw mode); no `O_EXCL` lock because two dashboards are two readers — harmless. (The
   `watch` SIGINT gap is a separate open item in the root plan and stays there.)
3. **The cold-start budget finally gets pinned.** The 10 ms claim is documented but untested
   today (a trace finding). This feature adds the missing wall-clock startup test — a warm
   read command under a generous-but-real bound — so "the dashboard didn't tax the CLI" is
   evidence, not assertion. The test asserts the budget on the shipped binary with ratatui
   linked in.

### Testing

- `snapshot` unit tests (headless, `#[cfg(test)]`): counts per pane, mine filter, empty-vault
  frame, fail-soft on one unreadable file.
- Integration: `dashboard --help`, the tty guard (exit 2 non-tty), and the cold-start pin.
- No pixel tests: the render mapping stays trivial by construction (frame model → widgets).

---

## Performance Budget

A frame is one vault read pass — the same cost as `mesh status` plus two list calls,
milliseconds on this corpus; a 2 s tick is idle-between-ticks. Startup of **other** verbs is
pinned unchanged by the new wall-clock test.
