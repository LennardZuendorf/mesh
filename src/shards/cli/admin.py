"""Admin surface: ``daemon`` lifecycle, ``shards status``, ``shards reindex``.

These are *admin* commands (spec R4), not new verbs — the three-verb rule
(``note`` / ``task`` / ``search``) is untouched. Everything here honours the
daemon's founding rule: it is an **accelerator, never a gate**. ``shards status``
computes vault health from a *direct filesystem scan* and ``reindex`` degrades to
a no-op notice, so both work with the daemon down.

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
  rule from :mod:`shards.storage.locks`). Strictly read-only — it never bumps
  ``updated`` nor rewrites a file.
* **``shards reindex``** — delegates to the search feature's ``indexed`` client.
  That module is unbuilt, so this degrades gracefully: one stderr notice, exit 0,
  suppressed under ``--quiet``. Wiring in the real delegate later is a one-liner.

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
import time
from pathlib import Path
from typing import Any

import typer

from shards.core.notes import list_notes
from shards.core.tasks import list_tasks
from shards.core.wikilinks import find_dangling
from shards.daemon.client import default_socket_path
from shards.index.watch import scan_recent
from shards.schemas.config import Config, load_config
from shards.storage.files import atomic_write

# Reuse the canonical liveness + staleness rule rather than re-deriving it: a lock
# is stale iff its PID is dead OR its age exceeds LOCK_TTL_SECONDS (300 s).
from shards.storage.locks import _is_stale, _pid_alive

_PID_NAME = "shards.pid"
_TASK_STATUSES: tuple[str, ...] = ("open", "claimed", "done", "cancelled")


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
# Vault health (direct scan — daemon-independent)                             #
# --------------------------------------------------------------------------- #


def scan_stale_locks(config: Config) -> list[Path]:
    """Every stale ``O_EXCL`` lock under ``notes/.locks`` and ``tasks/.locks``.

    A lock is stale iff its PID is dead **or** its age exceeds the 300 s TTL — the
    exact rule enforced on acquire (:func:`shards.storage.locks._is_stale`), reused
    here so ``shards status`` and the locker never disagree.
    """
    vault = config.core.tolaria_path
    stale: list[Path] = []
    for kind in ("notes", "tasks"):
        locks = vault / kind / ".locks"
        if not locks.is_dir():
            continue
        for lock in sorted(locks.glob("*.lock")):
            if _is_stale(lock):
                stale.append(lock)
    return stale


def vault_status(config: Config) -> dict[str, Any]:
    """Compute vault health by direct scan (no daemon required).

    Returns note count, tasks-by-status (zero-filled for a stable shape),
    freshness (newest shards-file mtime + its age in seconds), dangling wikilink
    targets, and stale lock paths. Strictly read-only: nothing here writes.
    """
    notes = list_notes(config, limit=None)
    tasks = list_tasks(config, limit=None)

    task_counts = dict.fromkeys(_TASK_STATUSES, 0)
    for view in tasks:
        status = view.task.status
        task_counts[status] = task_counts.get(status, 0) + 1

    recent = scan_recent(config, 1)
    mtime: float | None = None
    age: float | None = None
    if recent:
        mtime = float(recent[0]["mtime"])
        age = max(0.0, time.time() - mtime)

    return {
        "notes": len(notes),
        "tasks": task_counts,
        "tasks_total": len(tasks),
        "freshness": {"mtime": mtime, "age_seconds": age},
        "dangling_links": find_dangling(config.core.tolaria_path),
        "stale_locks": [str(p) for p in scan_stale_locks(config)],
    }


# --------------------------------------------------------------------------- #
# Output helpers                                                              #
# --------------------------------------------------------------------------- #


def _json(ctx: typer.Context) -> bool:
    return bool(getattr(ctx.obj, "json", False))


def _quiet(ctx: typer.Context) -> bool:
    return bool(getattr(ctx.obj, "quiet", False))


def _notice(ctx: typer.Context, message: str) -> None:
    """Emit an infrastructure notice on stderr (suppressed under ``--quiet``)."""
    if not _quiet(ctx):
        typer.echo(message, err=True)


def _emit(ctx: typer.Context, payload: dict[str, Any], human: str) -> None:
    """Machine JSON on ``--json``; a terse human line otherwise (silent if quiet)."""
    if _json(ctx):
        typer.echo(json.dumps(payload))
    elif not _quiet(ctx):
        typer.echo(human)


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
    running = daemon_running(pid_path)
    if running is None:
        _remove(pid_path)  # clear a stale PID file if one lingers
        _emit(ctx, {"running": False, "stopped": False}, "daemon not running")
        return
    terminate_process(running)
    _remove(default_socket_path())
    _remove(pid_path)
    _emit(ctx, {"running": False, "stopped": True, "pid": running}, f"daemon stopped (pid {running})")


@daemon_app.command("status")
def daemon_status_command(ctx: typer.Context) -> None:
    """Report whether the daemon is running, its PID, and the socket path."""
    pid_path = default_pid_path()
    socket_path = default_socket_path()
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


def status_command(ctx: typer.Context) -> None:
    """Report vault health (counts, freshness, dangling links, stale locks).

    Computed by direct scan, so it works with the daemon down; the daemon's
    liveness is reported from its PID file (no socket dependency).
    """
    config = load_config()
    report = vault_status(config)
    running = daemon_running(default_pid_path())
    report["daemon"] = {"running": running is not None, "pid": running}

    if _json(ctx):
        typer.echo(json.dumps(report))
        return

    typer.echo("\n".join(_status_lines(report)))


def _status_lines(report: dict[str, Any]) -> list[str]:
    """Render the human-readable ``shards status`` block."""
    tasks = report["tasks"]
    task_line = " ".join(f"{status}={tasks[status]}" for status in _TASK_STATUSES)
    fresh = report["freshness"]["age_seconds"]
    freshness = f"{fresh:.1f}s ago" if fresh is not None else "(no vault files)"
    dangling = report["dangling_links"]
    daemon = report["daemon"]
    daemon_line = f"running (pid {daemon['pid']})" if daemon["running"] else "stopped"
    return [
        f"notes: {report['notes']}",
        f"tasks: {task_line}",
        f"freshness: {freshness}",
        f"dangling links: {len(dangling)}" + (f" ({', '.join(dangling)})" if dangling else ""),
        f"stale locks: {len(report['stale_locks'])}",
        f"daemon: {daemon_line}",
    ]


# --------------------------------------------------------------------------- #
# shards reindex                                                               #
# --------------------------------------------------------------------------- #


def reindex_command(ctx: typer.Context) -> None:
    """Rebuild the search index (delegates to ``indexed``; degrades if it's absent)."""
    config = load_config()
    from shards.index import indexed_client

    try:
        indexed_client.reindex(config)
    except (FileNotFoundError, subprocess.CalledProcessError):
        # ``indexed`` binary is missing or exited non-zero: the search accelerator is
        # optional, so degrade with a notice rather than crashing the CLI.
        _notice(ctx, "search index unavailable (indexed binary missing or failed)")
