//! `mesh dashboard` — the operator's read-only screen.
//!
//! Two halves, one verb.
//!
//! The **snapshot**: one read pass that composes the same domain surfaces the CLI verbs already
//! print — the `status` census, the `task list` slices, the recent-activity lens and the
//! search-health report — into a plain frame model. It adds no second source of truth, holds
//! no lock and writes nothing.
//!
//! The **terminal shell**: a tty guard, an alternate-screen lifecycle whose restoration is a
//! `Drop` guard (so a quit, an error and the panic unwinding through `main`'s catch all give
//! the shell back), and an event loop that re-reads the folder on a tick. This module is the
//! only place `ratatui` and `crossterm` are imported.

use std::io::{self, IsTerminal, Stdout};
use std::time::{Duration, Instant};

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{Frame, Terminal};
use serde_json::Value as Json;

use crate::cli::DashboardArgs;
use crate::config::Config;
use crate::ctx::Ctx;
use crate::domain::activity;
use crate::domain::select::{Filter, SortKey};
use crate::domain::tasks::{self, Availability};
use crate::error::{MeshError, Result};

/// How the dashboard is run.
#[derive(Clone, Debug)]
pub struct DashOpts {
    /// The tick interval: how long the loop waits before re-reading the folder.
    pub interval: Duration,
}

/// One agent's row in the agents pane: an identity plus the census the status report built.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentRow {
    /// The owner or claimer identity, as the census reports it.
    pub identity: String,
    /// Tasks this identity owns that are still open.
    pub owns_open: u64,
    /// Tasks this identity holds a claim on.
    pub claimed: u64,
    /// Claims past the stale window.
    pub stale_claims: u64,
    /// Notes this identity owns.
    pub notes_owned: u64,
    /// Notes this identity holds a claim on.
    pub notes_claimed: u64,
}

/// One task row in the tasks pane: the columns the pane prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    /// The claimer, for the claimed slice; `None` for ready and blocked rows.
    pub claimed_by: Option<String>,
}

/// The three slices of the tasks pane, in the order the pane renders them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TasksByState {
    /// Takeable work: open, unclaimed and unblocked.
    pub ready: Vec<TaskRow>,
    /// Open or claimed work with an unsatisfied blocker.
    pub blocked: Vec<TaskRow>,
    /// Work currently held: `status == claimed`.
    pub claimed: Vec<TaskRow>,
}

/// One row of the recent-activity pane, from the seven-key activity row.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityRow {
    pub id: String,
    /// The row's entity type (`note`, `task`, `memory`, …).
    pub kind: String,
    pub title: String,
    pub owner: Option<String>,
    pub claimed_by: Option<String>,
    pub path: String,
    /// The file mtime in epoch seconds — the pane's clock.
    pub mtime_seconds: f64,
}

/// The vault-health pane: the counts, the findings, and the watcher/index liveness.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HealthSummary {
    /// Mesh-native notes (id-bearing files, adopted ones included).
    pub notes_native: u64,
    /// Foreign Markdown: no mesh id, invisible to every lens.
    pub notes_foreign: u64,
    /// The dangling link targets the report lists (capped).
    pub dangling_links: Vec<String>,
    /// The real dangling total, beyond the report's cap.
    pub dangling_links_total: u64,
    /// Lock files past the staleness window.
    pub stale_locks: u64,
    pub watcher_running: bool,
    pub watcher_pid: Option<u32>,
    /// `indexed` when every search gate is open, `fallback` otherwise.
    pub search_mode: String,
    /// The first closed search gate's reason, when search is degraded.
    pub search_reason: Option<String>,
}

/// One frame: everything a single read pass produces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DashFrame {
    pub agents: Vec<AgentRow>,
    pub tasks: TasksByState,
    pub activity: Vec<ActivityRow>,
    pub health: HealthSummary,
}

/// Compose one frame from the domain reads the CLI already prints. Pure: one read pass, no
/// writes, no locks, no second source of truth.
///
/// * the **agents** pane is the `status` census (`lenses::status_report`), which already
///   carries each identity's task counts and note ownership/claims;
/// * the **tasks** pane is three `tasks::list` calls — the `--ready` and `--blocked`
///   availabilities and the `claimed` status slice;
/// * the **activity** pane is the recent-activity lens over its default notes+tasks corpus;
/// * the **health** pane takes the report's link, lock, watcher and note counts and joins the
///   search-health report for the index line.
///
/// `mine` narrows the agents, tasks and activity panes to the acting identity:
/// `cfg.agent()`, the `--owner`-else-`[core].agent` identity every `--mine` resolves — the
/// caller folds `--owner` in through `Config::with_agent`, exactly as `recent-activity` and
/// `session-start` do. With no identity configured, `mine` narrows to nothing, matching the
/// `--mine` discipline everywhere else. The health pane is vault-global and never narrows.
///
/// Fail-soft: every read either degrades on its own (the census and the search report are
/// total) or is collapsed here to an empty pane, so one unreadable or malformed file can
/// never fail the frame.
pub fn snapshot(cfg: &Config, mine: bool) -> DashFrame {
    let report = crate::domain::lenses::status_report(cfg);
    let me: Option<&str> = cfg.agent();
    DashFrame {
        agents: agent_rows(&report, mine, me),
        tasks: TasksByState {
            ready: task_rows(cfg, mine, me, Availability::Ready, None),
            blocked: task_rows(cfg, mine, me, Availability::Blocked, None),
            claimed: task_rows(cfg, mine, me, Availability::Any, Some("claimed")),
        },
        activity: activity_rows(cfg, mine),
        health: health_summary(&report, cfg),
    }
}

/// The agents pane: one row per identity the census reports, in the census's own
/// identity-ascending order. With `mine` the pane keeps only the acting identity — and keeps
/// nothing at all when no identity is configured.
fn agent_rows(report: &Json, mine: bool, me: Option<&str>) -> Vec<AgentRow> {
    let Some(agents) = report.get("agents").and_then(Json::as_object) else {
        return Vec::new();
    };
    agents
        .iter()
        .filter(|(identity, _)| !mine || me.is_some_and(|want| want == identity.as_str()))
        .map(|(identity, counts)| AgentRow {
            identity: identity.clone(),
            owns_open: count_of(counts, "owns_open"),
            claimed: count_of(counts, "claimed"),
            stale_claims: count_of(counts, "stale_claims"),
            notes_owned: count_of(counts, "notes_owned"),
            notes_claimed: count_of(counts, "notes_claimed"),
        })
        .collect()
}

