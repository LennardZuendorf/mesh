"""Phase-2 session lenses — ``mesh recent-activity`` + ``mesh build-context``.

Both are *leaf* commands (like ``status`` / ``reindex``), not fourth verbs: they
surface shared read-only lenses over the one Markdown folder. They stay honest to
the design language — machine JSON under ``--json``, terse text for humans,
IDs-only under ``--quiet``, and every infrastructure notice on **stderr**, never
in the JSON payload.

``recent-activity`` delegates to :func:`mesh.core.activity.recent_activity`.
The daemon is an accelerator: when it is down the underlying lens transparently
scans the folder instead, and this command emits a single informational stderr
line saying so (suppressed by ``--quiet``). The scan is *equivalent* to the warm
index — not degraded — so the notice is a heads-up, not a warning.

``build-context`` delegates to :func:`mesh.core.context.build_context`, a BFS
over the ``related`` id graph. It is *daemon-independent* (every node is read
straight off disk), so unlike ``recent-activity`` it has no degradation path and
emits **no** infrastructure notice; an unresolvable seed exits 3.

Cross-cutting flags (``--json`` / ``--quiet`` / ``--owner`` / ``--mine``) are
accepted both here and on the root callback; the two are coalesced (R6, root
tech.md § Surface C) so ``mesh --mine recent-activity`` and
``mesh recent-activity --mine`` behave identically. ``--json``/``--quiet``/
``--owner`` route through the one shared :func:`mesh.cli._output.coalesce_flags`
— the same function ``note``/``task``/``search`` call — rather than a private
per-module copy; ``--mine`` stays a direct inline OR here since it has no
counterpart on those other verbs.

``session-start`` (memory/4; widened by team-awareness/7) is the warm-start
composite: it merges the caller's live ``open``/``claimed`` task queue, inbound
mentions of the caller's nodes, and a ``recent_activity(7d)`` window,
de-duplicates by id, and orders *tasks, then mentions, then the remaining
activity* newest-first — the payload the ``SessionStart`` hook feeds a fresh
agent session. ``--owner <agent>`` (honoured on both sides of the command name,
like every other cross-cutting flag here) swaps the effective identity for
*both* halves — "what would flights-agent's warm start show" — by building a
:class:`~mesh.schemas.config.Config` with that agent substituted for
``[core].agent`` and driving every source off it, rather than adding a second
identity parameter to each source. ``--team`` drops the identity filter on the
activity half *only*; the task half always stays the effective agent's own
open/claimed queue — widening never means "show me everyone's todo list."

``graph`` (cli-toolset-rework/3) delegates to :func:`mesh.core.context.graph_query`
— the same daemon-free BFS ``build-context`` performs, promoted to a first-class
"what's connected to X" query. ``--json`` emits ``{seed, nodes, edges}``; the
default text is a readable indented tree; ``--quiet`` is ids only. Like
``build-context`` it never touches the daemon or hybrid search, so it has no
degradation notice. ``--direction out|in|both`` (team-awareness/1) selects
which way the BFS walks ``related``: ``in`` inverts it at read time to surface
backlinks (mentions) — the primitive that makes ``@agent — [[t-184G]]`` in
someone else's note deliverable from ``t-184G`` itself.
"""

from __future__ import annotations

import json
from typing import Any

import typer

from mesh.cli import _output
from mesh.cli._errors import cli_errors
from mesh.core.lenses import (
    SESSION_SINCE,
    as_effective_agent,
    build_context,
    graph_query,
    project_view,
    recent_activity,
    session_mentions,
    session_start_entries,
)
from mesh.daemon.client import DaemonClient
from mesh.index.warm import DEFAULT_RECENT_LIMIT
from mesh.schemas.config import load_config

_DAEMON_DOWN_NOTICE = "recent-activity: daemon down, scanning the folder directly"


def _daemon_up() -> bool:
    """Whether the warm daemon answers a ping (drives the informational notice).

    Kept as a module-level seam so tests can fake daemon liveness without a socket.
    """
    return DaemonClient().is_up()


