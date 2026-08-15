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

``graph`` (cli-toolset-rework/3) delegates to :func:`shards.core.context.graph_query`
— the same daemon-free BFS ``build-context`` performs, promoted to a first-class
"what's connected to X" query. ``--json`` emits ``{seed, nodes, edges}``; the
default text is a readable indented tree; ``--quiet`` is ids only. Like
``build-context`` it never touches the daemon or hybrid search, so it has no
degradation notice.
"""

from __future__ import annotations

import json

import typer

from shards.cli._errors import cli_errors
from shards.core.lenses import (
    build_context,
    graph_query,
    project_view,
    recent_activity,
    session_start_entries,
)
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


def _coalesce(ctx: typer.Context, json_out: bool, quiet: bool) -> tuple[bool, bool]:
    """Coalesce the leaf ``--json``/``--quiet`` flags with the root callback's globals.

    A flag given on either side of the command name takes effect — shared by
    every lens command below.
    """
    return (
        json_out or bool(getattr(ctx.obj, "json", False)),
        quiet or bool(getattr(ctx.obj, "quiet", False)),
    )


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
    json_out, quiet = _coalesce(ctx, json_out, quiet)
    owner = owner if owner is not None else getattr(ctx.obj, "owner", None)
    mine = mine or bool(getattr(ctx.obj, "mine", False))

    with cli_errors():
        entries = recent_activity(config, since=since, owner=owner, mine=mine, limit=limit)

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
    json_out, quiet = _coalesce(ctx, json_out, quiet)

    with cli_errors():
        # Source A — my live queue: every open/claimed task I own or have claimed.
        # Warm-index served when the daemon is up, disk-walked when it is down.
        task_views = DaemonClient().task_list(config, mine=True, limit=None)
        # Source B — my recent changes (dedup happens in the compose step below).
        activity = recent_activity(
            config, since=_SESSION_SINCE, owner=None, mine=True, limit=DEFAULT_RECENT_LIMIT
        )
        # Compose the warm-start payload: open/claimed tasks first (deduped by
        # id), then the remaining activity newest-first.
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
    json_out, quiet = _coalesce(ctx, json_out, quiet)

    with cli_errors():
        entries = build_context(config, seed_id, depth=depth)

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


def graph_command(
    ctx: typer.Context,
    seed_id: str = typer.Argument(..., help="Seed note/task id (n-… or t-…) to expand from."),
    depth: int = typer.Option(1, "--depth", help="Hops to walk (0 = seed only; 1 = direct)."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable {seed, nodes, edges}."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only, one per line."),
) -> None:
    """Query what's connected to a seed id: readable tree, or JSON nodes+edges."""
    config = load_config()

    # Coalesce the leaf flags with the root callback's global flags so a flag given
    # on either side of the command name takes effect. Same daemon-free traversal
    # as build-context, so there is no degradation notice.
    json_out, quiet = _coalesce(ctx, json_out, quiet)

    with cli_errors():
        result = graph_query(config, seed_id, depth=depth)

    # Both branches below render the one already-computed `result` — no second
    # traversal for JSON vs. tree output.
    if json_out:
        typer.echo(json.dumps(result.to_dict()))
        return
    if quiet:
        for entry_id in result.ids:
            typer.echo(entry_id)
        return
    for line in result.tree_lines():
        typer.echo(line)


def project_command(
    ctx: typer.Context,
    project_id: str = typer.Argument(..., help="Project note id (n-…) to scope to."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable {project, tasks}."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only (project then tasks)."),
) -> None:
    """Show a project note and the tasks scoped to it — a read-only lens, not a verb.

    Delegates to :func:`shards.core.lenses.project_view`: the project note plus
    every task whose ``project`` soft link matches. ``--json`` emits
    ``{project, tasks}``; the default text is the project row then its task rows;
    ``--quiet`` is ids only (project id first). Daemon-free — every node is read
    off disk — so there is no degradation notice; an unresolvable project exits 3.
    """
    config = load_config()

    # Coalesce the leaf flags with the root callback's global flags so a flag given
    # on either side of the command name takes effect.
    json_out, quiet = _coalesce(ctx, json_out, quiet)

    with cli_errors():
        result = project_view(config, project_id)

    if json_out:
        typer.echo(json.dumps(result.to_dict()))
        return
    if quiet:
        typer.echo(str(result.project.get("id", "")))
        for task in result.tasks:
            typer.echo(str(task.get("id", "")))
        return
    project = result.project
    typer.echo(f"{project.get('id', '')}\t{project.get('type', '')}\t{project.get('title', '')}")
    for task in result.tasks:
        typer.echo(f"  {task.get('id', '')}\t{task.get('status', '')}\t{task.get('title', '')}")