/// One tasks-pane slice: `tasks::list` under the slice's availability, plus the `claimed`
/// status predicate. The sort key mirrors `task list`'s computed default — priority for the
/// ready and blocked slices, updated otherwise — so the dashboard and the CLI agree on the
/// row order. The slice is unbounded; the pane scrolls.
fn task_rows(
    cfg: &Config,
    mine: bool,
    me: Option<&str>,
    availability: Availability,
    status: Option<&str>,
) -> Vec<TaskRow> {
    let filter = Filter {
        mine,
        me: me.map(str::to_string),
        sort: if status.is_some() {
            SortKey::Updated
        } else {
            SortKey::Priority
        },
        limit: None,
        ..Filter::default()
    }
    .with_extra("status", status);
    tasks::list(cfg, &filter, availability)
        .unwrap_or_default()
        .into_iter()
        .map(|view| TaskRow {
            id: view.item.id,
            title: view.item.title,
            claimed_by: view.item.claimed_by,
        })
        .collect()
}

/// The activity pane: the recent-activity lens, capped by its own default limit. `mine` is
/// handed to the lens, which resolves the acting identity itself.
fn activity_rows(cfg: &Config, mine: bool) -> Vec<ActivityRow> {
    activity::recent_activity_in(
        cfg,
        None,
        None,
        mine,
        activity::DEFAULT_LIMIT,
        &activity::DEFAULT_SPACES,
    )
    .unwrap_or_default()
    .iter()
    .map(to_activity_row)
    .collect()
}

/// One seven-key activity row, read by column.
fn to_activity_row(entry: &Json) -> ActivityRow {
    ActivityRow {
        id: text_of(entry, "id"),
        kind: text_of(entry, "type"),
        title: text_of(entry, "title"),
        owner: option_text_of(entry, "owner"),
        claimed_by: option_text_of(entry, "claimed_by"),
        path: text_of(entry, "path"),
        mtime_seconds: entry.get("mtime").and_then(Json::as_f64).unwrap_or(0.0),
    }
}

/// The vault-health pane: the report's counts and findings plus the search-health line.
fn health_summary(report: &Json, cfg: &Config) -> HealthSummary {
    let search = crate::search::health::payload(cfg);
    let watcher = report.get("watcher");
    HealthSummary {
        notes_native: count_of(report, "notes"),
        notes_foreign: count_of(report, "notes_foreign"),
        dangling_links: string_list(report.get("dangling_links")),
        dangling_links_total: count_of(report, "dangling_links_total"),
        stale_locks: report
            .get("stale_locks")
            .and_then(Json::as_array)
            .map_or(0, |locks| locks.len() as u64),
        watcher_running: watcher
            .and_then(|value| value.get("running"))
            .and_then(Json::as_bool)
            .unwrap_or(false),
        watcher_pid: watcher
            .and_then(|value| value.get("pid"))
            .and_then(Json::as_u64)
            .and_then(|pid| u32::try_from(pid).ok()),
        search_mode: search
            .get("mode")
            .and_then(Json::as_str)
            .unwrap_or("fallback")
            .to_string(),
        search_reason: search
            .get("reason")
            .and_then(Json::as_str)
            .map(str::to_string),
    }
}

/// A count column: `0` when the key is absent or not a number.
fn count_of(value: &Json, key: &str) -> u64 {
    value.get(key).and_then(Json::as_u64).unwrap_or(0)
}

