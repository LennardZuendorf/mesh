"""Phase-2 session lenses — ``shards recent-activity`` + ``shards build-context``.

Both are *leaf* commands (like ``status`` / ``reindex``), not fourth verbs: they
surface shared read-only lenses over the one Markdown folder. They stay honest to
the design language — machine JSON under ``--json``, terse text for humans,
IDs-only under ``--quiet``, and every infrastructure notice on **stderr**, never
in the JSON payload.

``recent-activity`` delegates to :func:`shards.core.activity.recent_activity`.
The daemon is an accelerator: when it is down the underlying lens transparently
scans the folder instead, and this command emits a single informational stderr
line saying so (suppressed by ``--quiet``). The scan is *equivalent* to the warm
index — not degraded — so the notice is a heads-up, not a warning.

``build-context`` delegates to :func:`shards.core.context.build_context`, a BFS
over the ``related`` id graph. It is *daemon-independent* (every node is read
straight off disk), so unlike ``recent-activity`` it has no degradation path and
emits **no** infrastructure notice; an unresolvable seed exits 3.

Cross-cutting flags (``--json`` / ``--quiet`` / ``--owner`` / ``--mine``) are
accepted both here and on the root callback; the two are coalesced so
``shards --mine recent-activity`` and ``shards recent-activity --mine`` behave
identically.

``session-start`` (memory/4) is the warm-start composite: it merges the
``recent_activity(7d, mine)`` window with the caller's live ``open``/``claimed``
task queue, de-duplicates by id, and orders *tasks first* then the remaining
activity newest-first — the payload the ``SessionStart`` hook feeds a fresh
agent session.
"""

from __future__ import annotations

import json

import typer

from shards.core.lenses import (
    SeedNotFoundError,
    build_context,
    recent_activity,
    session_start_entries,
)
from shards.core.tasks import list_tasks
from shards.daemon.client import DaemonClient
from shards.index.warm import DEFAULT_RECENT_LIMIT
from shards.schemas.config import load_config

_DAEMON_DOWN_NOTICE = "recent-activity: daemon down, scanning the folder directly"
_SESSION_SINCE = "7d"


def _daemon_up() -> bool:
    """Whether the warm daemon answers a ping (drives the informational notice).

    Kept as a module-level seam so tests can fake daemon liveness without a socket.
    """
    return DaemonClient().is_up()


def recent_activity_command(
    ctx: typer.Context,
    since: str | None = typer.Option(None, "--since", help="Recency window: 7d, 12h, or ISO."),
    owner: str | None = typer.Option(None, "--owner", help="Filter by exact owner."),
    mine: bool = typer.Option(False, "--mine", help="Filter to owner or claimed_by == me."),
    limit: int = typer.Option(DEFAULT_RECENT_LIMIT, "--limit", help="Cap the number of rows."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON array."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """List recent vault changes (newest first), filtered by ``--since`` / owner."""
    config = load_config()

    # Coalesce the leaf flags with the root callback's global flags so a flag given
    # on either side of the command name takes effect.
    json_out = json_out or bool(getattr(ctx.obj, "json", False))
    quiet = quiet or bool(getattr(ctx.obj, "quiet", False))
    owner = owner if owner is not None else getattr(ctx.obj, "owner", None)
    mine = mine or bool(getattr(ctx.obj, "mine", False))

    try:
        entries = recent_activity(config, since=since, owner=owner, mine=mine, limit=limit)
    except ValueError:
        if not quiet:
            typer.echo(f"recent-activity: invalid --since value {since!r}", err=True)
        raise typer.Exit(2) from None

    # Informational notice *after* a successful fetch, so a bad --since never emits
    # a spurious "daemon down" line before its exit-2.
    if not quiet and not _daemon_up():
        typer.echo(_DAEMON_DOWN_NOTICE, err=True)

    if json_out:
        typer.echo(json.dumps(entries))
        return
    if quiet:
        for entry in entries:
            typer.echo(str(entry.get("id", "")))
        return
    for entry in entries:
        typer.echo(
            f"{entry.get('id', '')}\t{entry.get('type', '')}\t"
            f"{entry.get('title', '')}\t{entry.get('path', '')}"
        )


def session_start_command(
    ctx: typer.Context,
    meta_only: bool = typer.Option(
        False, "--meta-only", help="Omit note/task bodies (token-budget path)."
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON array."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only, one per line."),
) -> None:
    """Warm-start payload: my recent activity (7d) + my open/claimed tasks (R4).

    Composes two read-only lenses — ``recent_activity(since=7d, mine)`` and
    ``list_tasks(mine, open|claimed)`` — de-duplicates by id, and orders the
    result *tasks first* (what I still owe) then the remaining activity entries
    newest-first. ``--meta-only`` drops bodies for the token budget; the
    ``SessionStart`` hook invokes ``session-start --meta-only --json``. Both
    lenses are daemon-independent (or degrade transparently), so the payload is
    produced with the daemon down; no infrastructure notice is emitted from this
    composite path.
    """
    config = load_config()

    # Coalesce the leaf flags with the root callback's global flags so a flag on
    # either side of the command name takes effect.
    json_out = json_out or bool(getattr(ctx.obj, "json", False))
    quiet = quiet or bool(getattr(ctx.obj, "quiet", False))

    # Source A — my live queue: every open/claimed task I own or have claimed.
    task_views = list_tasks(config, mine=True, limit=None)
    # Source B — my recent changes (dedup by id happens in the compose step below).
    activity = recent_activity(
        config, since=_SESSION_SINCE, owner=None, mine=True, limit=DEFAULT_RECENT_LIMIT
    )
    # Compose the warm-start payload: open/claimed tasks first (deduped by id),
    # then the remaining activity newest-first.
    entries = session_start_entries(task_views, activity, meta_only=meta_only)

    if json_out:
        typer.echo(json.dumps(entries))
        return
    if quiet:
        for entry in entries:
            typer.echo(str(entry.get("id", "")))
        return
    for entry in entries:
        typer.echo(
            f"{entry.get('id', '')}\t{entry.get('type', '')}\t"
            f"{entry.get('title', '')}\t{entry.get('path', '')}"
        )


def build_context_command(
    ctx: typer.Context,
    seed_id: str = typer.Argument(..., help="Seed note/task id (n-… or t-…) to expand from."),
    depth: int = typer.Option(1, "--depth", help="Hops to walk (0 = seed only; 1 = direct)."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON array."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only, one per line."),
) -> None:
    """Expand the ``related`` graph around a seed id (BFS to ``--depth``, seed first)."""
    config = load_config()

    # Coalesce the leaf flags with the root callback's global flags so a flag given
    # on either side of the command name takes effect. This lens is daemon-free —
    # every node is read off disk — so there is no degradation notice.
    json_out = json_out or bool(getattr(ctx.obj, "json", False))
    quiet = quiet or bool(getattr(ctx.obj, "quiet", False))

    try:
        entries = build_context(config, seed_id, depth=depth)
    except SeedNotFoundError:
        if not quiet:
            typer.echo(f"build-context: seed not found: {seed_id}", err=True)
        raise typer.Exit(3) from None

    if json_out:
        typer.echo(json.dumps(entries))
        return
    if quiet:
        for entry in entries:
            typer.echo(str(entry.get("id", "")))
        return
    for entry in entries:
        typer.echo(
            f"{entry.get('id', '')}\t{entry.get('type', '')}\t"
            f"{entry.get('title', '')}\t{entry.get('path', '')}"
        )