def _identity_columns(entry: dict[str, Any]) -> tuple[str, str]:
    """``(owner, claimed_by)`` text-row columns for an activity/task/mention row.

    ``"-"`` stands in for an absent value — the exact fallback :func:`task
    list <mesh.cli.task.list_command>` established (``35f7301``) for
    ``claimed_by`` in its own text rows; this is that same one convention,
    reused rather than re-invented, now covering ``owner`` too. Every row this
    module renders already carries both keys as of team-awareness/6/7 (activity
    rows, task dumps, and resolved mention entries alike), so this is a pure
    format, never a fallback disk read.
    """
    owner = entry.get("owner")
    claimed_by = entry.get("claimed_by")
    return (str(owner) if owner else "-", str(claimed_by) if claimed_by else "-")


def _recent_activity_row(entry: dict[str, Any]) -> str:
    """``recent-activity``'s text row: id / type / owner / claimed_by / title / path."""
    owner_col, claimed_col = _identity_columns(entry)
    return (
        f"{entry.get('id', '')}\t{entry.get('type', '')}\t{owner_col}\t{claimed_col}\t"
        f"{entry.get('title', '')}\t{entry.get('path', '')}"
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
    # on either side of the command name takes effect. ``emit_entries`` below reads
    # the merged json/quiet straight off ``ctx.obj``, so only ``quiet`` (the notice
    # gate) and ``owner`` are needed as locals here.
    _json_out, quiet, owner = _output.coalesce_flags(
        ctx, json_out=json_out, quiet=quiet, owner=owner
    )
    mine = mine or bool(getattr(ctx.obj, "mine", False))

    with cli_errors():
        entries = recent_activity(config, since=since, owner=owner, mine=mine, limit=limit)

    # Informational notice *after* a successful fetch, so a bad --since never emits
    # a spurious "daemon down" line before its exit-2.
    if not quiet and not _daemon_up():
        typer.echo(_DAEMON_DOWN_NOTICE, err=True)

    _output.emit_entries(ctx, entries, _recent_activity_row)


def session_start_command(
    ctx: typer.Context,
    owner: str | None = typer.Option(
        None, "--owner", help="Show this agent's warm start instead of mine."
    ),
    team: bool = typer.Option(
        False, "--team", help="Widen the activity half to every agent (task half stays mine)."
    ),
    meta_only: bool = typer.Option(
        False, "--meta-only", help="Omit note/task bodies (token-budget path)."
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON array."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only, one per line."),
) -> None:
    """Warm-start payload: my open/claimed tasks + mentions of me + recent activity (R4, R7).

    Composes three read-only lenses — ``list_tasks(mine, open|claimed)``,
    :func:`~mesh.core.lenses.session_mentions` (inbound links to nodes I own or
    have claimed), and ``recent_activity(since=7d)`` — de-duplicates by id, and
    orders the result *tasks, then mentions, then remaining activity*
    newest-first. ``--meta-only`` drops bodies for the token budget; the
    ``SessionStart`` hook invokes ``session-start --meta-only --json`` and this
    stays valid and body-free under it (mentions and activity rows never carry a
    body regardless). Every lens here is daemon-independent (or degrades
    transparently), so the payload is produced identically with the daemon down;
    no infrastructure notice is emitted from this composite path.

    ``--owner <agent>`` (honoured on both sides of the command name — coalesced
    with the root callback's global ``--owner``, like every other cross-cutting
    flag) substitutes ``agent`` for the effective identity on *every* source —
    "what would that agent's warm start show" — via :func:`~mesh.core.lenses.as_effective_agent`.
    ``--team`` drops the identity filter on the activity half only; the task
    half (and the mentions target set, which is built from that same task
    queue) always stays the effective agent's own.
    """
    config = load_config()

    # Coalesce the leaf flags with the root callback's global flags so a flag on
    # either side of the command name takes effect. ``emit_entries`` below reads
    # the merged json/quiet straight off ``ctx.obj``.
    _json_out, _quiet, owner = _output.coalesce_flags(
        ctx, json_out=json_out, quiet=quiet, owner=owner
    )
    effective_config = as_effective_agent(config, owner)
    me = effective_config.agent

    with cli_errors():
        # Source A — the effective agent's live queue: every task they own or
        # have claimed, any status (session_mentions needs the unfiltered set
        # too — see below — so the status narrowing to open/claimed happens
        # once, inside the compose step).
        task_views = DaemonClient().task_list(effective_config, mine=True, limit=None)
        # Every note the effective agent owns — the note half of "my nodes" for
        # the mentions target set (a task has claimed_by too; a note does not).
        # ``owner=None`` means *unfiltered* to note_list/select_notes — unlike
        # task_list's ``mine``, which correctly degrades to empty against a
        # ``None`` me (core/tasks.py's ``select_tasks`` treats an unset ``spec.me``
        # as matching nothing) — so with no configured identity this must skip
        # the fetch here too, not pass ``owner=None`` through and silently claim
        # every note in the vault as "mine".
        note_views = DaemonClient().note_list(effective_config, owner=me, limit=None) if me else []
        # Source B — inbound mentions of my nodes (team-awareness/7): one
        # whole-vault reverse-``related`` pass, reused across every target in
        # task_views/note_views rather than walked once per target.
        mentions = session_mentions(
            effective_config, task_views, note_views, me=me, since=SESSION_SINCE
        )
        # Source C — recent changes. --team drops the identity filter here only;
        # the task queue above (and the mentions target set built from it) never
        # widens — dedup happens in the compose step below.
        activity = recent_activity(
            effective_config,
            since=SESSION_SINCE,
            owner=None,
            mine=not team,
            limit=DEFAULT_RECENT_LIMIT,
        )
        # Compose the warm-start payload: open/claimed tasks, then mentions,
        # then the remaining activity newest-first — deduped by id throughout.
        entries = session_start_entries(task_views, activity, mentions, meta_only=meta_only)

    _output.emit_entries(ctx, entries, _session_start_row)


def _session_start_row(entry: dict[str, Any]) -> str:
    """``session-start``'s text row: id / type / reason / owner / claimed_by / title / path."""
    owner_col, claimed_col = _identity_columns(entry)
    return (
        f"{entry.get('id', '')}\t{entry.get('type', '')}\t{entry.get('reason', '')}\t"
        f"{owner_col}\t{claimed_col}\t{entry.get('title', '')}\t{entry.get('path', '')}"
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
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)

    with cli_errors():
        entries = build_context(config, seed_id, depth=depth)

    _output.emit_entries(ctx, entries, _build_context_row)


def _build_context_row(entry: dict[str, Any]) -> str:
    """``build-context``'s text row: id / type / title / path."""
    return (
        f"{entry.get('id', '')}\t{entry.get('type', '')}\t"
        f"{entry.get('title', '')}\t{entry.get('path', '')}"
    )


def graph_command(
    ctx: typer.Context,
    seed_id: str = typer.Argument(..., help="Seed note/task id (n-… or t-…) to expand from."),
    depth: int = typer.Option(1, "--depth", help="Hops to walk (0 = seed only; 1 = direct)."),
    direction: str = typer.Option(
        "out",
        "--direction",
        help="Edge direction to walk: out (related, default), in (backlinks), both.",
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable {seed, nodes, edges}."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only, one per line."),
) -> None:
    """Query what's connected to a seed id: readable tree, or JSON nodes+edges.

    ``--direction in`` inverts ``related`` at read time — every node whose own
    ``related`` names the seed, i.e. every mention of it — so a reply that links
    ``[[t-184G]]`` becomes visible *from* ``t-184G`` even though nothing was
    ever written to ``t-184G`` itself (team-awareness/1). ``--direction both``
    is the union, each node and edge reported once.
    """
    config = load_config()

    # Coalesce the leaf flags with the root callback's global flags so a flag given
    # on either side of the command name takes effect. Same daemon-free traversal
    # as build-context, so there is no degradation notice.
    json_out, quiet, _owner = _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)

    with cli_errors():
        result = graph_query(config, seed_id, depth=depth, direction=direction)

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

    Delegates to :func:`mesh.core.lenses.project_view`: the project note plus
    every task whose ``project`` soft link matches. ``--json`` emits
    ``{project, tasks}``; the default text is the project row then its task rows;
    ``--quiet`` is ids only (project id first). Daemon-free — every node is read
    off disk — so there is no degradation notice; an unresolvable project exits 3.
    """
    config = load_config()

    # Coalesce the leaf flags with the root callback's global flags so a flag given
    # on either side of the command name takes effect.
    json_out, quiet, _owner = _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)

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