/// A string column: `""` when the key is absent or null.
fn text_of(entry: &Json, key: &str) -> String {
    entry
        .get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An identity column: `None` when absent, null or empty.
fn option_text_of(entry: &Json, key: &str) -> Option<String> {
    entry
        .get(key)
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// A string array, or an empty list when the key is absent or not an array.
fn string_list(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------
// the terminal shell
// ---------------------------------------------------------------------------------------

/// The four panes, in `Tab` order: design.md's fixed quadrants, never reordered.
const PANE_TITLES: [&str; 4] = ["agents", "tasks", "recent activity", "vault health"];

/// design.md's four calm empty states: never an error, never a spinner.
const EMPTY_AGENTS: &str = "no agents yet";
const EMPTY_TASKS: &str = "no tasks — all clear";
const EMPTY_ACTIVITY: &str = "no recent activity";
const EMPTY_HEALTH: &str = "healthy — nothing to report";

/// What one key asks the loop to do. This enum is the whole of R3: five keys, read-only, and
/// not one of them claims, edits or deletes anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// `q`, or Ctrl-C read as a key event in raw mode.
    Quit,
    /// `r`: re-read the folder now, without waiting for the tick.
    Refresh,
    /// `m`: flip the mine-only filter.
    ToggleMine,
    /// `Tab`: hand the focus to the next pane.
    FocusNext,
    /// `↑`: scroll the focused pane up one row.
    ScrollUp,
    /// `↓`: scroll the focused pane down one row.
    ScrollDown,
    /// No key this version acts on — and no key event that is not a key press.
    Ignore,
}

/// What the loop must do after one action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    /// Nothing beyond the next draw pass: an ignored key, a moved focus, a scroll.
    Idle,
    /// Re-read the folder now.
    Refresh,
    /// Leave the loop; the guard restores the terminal on the way out.
    Quit,
}

/// Everything one render pass reads: the last good frame, the filter, the focus.
#[derive(Clone, Debug, Default, PartialEq)]
struct DashState {
    /// The last frame read from the folder. A failed refresh never replaces it.
    frame: DashFrame,
    /// The mine-only filter: seeded from the global `--mine`, flipped by `m`.
    mine: bool,
    /// The focused pane, as an index into [`PANE_TITLES`].
    focus: usize,
    /// Per-pane scroll offset in rows, one slot per [`PANE_TITLES`] entry.
    scroll: [usize; PANE_TITLES.len()],
    /// The vault clock of the last successful refresh (`HH:MM:SS`, UTC).
    refreshed_at: Option<String>,
    /// The last refresh failure, dimmed under the chrome; `None` while the frame is fresh.
    status: Option<String>,
}

/// The config the panes read: the global `--owner` swaps the effective identity, exactly as
/// the lenses do, so `--mine` resolves `me` from the flag rather than `[core].agent`.
fn effective_config(ctx: &Ctx) -> Result<Config> {
    Ok(ctx.cfg()?.with_agent(ctx.g.owner.as_deref()))
}

/// `mesh dashboard` — the whole verb. Never returns until the operator quits.
pub fn run(ctx: &mut Ctx, args: DashboardArgs) -> Result<()> {
    // The gate comes first: with no terminal there is nothing to read keys from or draw on,
    // and an agent that asked for a dashboard should be told so, not left hanging.
    terminal_gate(ctx.tty, io::stdout().is_terminal())?;
    let opts = DashOpts {
        interval: Duration::from_secs(args.interval),
    };
    let cfg = effective_config(ctx)?;

    let session = CrosstermSession::enter()?;
    // From here on every exit path is the guard's: a quit, an error, or the unwind `main`
    // catches.
    let mut guard = SessionGuard::new(session);
    let mut source = VaultSource { cfg: &cfg };
    let mut state = DashState {
        mine: ctx.g.mine,
        ..DashState::default()
    };
    // The first frame is read before the first draw, so the screen opens populated.
    let outcome = source.read(state.mine);
    apply_refresh(&mut state, outcome, clock_now());

    drive(guard.session(), &mut source, &mut state, opts.interval)?;
    Ok(())
}

/// The tty gate: the dashboard reads keys on stdin and draws on stdout, so both must be a
/// terminal. A headless agent gets an error at exit 2, never a hang.
fn terminal_gate(stdin_tty: bool, stdout_tty: bool) -> Result<()> {
    if stdin_tty && stdout_tty {
        Ok(())
    } else {
        Err(MeshError::validation("dashboard needs a terminal"))
    }
}

/// The key map, in one place.
fn action_for(key: KeyEvent) -> Action {
    // Windows reports a Release beside every Press; acting on both would double every key.
    if key.kind != KeyEventKind::Press {
        return Action::Ignore;
    }
    match key.code {
        // Ctrl-C is a key event in raw mode, not a signal — there is no signal handler here.
        KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
        // A terminal reports the shifted letter as its uppercase form, so Shift-Q quits too.
        KeyCode::Char('q' | 'Q') => Action::Quit,
        KeyCode::Char('r' | 'R') => Action::Refresh,
        KeyCode::Char('m' | 'M') => Action::ToggleMine,
        KeyCode::Tab => Action::FocusNext,
        KeyCode::Up => Action::ScrollUp,
        KeyCode::Down => Action::ScrollDown,
        _ => Action::Ignore,
    }
}

/// Fold one action into the state. Pure: the loop renders whatever this leaves behind.
fn apply_action(state: &mut DashState, action: Action) -> Effect {
    match action {
        Action::Quit => Effect::Quit,
        Action::Refresh => Effect::Refresh,
        // The filter changes what the next frame holds, so it re-reads at once.
        Action::ToggleMine => {
            state.mine = !state.mine;
            Effect::Refresh
        }
        Action::FocusNext => {
            state.focus = (state.focus + 1) % PANE_TITLES.len();
            Effect::Idle
        }
        Action::ScrollUp => {
            state.scroll[state.focus] = state.scroll[state.focus].saturating_sub(1);
            Effect::Idle
        }
        Action::ScrollDown => {
            let last = pane_rows(state, state.focus).saturating_sub(1);
            let offset = state.scroll[state.focus].saturating_add(1).min(last);
            state.scroll[state.focus] = offset;
            Effect::Idle
        }
        Action::Ignore => Effect::Idle,
    }
}

/// One refresh attempt: the frame read from the folder, or why the read failed.
///
/// `snapshot` is total — every pane degrades on its own — so the production source below
/// cannot fail today. The seam is a `Result` anyway, because R2's fail-soft rule (keep the
/// last good frame, dim a status line, never crash the loop) is behaviour a headless test must
/// be able to watch, and because a read surface may become fallible without the loop changing.
trait FrameSource {
    fn read(&mut self, mine: bool) -> std::result::Result<DashFrame, String>;
}

/// The production refresh: one direct read pass over the folder, every tick.
struct VaultSource<'a> {
    cfg: &'a Config,
}

impl FrameSource for VaultSource<'_> {
    fn read(&mut self, mine: bool) -> std::result::Result<DashFrame, String> {
        Ok(snapshot(self.cfg, mine))
    }
}

/// Record one refresh attempt: a good frame replaces it and stamps the clock; a failed one
/// keeps the last good frame and shows a dim status line. Never fails.
fn apply_refresh(
    state: &mut DashState,
    outcome: std::result::Result<DashFrame, String>,
    clock: String,
) {
    match outcome {
        Ok(frame) => {
            state.frame = frame;
            state.refreshed_at = Some(clock);
            state.status = None;
        }
        Err(reason) => state.status = Some(format!("refresh failed: {reason}")),
    }
}

/// Now, on the vault's clock.
fn clock_now() -> String {
    crate::timefmt::clock_utc(&crate::timefmt::now_utc())
}

/// What the loop needs from the terminal: draw one frame, and give the operator's shell back.
///
/// The restore half is a seam, so the `Drop` guard below is testable without a real tty; the
/// production session runs the real crossterm calls.
trait Session {
    /// Paint the state. A failure here is fatal — the terminal is gone.
    fn draw(&mut self, state: &DashState, interval: Duration) -> io::Result<()>;
    /// Restore cooked mode, the main screen buffer and the cursor.
    fn restore(&mut self);
}

/// The real terminal: raw mode plus the alternate screen, drawn through crossterm.
struct CrosstermSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl CrosstermSession {
    /// Enter the alternate screen and raw mode.
    fn enter() -> io::Result<CrosstermSession> {
        enable_raw_mode()?;
        // Anything that fails past this point must undo the raw mode it already set, or a
        // failure that never reached the guard would leave the shell broken.
        let entered = (|| -> io::Result<CrosstermSession> {
            execute!(io::stdout(), EnterAlternateScreen)?;
            Ok(CrosstermSession {
                terminal: Terminal::new(CrosstermBackend::new(io::stdout()))?,
            })
        })();
        if entered.is_err() {
            let _ = disable_raw_mode();
        }
        entered
    }
}

impl Session for CrosstermSession {
    fn draw(&mut self, state: &DashState, interval: Duration) -> io::Result<()> {
        self.terminal
            .draw(|frame| draw_ui(frame, state, interval))
            .map(|_| ())
    }

    fn restore(&mut self) {
        // Cooked mode first: it has the most side effects. The failures are ignored on
        // purpose — a dashboard that cannot restore has nothing useful left to say.
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
    }
}

/// Owns the session and gives the terminal back when it goes away — on a normal quit, on an
/// error, and on the unwind that `main`'s panic-catch swallows. Nothing else calls `restore`.
struct SessionGuard<S: Session> {
    session: S,
}

impl<S: Session> SessionGuard<S> {
    /// Take ownership of an entered session; dropping the guard restores it.
    fn new(session: S) -> SessionGuard<S> {
        SessionGuard { session }
    }

    /// The session, for the loop to draw on.
    fn session(&mut self) -> &mut S {
        &mut self.session
    }
}

impl<S: Session> Drop for SessionGuard<S> {
    fn drop(&mut self) {
        self.session.restore();
    }
}

