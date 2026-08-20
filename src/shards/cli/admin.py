"""Admin surface: ``shards init``, ``daemon`` lifecycle, ``status``, ``reindex``.

These are *admin* commands (spec R4), not new verbs — the three-verb rule
(``note`` / ``task`` / ``search``) is untouched. Everything here honours the
daemon's founding rule: it is an **accelerator, never a gate**. ``shards status``
reads vault health from the warm index when the daemon is up and falls back to
the identical direct filesystem scan when it is down, and ``reindex`` degrades to
a notice, so both work with the daemon down.

* **``shards init``** (agent-usability/7) — writes ``~/.shards/config.toml`` (or
  ``$SHARDS_CONFIG_PATH``): vault path, agent identity, roster, search settings.
  Refuses to overwrite an existing config without ``--force`` (the file is left
  byte-for-byte alone on refusal — no read-then-rewrite-identical), so running it
  twice is always safe. Every path it writes or creates is ``expanduser()``'d
  first (the same rule ``CoreConfig.__post_init__`` enforces on load — a literal
  ``~/vault`` must never become a relative ``./~/vault`` directory). Human-only:
  withheld from the MCP surface like the rest of this module — nothing here is
  agent-safe to trigger remotely, since it writes the very config every other
  command depends on.
* **``daemon start|stop|status``** — supervise a detached socket server. Its PID
  lives in a state file beside the socket (``$XDG_RUNTIME_DIR/shards.pid``, else
  ``~/.shards/run/shards.pid``), written atomically (temp file + ``os.replace``).
  ``start`` is idempotent (a live PID → "already running", no second spawn);
  ``stop`` is idempotent (no live PID → "not running"). The process spawn and the
  termination signal are isolated in :func:`spawn_daemon` / :func:`terminate_process`
  so the lifecycle logic is testable without forking a real daemon.
* **``shards status``** — note count, tasks-by-status, freshness (newest vault
  mtime + age), dangling wikilinks (via :func:`shards.core.wikilinks.find_dangling`),
  and stale ``O_EXCL`` locks (PID dead **or** age > 300 s, reusing the canonical
  rule from :mod:`shards.storage.locks`). Served from the daemon's warm index when
  it is up and from a direct scan when it is down — same payload either way, via
  :meth:`shards.daemon.client.DaemonClient.vault_status`. Strictly read-only — it
  never bumps ``updated`` nor rewrites a file.
* **``shards reindex``** — delegates to the search feature's ``indexed`` client
  (:func:`shards.index.indexed_client.reindex`). A missing binary or a non-zero
  exit degrades gracefully: one stderr notice, exit 0, suppressed under
  ``--quiet``.

Output follows the design language: ``--json`` for machines, terse text for
humans, infrastructure notices on stderr (hidden by ``--quiet``).
"""

from __future__ import annotations

import contextlib
import json
import os
import signal
import subprocess
import sys
from pathlib import Path
from typing import Any

import typer

from shards.cli import _output
from shards.cli._errors import cli_errors
from shards.core.lenses import TASK_STATUSES

# Reused rather than re-derived: the same duration grammar ``--stale`` parses.
from shards.core.notes import _parse_since
from shards.core.tasks import TaskView
from shards.daemon.client import DaemonClient, default_socket_path
from shards.schemas.config import load_config, resolve_config_path
from shards.storage.files import atomic_write

# Reuse the canonical liveness rule rather than re-deriving it.
from shards.storage.locks import _pid_alive

_PID_NAME = "shards.pid"
# ``shards init`` defaults — a first run with no flags at all still produces a
# working config (agent-usability/7's load-bearing test: note new / task list
# actually succeed afterward, not just "a file exists").
_DEFAULT_VAULT_PATH = Path.home() / ".shards" / "vault"
_DEFAULT_AGENT = "agent"
# The staleness window for the per-agent "stale claims" count in ``shards
# status`` (team-awareness/4) — a claim is stale here iff it has not been
# touched (``updated``, which bumps on every write including ``task append``)
# in this long. Matches the illustrative threshold in product.md's own
# board-visibility scenario; an operator who wants a different window runs the
# equivalent query directly: ``task list --status claimed --stale <dur>``.
_STATUS_STALE_WINDOW = "2d"


# --------------------------------------------------------------------------- #
# PID state file                                                              #
# --------------------------------------------------------------------------- #


def default_pid_path() -> Path:
    """Resolve the daemon PID file — beside the socket (``$XDG_RUNTIME_DIR`` etc.)."""
    return default_socket_path().parent / _PID_NAME


def write_pid(pid_path: Path, pid: int) -> None:
    """Atomically record ``pid`` in ``pid_path`` (temp file + ``os.replace``)."""
    atomic_write(pid_path, f"{pid}\n")