/// The foreground loop: draw, wait for a key or the tick deadline, act, repeat. Returns when
/// the operator quits, or when the terminal can no longer be drawn on.
fn drive<S: Session>(
    session: &mut S,
    source: &mut dyn FrameSource,
    state: &mut DashState,
    interval: Duration,
) -> io::Result<()> {
    let mut last_tick = Instant::now();
    loop {
        session.draw(state, interval)?;
        let action = match event::poll(interval.saturating_sub(last_tick.elapsed())) {
            Ok(true) => next_action()?,
            // The tick landed with no key waiting: re-read the folder.
            Ok(false) => Action::Refresh,
            Err(error) => return Err(error),
        };
        match apply_action(state, action) {
            Effect::Quit => return Ok(()),
            Effect::Refresh => {
                let outcome = source.read(state.mine);
                apply_refresh(state, outcome, clock_now());
                last_tick = Instant::now();
            }
            Effect::Idle => {}
        }
    }
}

/// Read one event and map it: only keys act, and a resize simply redraws on the next pass.
fn next_action() -> io::Result<Action> {
    match event::read()? {
        Event::Key(key) => Ok(action_for(key)),
        _ => Ok(Action::Ignore),
    }
}

// ---------------------------------------------------------------------------------------
// the render mapping: the frame model rendered once into widgets
// ---------------------------------------------------------------------------------------

/// Paint one frame: the four fixed quadrants, the chrome and the status line.
fn draw_ui(frame: &mut Frame, state: &DashState, interval: Duration) {
    let [body, chrome_row, status_row] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);
    let [agents, activity] =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(left);
    let [tasks, health] =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(right);

    // The panes fill the left column top-to-bottom, then the right column: the fleet and what
    // changed on the left, the work and the machine on the right.
    for (pane, area) in [(0, agents), (2, activity), (1, tasks), (3, health)] {
        render_pane(frame, area, pane, state);
    }
    frame.render_widget(Paragraph::new(chrome(state, interval)), chrome_row);
    frame.render_widget(status_line(state), status_row);
}

/// Draw one pane: the frame's lines for it, scrolled by the state's offset, under a titled
/// border. The focused pane's border is bright; the others stay dim.
fn render_pane(frame: &mut Frame, area: Rect, pane: usize, state: &DashState) {
    let lines = pane_lines(&state.frame, pane);
    let offset = state.scroll[pane].min(lines.len());
    let border = if state.focus == pane {
        Style::default().fg(Color::Cyan)
    } else {
        dim_style()
    };
    let block = Block::bordered()
        .title(Line::from(format!(" {} ", pane_title(pane))))
        .border_style(border);
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(offset).collect::<Vec<Line>>()).block(block),
        area,
    );
}

/// The rendered rows of one pane, before the border and the scroll offset.
fn pane_lines(frame: &DashFrame, pane: usize) -> Vec<Line<'static>> {
    match pane {
        0 => agents_lines(frame),
        1 => task_lines(frame),
        2 => activity_lines(frame),
        _ => health_lines(frame),
    }
}

/// How many rows a pane holds, for the scroll clamp.
fn pane_rows(state: &DashState, pane: usize) -> usize {
    pane_lines(&state.frame, pane).len()
}

/// The title of a pane, by index.
fn pane_title(pane: usize) -> &'static str {
    PANE_TITLES.get(pane).copied().unwrap_or("")
}

/// The agents pane: one row per identity — open tasks, claims, stale claims, then notes owned
/// and notes claimed, abbreviated `2o 1c 0s · 0n 0nc` as the design's compact rows are.
fn agents_lines(frame: &DashFrame) -> Vec<Line<'static>> {
    if frame.agents.is_empty() {
        return vec![Line::styled(EMPTY_AGENTS, dim_style())];
    }
    frame
        .agents
        .iter()
        .map(|row| {
            Line::from(vec![
                Span::styled(
                    format!(" {:<16}", row.identity),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                count_span(row.owns_open, "o"),
                count_span(row.claimed, "c"),
                count_span(row.stale_claims, "s"),
                Span::styled("· ", dim_style()),
                count_span(row.notes_owned, "n"),
                count_span(row.notes_claimed, "nc"),
            ])
        })
        .collect()
}

/// The tasks pane: the ready, blocked and claimed slices in that order, each row carrying its
/// state word in its status colour, the id, the title, and — on a claim — who holds it.
fn task_lines(frame: &DashFrame) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    push_task_slice(&mut lines, "ready", &frame.tasks.ready, Color::Green);
    push_task_slice(&mut lines, "blocked", &frame.tasks.blocked, Color::Yellow);
    push_task_slice(&mut lines, "claimed", &frame.tasks.claimed, Color::Yellow);
    if lines.is_empty() {
        return vec![Line::styled(EMPTY_TASKS, dim_style())];
    }
    lines
}

/// One tasks-pane slice, appended in slice order.
fn push_task_slice(
    lines: &mut Vec<Line<'static>>,
    state: &'static str,
    rows: &[TaskRow],
    colour: Color,
) {
    for row in rows {
        let mut spans = vec![
            Span::styled(format!(" {state:<8}"), Style::default().fg(colour)),
            Span::styled(format!(" {:<8}", row.id), dim_style()),
            Span::raw(format!(" {}", row.title)),
        ];
        if let Some(claimer) = &row.claimed_by {
            spans.push(Span::styled(format!(" ({claimer})"), dim_style()));
        }
        lines.push(Line::from(spans));
    }
}

/// The recent-activity pane: the vault clock, the id and the title, in the lens's own
/// newest-first order.
fn activity_lines(frame: &DashFrame) -> Vec<Line<'static>> {
    if frame.activity.is_empty() {
        return vec![Line::styled(EMPTY_ACTIVITY, dim_style())];
    }
    frame
        .activity
        .iter()
        .map(|row| {
            let mut spans = vec![
                Span::styled(format!(" {} ", clock_of(row.mtime_seconds)), dim_style()),
                Span::styled(
                    format!("{:<10}", row.id),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" {}", row.title)),
            ];
            if let Some(claimer) = &row.claimed_by {
                spans.push(Span::styled(format!(" ({claimer})"), dim_style()));
            }
            Line::from(spans)
        })
        .collect()
}

/// The vault clock of one file's mtime. An instant mesh cannot place renders as a placeholder
/// rather than failing the frame.
fn clock_of(mtime_seconds: f64) -> String {
    chrono::DateTime::from_timestamp_secs(mtime_seconds as i64).map_or_else(
        || "--:--:--".to_string(),
        |at| crate::timefmt::clock_utc(&at),
    )
}