def read_pid(pid_path: Path) -> int | None:
    """Return the PID recorded in ``pid_path``, or ``None`` if absent/malformed."""
    try:
        raw = pid_path.read_text(encoding="utf-8").strip()
    except (FileNotFoundError, IsADirectoryError):
        return None
    try:
        return int(raw)
    except ValueError:
        return None


def daemon_running(pid_path: Path) -> int | None:
    """The live daemon PID from ``pid_path``, or ``None`` when down/stale."""
    pid = read_pid(pid_path)
    if pid is None:
        return None
    return pid if _pid_alive(pid) else None


# --------------------------------------------------------------------------- #
# Process seams (mockable in tests; the only place a real daemon is spawned)  #
# --------------------------------------------------------------------------- #


def spawn_daemon() -> int:
    """Launch the socket server detached and return its PID.

    Runs :func:`shards.daemon.server.serve` in a fresh, session-leading process; the
    child inherits ``$XDG_RUNTIME_DIR`` / ``$SHARDS_CONFIG_PATH`` so it resolves the
    same socket + config as this CLI. Its streams are detached (``/dev/null``) so
    the daemon outlives this invocation.
    """
    proc = subprocess.Popen(  # noqa: S603 - fixed argv, no shell, no user input
        [sys.executable, "-c", "from shards.daemon.server import serve; serve()"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
        close_fds=True,
    )
    return proc.pid


def terminate_process(pid: int) -> None:
    """Send ``SIGTERM`` to ``pid`` (a vanished process is not an error)."""
    with contextlib.suppress(ProcessLookupError):
        os.kill(pid, signal.SIGTERM)


def _remove(path: Path) -> None:
    with contextlib.suppress(FileNotFoundError, IsADirectoryError):
        path.unlink()


# --------------------------------------------------------------------------- #
# Output helpers                                                              #
# --------------------------------------------------------------------------- #


def _notice(ctx: typer.Context, message: str) -> None:
    """Emit an infrastructure notice on stderr (suppressed under ``--quiet``)."""
    if not _output.is_quiet(ctx):
        typer.echo(message, err=True)


def _emit(ctx: typer.Context, payload: dict[str, Any], human: str) -> None:
    """Machine JSON on ``--json``; a terse human line otherwise (silent if quiet).

    ``payload`` is this command's own already-built ``{...}`` shape (no per-target
    id, no ``updated`` timestamp) — a different contract from
    :func:`shards.cli._output.emit_mutation`'s note/task envelope, so this stays
    admin's own helper rather than forcing the two together (root tech.md §
    Duplication).
    """
    if _output.is_json(ctx):
        typer.echo(json.dumps(payload))
    elif not _output.is_quiet(ctx):
        typer.echo(human)


# --------------------------------------------------------------------------- #
# shards init                                                                  #
# --------------------------------------------------------------------------- #


def _parse_roster(raw: str | None) -> list[str]:
    """``--collections`` (comma-separated) into a roster list.

    ``None``/empty → ``[]``, the open-roster default: ``[tasks].collections``
    empty means any ``--owner`` string is accepted (``core/notes.py``,
    ``core/tasks.py`` both special-case it that way already).
    """
    if not raw:
        return []
    return [item.strip() for item in raw.split(",") if item.strip()]


def _toml_string(value: str) -> str:
    """A TOML basic-string literal for ``value`` (escapes backslash and quote)."""
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def render_config_toml(
    *,
    vault_path: Path,
    agent: str,
    collections: list[str],
    search_collection: str | None,
    hybrid: bool,
    threshold: float,
) -> str:
    """Render a complete, ``load_config``-parseable ``config.toml``.

    Every ``[core]``/``[search]``/``[tasks]`` key is written explicitly rather
    than left to the schema's own defaults, so the file this writes is a
    legible reference by itself — mirroring the shape of the committed
    ``config.example.toml`` at the repo root.
    """
    lines = [
        "[core]",
        f"vault_path = {_toml_string(str(vault_path))}",
        f"agent = {_toml_string(agent)}",
        "",
        "[search]",
        f"hybrid = {'true' if hybrid else 'false'}",
        f"threshold = {threshold}",
    ]
    if search_collection:
        lines.append(f"collection = {_toml_string(search_collection)}")
    roster = ", ".join(_toml_string(item) for item in collections)
    lines.extend(["", "[tasks]", f"collections = [{roster}]", ""])
    return "\n".join(lines)


def init_command(
    ctx: typer.Context,
    path: str | None = typer.Option(
        None,
        "--path",
        help="Vault folder ([core].vault_path). Defaults to ~/.shards/vault.",
    ),
    agent: str | None = typer.Option(
        None,
        "--agent",
        help="This agent's identity ([core].agent). Defaults to $SHARDS_AGENT, else 'agent'.",
    ),
    collections: str | None = typer.Option(
        None,
        "--collections",
        help=(
            "Comma-separated roster of valid --owner identities ([tasks].collections). "
            "Default: empty — an open roster, any owner string accepted."
        ),
    ),
    search_collection: str | None = typer.Option(
        None,
        "--search-collection",
        help="indexed collection name ([search].collection). Default: unset.",
    ),
    hybrid: bool = typer.Option(
        True,
        "--hybrid/--no-hybrid",
        help="Hybrid lexical+vector search via indexed ([search].hybrid). Default: on.",
    ),
    threshold: float = typer.Option(
        0.65,
        "--threshold",
        help="Substring-fallback score floor ([search].threshold). Default: 0.65.",
    ),
    force: bool = typer.Option(
        False, "--force", help="Overwrite an existing config. Default: refuse."
    ),
) -> None:
    """Write ~/.shards/config.toml (or $SHARDS_CONFIG_PATH); refuses to overwrite without --force.

    Admin (root AGENTS.md §6), not a fourth verb — beside ``daemon`` / ``status`` /
    ``reindex``, and withheld from the MCP surface for the same reason they are:
    this writes the very config every other command (CLI and MCP alike) depends
    on, so triggering it remotely is never agent-safe. Idempotent and
    non-destructive: with an existing config and no ``--force``, the file is
    never opened for writing at all (refuse-first, no read-then-rewrite of
    identical content) — running ``init`` twice in a row is always safe.
    ``--path`` is ``expanduser()``'d before it is used for anything (creating
    the vault directory or writing it into the config), the same rule
    ``CoreConfig.__post_init__`` enforces on load.
    """
    cfg_path = resolve_config_path()
    with cli_errors():
        if cfg_path.is_file() and not force:
            raise ValueError(f"config already exists at {cfg_path} — pass --force to overwrite")

        resolved_vault = Path(path).expanduser() if path else _DEFAULT_VAULT_PATH
        resolved_agent = agent or os.environ.get("SHARDS_AGENT") or _DEFAULT_AGENT
        roster = _parse_roster(collections)

        resolved_vault.mkdir(parents=True, exist_ok=True)
        content = render_config_toml(
            vault_path=resolved_vault,
            agent=resolved_agent,
            collections=roster,
            search_collection=search_collection,
            hybrid=hybrid,
            threshold=threshold,
        )
        atomic_write(cfg_path, content)

    _emit(
        ctx,
        {
            "path": str(cfg_path),
            "vault_path": str(resolved_vault),
            "agent": resolved_agent,
        },
        f"wrote config to {cfg_path}",
    )


# --------------------------------------------------------------------------- #
# daemon command group                                                        #
# --------------------------------------------------------------------------- #


daemon_app = typer.Typer(
    name="daemon",
    help="Supervise the warm socket daemon (start | stop | status).",
    no_args_is_help=True,
)


@daemon_app.command("start")
def daemon_start_command(ctx: typer.Context) -> None:
    """Launch the daemon in the background (idempotent; no second spawn)."""
    pid_path = default_pid_path()
    with cli_errors():
        running = daemon_running(pid_path)
        if running is not None:
            _emit(
                ctx,
                {"running": True, "started": False, "pid": running},
                f"daemon already running (pid {running})",
            )
            return
        pid = spawn_daemon()
        write_pid(pid_path, pid)
        _emit(ctx, {"running": True, "started": True, "pid": pid}, f"daemon started (pid {pid})")


@daemon_app.command("stop")
def daemon_stop_command(ctx: typer.Context) -> None:
    """Terminate the daemon and clear its socket + PID file (idempotent)."""
    pid_path = default_pid_path()
    with cli_errors():
        running = daemon_running(pid_path)
        if running is None:
            _remove(pid_path)  # clear a stale PID file if one lingers
            _emit(ctx, {"running": False, "stopped": False}, "daemon not running")
            return
        terminate_process(running)
        _remove(default_socket_path())
        _remove(pid_path)
        _emit(
            ctx,
            {"running": False, "stopped": True, "pid": running},
            f"daemon stopped (pid {running})",
        )


@daemon_app.command("status")
def daemon_status_command(ctx: typer.Context) -> None:
    """Report whether the daemon is running, its PID, and the socket path."""
    pid_path = default_pid_path()
    socket_path = default_socket_path()
    with cli_errors():
        running = daemon_running(pid_path)
        payload = {"running": running is not None, "pid": running, "socket": str(socket_path)}
        if running is not None:
            human = f"running (pid {running}) — socket {socket_path}"
        else:
            human = f"stopped — socket {socket_path}"
        _emit(ctx, payload, human)


# --------------------------------------------------------------------------- #
# shards status                                                                #
# --------------------------------------------------------------------------- #


def _agent_breakdown(views: list[TaskView]) -> dict[str, dict[str, int]]:
    """Reduce a task list to a per-agent ``{owns_open, claimed, stale_claims}`` map (R4).

    Board visibility, not a new store: every count is derived on the spot from
    the task list ``shards status`` already fetches (:meth:`DaemonClient.task_list`
    — warm-index served when the daemon is up, the identical on-disk walk when it
    is down, exactly like every other read here). An identity is registered as
    soon as it appears anywhere — as ``owner`` *or* ``claimed_by``, on a task of
    *any* status — so an agent who currently holds nothing still gets a row of
    zeros rather than vanishing from the report; only ``owner``-on-an-``open``-task
    and ``claimed_by``-on-a-``claimed``-task actually increment a counter.

    * ``owns_open`` — tasks this agent owns that are still ``open`` (not yet
      claimed by anyone).
    * ``claimed`` — tasks this agent currently holds (``status == "claimed"``).
    * ``stale_claims`` — the subset of ``claimed`` whose ``updated`` is older
      than :data:`_STATUS_STALE_WINDOW` — an abandoned-looking claim, the exact
      thing ``--stale`` is for.
    """
    stale_cutoff = _parse_since(_STATUS_STALE_WINDOW)
    agents: dict[str, dict[str, int]] = {}

    def _register(identity: str) -> dict[str, int]:
        return agents.setdefault(identity, {"owns_open": 0, "claimed": 0, "stale_claims": 0})

    for view in views:
        task = view.task
        if task.owner:
            counts = _register(task.owner)
            if task.status == "open":
                counts["owns_open"] += 1
        if task.claimed_by:
            counts = _register(task.claimed_by)
            if task.status == "claimed":
                counts["claimed"] += 1
                if task.updated < stale_cutoff:
                    counts["stale_claims"] += 1

    return agents


def status_command(ctx: typer.Context) -> None:
    """Report vault health (counts, freshness, dangling links, stale locks, team state).

    Counts and freshness come from the daemon's warm index when it is up and from
    a direct scan when it is down — identical payload either way, so this works
    with the daemon down. The daemon's *liveness* line is still read from its PID
    file (no socket dependency), so a daemon that is running but unresponsive is
    reported as running. ``agents`` (team-awareness/4) is a per-identity
    ``owns_open`` / ``claimed`` / ``stale_claims`` breakdown — see
    :func:`_agent_breakdown` for exactly what each count measures. Human-only,
    like the rest of this command: not exposed over MCP (root AGENTS.md keeps
    ``status`` out of that surface, and the tool list is asserted against it).
    """
    config = load_config()
    with cli_errors():
        report = DaemonClient().vault_status(config)
        running = daemon_running(default_pid_path())
        report["daemon"] = {"running": running is not None, "pid": running}
        report["agents"] = _agent_breakdown(DaemonClient().task_list(config, limit=None))

    if _output.is_json(ctx):
        typer.echo(json.dumps(report))
        return

    typer.echo("\n".join(_status_lines(report)))


def _status_lines(report: dict[str, Any]) -> list[str]:
    """Render the human-readable ``shards status`` block."""
    tasks = report["tasks"]
    task_line = " ".join(f"{status}={tasks[status]}" for status in TASK_STATUSES)
    fresh = report["freshness"]["age_seconds"]
    freshness = f"{fresh:.1f}s ago" if fresh is not None else "(no vault files)"
    dangling = report["dangling_links"]
    daemon = report["daemon"]
    daemon_line = f"running (pid {daemon['pid']})" if daemon["running"] else "stopped"
    lines = [
        f"notes: {report['notes']}",
        f"tasks: {task_line}",
        f"freshness: {freshness}",
        f"dangling links: {len(dangling)}" + (f" ({', '.join(dangling)})" if dangling else ""),
        f"stale locks: {len(report['stale_locks'])}",
        f"daemon: {daemon_line}",
    ]
    agents = report.get("agents") or {}
    if not agents:
        lines.append("agents: (none)")
    else:
        lines.append("agents:")
        for identity in sorted(agents):
            counts = agents[identity]
            lines.append(
                f"  {identity}: open={counts['owns_open']} "
                f"claimed={counts['claimed']} stale={counts['stale_claims']}"
            )
    return lines


# --------------------------------------------------------------------------- #
# shards reindex                                                               #
# --------------------------------------------------------------------------- #


def reindex_command(ctx: typer.Context) -> None:
    """Rebuild the search index (delegates to ``indexed``; degrades if it's absent)."""
    config = load_config()
    from shards.index import indexed_client

    with cli_errors():
        try:
            indexed_client.reindex(config)
        except (FileNotFoundError, subprocess.CalledProcessError):
            # ``indexed`` binary is missing or exited non-zero: the search
            # accelerator is optional, so degrade with a notice rather than
            # crashing the CLI.
            _notice(ctx, "search index unavailable (indexed binary missing or failed)")