/// The vault-health pane: the corpus split, the findings, then the watcher and index lines.
fn health_lines(frame: &DashFrame) -> Vec<Line<'static>> {
    let health = &frame.health;
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(vec![
            Span::styled(" notes   ", dim_style()),
            Span::styled(
                format!("{} mesh-native", health.notes_native),
                count_style(health.notes_native),
            ),
            Span::styled(
                format!(" · {} foreign", health.notes_foreign),
                count_style(health.notes_foreign),
            ),
        ]),
        findings_line(health),
        Line::from(vec![
            Span::styled(" watcher ", dim_style()),
            Span::styled(
                watcher_text(health),
                if health.watcher_running {
                    Style::default().fg(Color::Green)
                } else {
                    dim_style()
                },
            ),
            Span::styled(
                format!(" · index {}", health.search_mode),
                if health.search_mode == "indexed" {
                    Style::default().fg(Color::Green)
                } else {
                    dim_style()
                },
            ),
        ]),
    ];
    if let Some(reason) = &health.search_reason {
        lines.push(Line::styled(format!("         {reason}"), dim_style()));
    }
    if !health.dangling_links.is_empty() {
        lines.push(Line::styled(
            format!("         {}", health.dangling_links.join(", ")),
            dim_style(),
        ));
    }
    lines
}

/// The findings line: the dangling-link and stale-lock counts in alert red, or the design's
/// calm empty state when there is nothing to report.
fn findings_line(health: &HealthSummary) -> Line<'static> {
    if health.dangling_links_total == 0 && health.stale_locks == 0 {
        return Line::from(vec![
            Span::styled(" links   ", dim_style()),
            Span::styled(EMPTY_HEALTH, dim_style()),
        ]);
    }
    Line::from(vec![
        Span::styled(" links   ", dim_style()),
        Span::styled(
            format!("{} dangling", health.dangling_links_total),
            count_style(health.dangling_links_total),
        ),
        Span::styled(" · ", dim_style()),
        Span::styled(
            format!("locks {} stale", health.stale_locks),
            count_style(health.stale_locks),
        ),
    ])
}

/// The watcher line: running with its pid, or stopped.
fn watcher_text(health: &HealthSummary) -> String {
    match health.watcher_pid {
        Some(pid) if health.watcher_running => format!("running (pid {pid})"),
        _ => "stopped".to_string(),
    }
}

/// The chrome: design.md's one help line — the tick, the mine badge, the clock of the frame on
/// screen, the focused pane and the five keys.
fn chrome(state: &DashState, interval: Duration) -> Line<'static> {
    let badge = if state.mine {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        dim_style()
    };
    let clock = state.refreshed_at.as_deref().unwrap_or("--:--:--");
    Line::from(vec![
        Span::styled(format!(" {}s ", interval.as_secs()), dim_style()),
        Span::styled("· ", dim_style()),
        Span::styled(if state.mine { "mine:on" } else { "mine:off" }, badge),
        Span::styled(
            format!(
                " · refreshed {clock} UTC · focus {} · q quit · r refresh · m mine · tab focus \
                 · arrows scroll",
                pane_title(state.focus)
            ),
            dim_style(),
        ),
    ])
}

/// The status line under the chrome: the last refresh failure in dim type, or empty while the
/// frame on screen is the folder.
fn status_line(state: &DashState) -> Paragraph<'static> {
    Paragraph::new(state.status.clone().unwrap_or_default()).style(dim_style())
}

/// One `12o`-style count, dimmed at zero — the design's "quiet when healthy" rule.
fn count_span(value: u64, suffix: &'static str) -> Span<'static> {
    Span::styled(format!("{value}{suffix} "), count_style(value))
}

/// The style of a count: dim at zero, default otherwise.
fn count_style(value: u64) -> Style {
    if value == 0 {
        dim_style()
    } else {
        Style::default()
    }
}

/// The dim style: labels, zero counts, the status line.
fn dim_style() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::config::test_support::config_for;
    use std::cell::RefCell;
    use std::fs;
    use std::path::Path;
    use std::rc::Rc;
    use std::time::Duration;

    /// A fresh `updated` stamp, so no fixture task reads as stale against the status window.
    fn stamp() -> String {
        crate::timefmt::iso_z(&crate::timefmt::now_utc())
    }

    /// One note file, optionally owned or claimed.
    fn note(dir: &Path, rel: &str, id: &str, title: &str, owner: Option<&str>, body: &str) {
        let owner = owner.map_or("null".to_string(), str::to_string);
        let text = format!(
            "---\nid: {id}\ntype: note\ntitle: {title}\ntags: []\nowner: {owner}\n\
             created: 2026-01-01T00:00:00Z\nupdated: {}\nrelated: []\n---\n\n{body}\n",
            stamp()
        );
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// One open task file, optionally claimed and blocked.
    fn task(
        dir: &Path,
        id: &str,
        title: &str,
        status: &str,
        owner: &str,
        claimed_by: Option<&str>,
        blocked_by: &[&str],
    ) {
        let claimed = claimed_by.map_or("null".to_string(), str::to_string);
        let blockers: String = blocked_by
            .iter()
            .map(|blocker| format!("\"{blocker}\""))
            .collect::<Vec<String>>()
            .join(", ");
        let text = format!(
            "---\nid: {id}\ntype: task\ntitle: {title}\ntags: []\nowner: {owner}\n\
             created: 2026-01-01T00:00:00Z\nupdated: {}\nrelated: []\nstatus: {status}\n\
             priority: null\nclaimed_by: {claimed}\nproject: null\nblocks: []\n\
             blocked_by: [{blockers}]\n---\n\nbody\n",
            stamp()
        );
        let path = dir.join("tasks").join("open").join(format!("{id}.md"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// The scenario vault: two agents, a claimed, a blocked and a ready task, a dangling
    /// link, an adopted note (foreign stem, mesh id) and a foreign note (no id at all).
    fn fixture(dir: &Path) {
        note(
            dir,
            "notes/team-sol.md",
            "n-adopt",
            "Team solution",
            Some("alice"),
            "Context on [[Missing Page]].",
        );
        note(
            dir,
            "notes/n-bob.md",
            "n-bob",
            "Bob's native note",
            Some("bob"),
            "Bob's words.",
        );
        fs::write(
            dir.join("notes/loose.md"),
            "# Loose Heading\n\nno frontmatter here\n",
        )
        .unwrap();
        task(
            dir,
            "t-claimed",
            "Claimed work",
            "claimed",
            "alice",
            Some("alice"),
            &[],
        );
        task(
            dir,
            "t-blocker",
            "Blocking work",
            "open",
            "alice",
            None,
            &[],
        );
        task(
            dir,
            "t-blocked",
            "Blocked work",
            "open",
            "alice",
            None,
            &["t-blocker"],
        );
        task(dir, "t-ready", "Ready work", "open", "bob", None, &[]);
    }

    fn sorted_task_ids(rows: &[TaskRow]) -> Vec<String> {
        let mut ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
        ids.sort();
        ids
    }

    fn sorted_activity_ids(rows: &[ActivityRow]) -> Vec<String> {
        let mut ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
        ids.sort();
        ids
    }

    #[test]
    fn every_pane_reads_one_pass_of_the_domain_surfaces() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        fixture(dir.path());
        let frame = snapshot(&cfg, false);

        assert_eq!(
            frame.agents,
            vec![
                AgentRow {
                    identity: "alice".to_string(),
                    owns_open: 2,
                    claimed: 1,
                    stale_claims: 0,
                    notes_owned: 1,
                    notes_claimed: 0,
                },
                AgentRow {
                    identity: "bob".to_string(),
                    owns_open: 1,
                    claimed: 0,
                    stale_claims: 0,
                    notes_owned: 1,
                    notes_claimed: 0,
                },
            ]
        );

        assert_eq!(
            sorted_task_ids(&frame.tasks.ready),
            ["t-blocker", "t-ready"]
        );
        assert_eq!(sorted_task_ids(&frame.tasks.blocked), ["t-blocked"]);
        assert_eq!(sorted_task_ids(&frame.tasks.claimed), ["t-claimed"]);
        assert_eq!(frame.tasks.claimed[0].title, "Claimed work");
        assert_eq!(frame.tasks.claimed[0].claimed_by.as_deref(), Some("alice"));

        // The feed is the notes+tasks corpus: two notes and four tasks; the foreign note
        // carries no id and never appears.
        assert_eq!(
            sorted_activity_ids(&frame.activity),
            [
                "n-adopt",
                "n-bob",
                "t-blocked",
                "t-blocker",
                "t-claimed",
                "t-ready"
            ]
        );
        let claimed_row = frame
            .activity
            .iter()
            .find(|row| row.id == "t-claimed")
            .unwrap();
        assert_eq!(claimed_row.kind, "task");
        assert_eq!(claimed_row.claimed_by.as_deref(), Some("alice"));
        assert_eq!(claimed_row.owner.as_deref(), Some("alice"));
        assert!(!claimed_row.path.is_empty());

        assert_eq!(frame.health.notes_native, 2);
        assert_eq!(frame.health.notes_foreign, 1);
        assert_eq!(frame.health.dangling_links, ["Missing Page"]);
        assert_eq!(frame.health.dangling_links_total, 1);
        assert_eq!(frame.health.stale_locks, 0);
        assert!(!frame.health.watcher_running);
        assert_eq!(frame.health.watcher_pid, None);
        assert_eq!(frame.health.search_mode, "fallback");
        assert_eq!(
            frame.health.search_reason.as_deref(),
            Some(crate::search::health::REASON_COLLECTION)
        );
    }

    #[test]
    fn the_global_owner_becomes_the_effective_agent() {
        let dir = tempfile::tempdir().unwrap();
        let g = crate::cli::globals::GlobalOpts {
            owner: Some("bob".into()),
            ..Default::default()
        };
        let ctx = Ctx::with_config(g, config_for(dir.path()), true);
        assert_eq!(effective_config(&ctx).unwrap().agent(), Some("bob"));
    }

    #[test]
    fn mine_narrows_the_agents_tasks_and_activity_panes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path()).with_agent(Some("alice"));
        fixture(dir.path());
        let frame = snapshot(&cfg, true);

        assert_eq!(frame.agents.len(), 1);
        assert_eq!(frame.agents[0].identity, "alice");
        assert_eq!(frame.agents[0].notes_owned, 1);

        assert_eq!(sorted_task_ids(&frame.tasks.ready), ["t-blocker"]);
        assert_eq!(sorted_task_ids(&frame.tasks.blocked), ["t-blocked"]);
        assert_eq!(sorted_task_ids(&frame.tasks.claimed), ["t-claimed"]);

        assert_eq!(
            sorted_activity_ids(&frame.activity),
            ["n-adopt", "t-blocked", "t-blocker", "t-claimed"]
        );

        // Vault health is global: the mine filter never narrows it.
        assert_eq!(frame.health.notes_native, 2);
        assert_eq!(frame.health.notes_foreign, 1);
    }

    #[test]
    fn mine_without_an_identity_narrows_to_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.core.agent = None;
        fixture(dir.path());
        let frame = snapshot(&cfg, true);

        // `--mine` with no identity configured matches nothing, the discipline every other
        // `--mine` keeps — never "every unclaimed row".
        assert!(frame.agents.is_empty());
        assert_eq!(frame.tasks, TasksByState::default());
        assert!(frame.activity.is_empty());
        // Without the filter the same vault still reports everything.
        let all = snapshot(&cfg, false);
        assert_eq!(all.agents.len(), 2);
        assert_eq!(all.tasks.claimed.len(), 1);
        assert_eq!(all.activity.len(), 6);
    }

    #[test]
    fn an_empty_vault_renders_a_calm_empty_frame() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let frame = snapshot(&cfg, false);

        assert!(frame.agents.is_empty());
        assert_eq!(frame.tasks, TasksByState::default());
        assert!(frame.activity.is_empty());
        assert_eq!(frame.health.notes_native, 0);
        assert_eq!(frame.health.notes_foreign, 0);
        assert!(frame.health.dangling_links.is_empty());
        assert_eq!(frame.health.dangling_links_total, 0);
        assert_eq!(frame.health.stale_locks, 0);
        assert!(!frame.health.watcher_running);
        assert_eq!(frame.health.search_mode, "fallback");
    }

    #[test]
    fn one_corrupt_file_degrades_its_own_pane_and_not_the_frame() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        fixture(dir.path());
        let before = snapshot(&cfg, false);
        // The base frame is real before the corrupt files land, so the equality below is
        // not vacuously true.
        assert_eq!(before.agents.len(), 2);
        assert_eq!(before.tasks.claimed.len(), 1);
        assert_eq!(before.activity.len(), 6);

        fs::write(
            dir.path().join("notes/n-bad.md"),
            "---\ntitle: [oops\n---\n\nx",
        )
        .unwrap();
        fs::write(
            dir.path().join("tasks/open/t-bad.md"),
            "---\nid: t-bad\nstatus: [oops\n---\n\nx",
        )
        .unwrap();
        let after = snapshot(&cfg, false);

        assert_eq!(
            after, before,
            "a corrupt file must not move any other number"
        );
    }

    // ------------------------------------------------------------------------------------
    // the terminal shell: every part of the loop that needs no real terminal
    // ------------------------------------------------------------------------------------

    /// The chrome as text, at the default tick.
    fn chrome_text(state: &DashState) -> String {
        chrome(state, Duration::from_secs(2)).to_string()
    }

    /// One pane's rendered rows as text. The render mapping has no pixel tests, so this is
    /// what the tests below assert on.
    fn pane_text(frame: &DashFrame, pane: usize) -> String {
        pane_lines(frame, pane)
            .iter()
            .map(Line::to_string)
            .collect::<Vec<String>>()
            .join("\n")
    }

    fn agent(identity: &str) -> AgentRow {
        AgentRow {
            identity: identity.to_string(),
            ..AgentRow::default()
        }
    }

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    /// The headless stand-in for the real terminal: records what the loop asked of it.
    #[derive(Default)]
    struct RecordingSession {
        log: Rc<RefCell<Vec<String>>>,
    }

    impl Session for RecordingSession {
        fn draw(&mut self, state: &DashState, _interval: Duration) -> std::io::Result<()> {
            self.log
                .borrow_mut()
                .push(format!("draw mine={}", state.mine));
            Ok(())
        }

        fn restore(&mut self) {
            self.log.borrow_mut().push("restore".to_string());
        }
    }

    /// The refresh seam: one scripted outcome per call, the last one repeating.
    struct ScriptedSource {
        outcomes: Vec<std::result::Result<DashFrame, String>>,
        calls: usize,
    }

    impl FrameSource for ScriptedSource {
        fn read(&mut self, _mine: bool) -> std::result::Result<DashFrame, String> {
            let index = self.calls.min(self.outcomes.len().saturating_sub(1));
            self.calls += 1;
            self.outcomes
                .get(index)
                .cloned()
                .unwrap_or_else(|| Ok(DashFrame::default()))
        }
    }

    #[test]
    fn the_tty_gate_needs_a_terminal_on_both_streams() {
        assert!(terminal_gate(true, true).is_ok());
        for (stdin_tty, stdout_tty) in [(false, false), (false, true), (true, false)] {
            let error = terminal_gate(stdin_tty, stdout_tty).unwrap_err();
            assert_eq!(error.code(), 2);
            assert_eq!(error.to_string(), "dashboard needs a terminal");
        }
    }

    #[test]
    fn the_key_map_is_the_whole_of_r3() {
        let plain = KeyModifiers::NONE;
        assert_eq!(action_for(press(KeyCode::Char('q'), plain)), Action::Quit);
        assert_eq!(
            action_for(press(KeyCode::Char('r'), plain)),
            Action::Refresh
        );
        assert_eq!(
            action_for(press(KeyCode::Char('m'), plain)),
            Action::ToggleMine
        );
        assert_eq!(action_for(press(KeyCode::Tab, plain)), Action::FocusNext);
        assert_eq!(action_for(press(KeyCode::Up, plain)), Action::ScrollUp);
        assert_eq!(action_for(press(KeyCode::Down, plain)), Action::ScrollDown);
        // Ctrl-C arrives as a key event in raw mode, never as a signal.
        assert_eq!(
            action_for(press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
        // A plain `c` is not Ctrl-C.
        assert_eq!(action_for(press(KeyCode::Char('c'), plain)), Action::Ignore);
        // v1 is read-only: no other key claims, edits, deletes or leaves the screen.
        for code in [
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Char('d'),
            KeyCode::Char('x'),
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::PageDown,
            KeyCode::Backspace,
        ] {
            assert_eq!(action_for(press(code, plain)), Action::Ignore, "{code:?}");
        }
        // A Windows Release event beside the Press is not a second action.
        let mut release = press(KeyCode::Char('q'), plain);
        release.kind = KeyEventKind::Release;
        assert_eq!(action_for(release), Action::Ignore);
    }

    #[test]
    fn r_refreshes_at_once_and_m_changes_what_the_next_frame_holds() {
        let mut state = DashState::default();
        assert_eq!(apply_action(&mut state, Action::Refresh), Effect::Refresh);
        assert_eq!(apply_action(&mut state, Action::Quit), Effect::Quit);
        assert_eq!(apply_action(&mut state, Action::Ignore), Effect::Idle);

        // `m` flips the filter and re-reads, so the panes change in the same tick.
        assert_eq!(
            apply_action(&mut state, Action::ToggleMine),
            Effect::Refresh
        );
        assert!(state.mine);
        assert_eq!(
            apply_action(&mut state, Action::ToggleMine),
            Effect::Refresh
        );
        assert!(!state.mine);
    }

    #[test]
    fn the_chrome_carries_the_tick_the_mine_badge_the_clock_and_the_focus() {
        let state = DashState {
            refreshed_at: Some("09:41:07".to_string()),
            ..DashState::default()
        };
        let text = chrome_text(&state);
        assert!(text.contains(" 2s "), "{text}");
        assert!(text.contains("mine:off"), "{text}");
        assert!(text.contains("refreshed 09:41:07 UTC"), "{text}");
        assert!(text.contains("focus agents"), "{text}");
        assert!(text.contains("q quit"), "{text}");

        // The badge follows the filter, so the mine state is visible in the chrome.
        let on = DashState {
            mine: true,
            ..state.clone()
        };
        assert!(chrome_text(&on).contains("mine:on"), "{}", chrome_text(&on));
        assert!(!chrome_text(&on).contains("mine:off"));
    }

    #[test]
    fn tab_cycles_the_focus_and_the_arrows_scroll_only_the_focused_pane() {
        let mut state = DashState {
            frame: DashFrame {
                agents: vec![agent("alice"), agent("bob")],
                ..DashFrame::default()
            },
            ..DashState::default()
        };
        assert_eq!(apply_action(&mut state, Action::FocusNext), Effect::Idle);
        assert_eq!(state.focus, 1);
        // Tab cycles through the four panes and never leaves them.
        for _ in 0..PANE_TITLES.len() {
            apply_action(&mut state, Action::FocusNext);
        }
        assert_eq!(state.focus, 1);

        // The tasks pane is empty: there is nothing to scroll past.
        apply_action(&mut state, Action::ScrollDown);
        assert_eq!(state.scroll[1], 0);

        // The arrows move the focused pane only, and stop at its last row.
        state.focus = 0;
        apply_action(&mut state, Action::ScrollDown);
        assert_eq!(state.scroll[0], 1);
        assert_eq!(state.scroll[1], 0);
        apply_action(&mut state, Action::ScrollDown);
        assert_eq!(state.scroll[0], 1, "two rows: one scroll and no further");
        apply_action(&mut state, Action::ScrollUp);
        assert_eq!(state.scroll[0], 0);
        apply_action(&mut state, Action::ScrollUp);
        assert_eq!(state.scroll[0], 0);
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_good_frame_and_dims_a_status_line() {
        let first = DashFrame {
            agents: vec![agent("alice")],
            ..DashFrame::default()
        };
        let second = DashFrame {
            agents: vec![agent("alice"), agent("bob")],
            ..DashFrame::default()
        };
        let mut source = ScriptedSource {
            outcomes: vec![
                Ok(first.clone()),
                Err("a pane could not be read".to_string()),
                Ok(second.clone()),
            ],
            calls: 0,
        };
        let mut state = DashState::default();

        // The first read lands: frame in, clock stamped, no status line.
        let outcome = source.read(state.mine);
        apply_refresh(&mut state, outcome, "10:00:00".to_string());
        assert_eq!(state.frame, first);
        assert_eq!(state.refreshed_at.as_deref(), Some("10:00:00"));
        assert_eq!(state.status, None);

        // The failed read keeps the last good frame and says so — never a crash.
        let outcome = source.read(state.mine);
        apply_refresh(&mut state, outcome, "10:00:02".to_string());
        assert_eq!(state.frame, first, "the previous frame stays on screen");
        assert_eq!(state.refreshed_at.as_deref(), Some("10:00:00"));
        assert_eq!(
            state.status.as_deref(),
            Some("refresh failed: a pane could not be read")
        );

        // The next good read replaces the frame and clears the status line.
        let outcome = source.read(state.mine);
        apply_refresh(&mut state, outcome, "10:00:04".to_string());
        assert_eq!(state.frame, second);
        assert_eq!(state.refreshed_at.as_deref(), Some("10:00:04"));
        assert_eq!(state.status, None);
    }

    #[test]
    fn the_drop_guard_restores_the_terminal() {
        let session = RecordingSession::default();
        let log = Rc::clone(&session.log);
        {
            let mut guard = SessionGuard::new(session);
            guard
                .session()
                .draw(&DashState::default(), Duration::from_secs(2))
                .unwrap();
            assert_eq!(log.borrow().as_slice(), ["draw mine=false"]);
        }
        // Leaving the scope restored the terminal — the one exit path the guard owns.
        assert_eq!(log.borrow().as_slice(), ["draw mine=false", "restore"]);
    }

    #[test]
    fn the_frame_draws_into_a_terminal_buffer() {
        // Not a pixel test: it only proves the whole mapping runs — every pane, the chrome
        // and the status line — and lands on a real buffer without failing.
        let state = DashState {
            refreshed_at: Some("09:41:07".to_string()),
            status: Some("refresh failed: a pane could not be read".to_string()),
            ..DashState::default()
        };
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| draw_ui(frame, &state, Duration::from_secs(2)))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let width = buffer.area.width as usize;
        let mut rendered = String::new();
        for (index, cell) in buffer.content.iter().enumerate() {
            if index > 0 && index % width == 0 {
                rendered.push('\n');
            }
            rendered.push_str(cell.symbol());
        }
        for expected in [
            "agents",
            "tasks",
            "recent activity",
            "vault health",
            "no agents yet",
            "mine:off",
            "2s",
            "refresh failed: a pane could not be read",
        ] {
            assert!(
                rendered.contains(expected),
                "{expected} missing:\n{rendered}"
            );
        }
    }

    #[test]
    fn an_empty_vault_renders_the_four_calm_empty_states() {
        let frame = DashFrame::default();
        for (pane, expected) in [
            (0, "no agents yet"),
            (1, "no tasks — all clear"),
            (2, "no recent activity"),
            (3, "healthy — nothing to report"),
        ] {
            let text = pane_text(&frame, pane);
            assert!(text.contains(expected), "pane {pane}: {text}");
        }
    }

    #[test]
    fn the_panes_render_the_frame_they_are_given() {
        let mtime = 1_772_000_000.0;
        let clock = crate::timefmt::clock_utc(
            &chrono::DateTime::from_timestamp_secs(mtime as i64).unwrap(),
        );
        let frame = DashFrame {
            agents: vec![AgentRow {
                identity: "alice".to_string(),
                owns_open: 2,
                claimed: 1,
                stale_claims: 0,
                notes_owned: 0,
                notes_claimed: 0,
            }],
            tasks: TasksByState {
                ready: vec![TaskRow {
                    id: "t-A".to_string(),
                    title: "Ready work".to_string(),
                    claimed_by: None,
                }],
                blocked: vec![TaskRow {
                    id: "t-B".to_string(),
                    title: "Blocked work".to_string(),
                    claimed_by: None,
                }],
                claimed: vec![TaskRow {
                    id: "t-C".to_string(),
                    title: "Claimed work".to_string(),
                    claimed_by: Some("alice".to_string()),
                }],
            },
            activity: vec![ActivityRow {
                id: "n-1".to_string(),
                kind: "note".to_string(),
                title: "A note".to_string(),
                owner: Some("alice".to_string()),
                claimed_by: None,
                path: "/vault/n-1.md".to_string(),
                mtime_seconds: mtime,
            }],
            health: HealthSummary {
                notes_native: 2,
                notes_foreign: 1,
                dangling_links: vec!["Missing Page".to_string()],
                dangling_links_total: 1,
                stale_locks: 0,
                watcher_running: true,
                watcher_pid: Some(42),
                search_mode: "indexed".to_string(),
                search_reason: None,
            },
        };

        let agents = pane_text(&frame, 0);
        assert!(agents.contains("alice"), "{agents}");
        assert!(agents.contains("2o") && agents.contains("1c"), "{agents}");

        let tasks = pane_text(&frame, 1);
        for expected in [
            "ready",
            "t-A",
            "Ready work",
            "blocked",
            "t-B",
            "claimed",
            "(alice)",
        ] {
            assert!(tasks.contains(expected), "{expected} missing from {tasks}");
        }

        let activity = pane_text(&frame, 2);
        assert!(activity.contains(&clock), "{activity}");
        assert!(
            activity.contains("n-1") && activity.contains("A note"),
            "{activity}"
        );

        let health = pane_text(&frame, 3);
        for expected in [
            "2 mesh-native",
            "1 foreign",
            "1 dangling",
            "Missing Page",
            "running (pid 42)",
            "index indexed",
        ] {
            assert!(
                health.contains(expected),
                "{expected} missing from {health}"
            );
        }
        assert!(
            !health.contains("nothing to report"),
            "findings are on screen: {health}"
        );
    }
}
