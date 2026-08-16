"""FastMCP server — the agent-facing ``shards_*`` tool surface (memory/1).

This is the MCP mirror of the *safe* CLI verbs. It is deliberately thin: every
tool loads the config, routes straight through the existing ``core`` domain logic
(and ``daemon/client.py`` for the list-shaped reads, the recent-activity lens and
search), and returns a JSON-serialisable dict. There is **no** parallel
re-implementation of note, task, or search behaviour — the tools call the same
functions the CLI does, including the same warm-index-or-fallback client verbs, so
the two surfaces cannot drift.

Unlike the CLI, MCP parameters are *typed fields*, not flag strings: ``shards_note_get``
takes ``id: str``, not ``--id <id>``. Each tool carries an MCP annotation
declaring its effect, so an agent runtime can reason about safety before calling:

* **read-only** (``readOnlyHint``) — ``shards_note_get`` / ``shards_note_list`` /
  ``shards_task_get`` / ``shards_task_list`` / ``shards_search`` /
  ``shards_recent_activity`` / ``shards_build_context`` / ``shards_graph`` /
  ``shards_project`` / ``shards_session_start`` (the warm-start composite —
  team-awareness/10 — reads three lenses and writes nothing);
* **idempotent** (``idempotentHint``) — ``shards_note_update`` / ``shards_task_claim`` /
  ``shards_task_finish`` / ``shards_task_update`` / ``shards_task_release``
  (re-running lands the same state — a release re-applied to an already-open
  task is a no-op, mirroring ``shards_task_claim``'s same-agent no-op);
* **write** (no special hint) — ``shards_note_new`` / ``shards_note_append`` /
  ``shards_task_new`` / ``shards_task_append`` (each call is a genuine mutation,
  not idempotent replay — appending the same text twice appends it twice);
* **destructive** (``destructiveHint``) — ``shards_task_cancel`` (a one-way lifecycle move).

The unsafe / administrative surface is **withheld**: neither delete verb, no daemon
controls, no ``reindex`` or ``status``. Those never reach an agent through MCP.
``shards_task_release`` *does* ship here (team-awareness/10 — release is a shipped
verb, not graph work; see root ``AGENTS.md`` §6 and team-awareness/tech.md §
"MCP parity"), but its ``--force`` override does not: owner identity is trusted
local input, not an authorization boundary (root ``AGENTS.md`` §6), so breaking a
peer's claim stays a human/CLI action, never something an agent can trigger
through this surface.

Tool functions are defined as plain module-level callables and registered on the
app afterwards, so they stay directly importable and unit-testable while the app
introspection still reports the correct names, schemas, and annotations.
"""

from __future__ import annotations

from collections.abc import Callable
from functools import wraps
from typing import Any

from fastmcp import FastMCP
from fastmcp.exceptions import ToolError

from shards.core.errors import ShardsError
from shards.core.lenses import (
    SESSION_SINCE,
    as_effective_agent,
    build_context,
    graph_query,
    project_view,
    recent_activity,
    session_mentions,
    session_start_entries,
)
from shards.core.notes import (
    NoteView,
    append_note,
    create_note,
    get_note,
    update_note,
)
from shards.core.notes import (
    find_duplicate_title as find_duplicate_note_title,
)
from shards.core.search import hit_dict, query_search, resolve_effective_threshold
from shards.core.tasks import (
    TaskView,
    append_task,
    cancel_task,
    claim_task,
    create_task,
    finish_task,
    get_task,
    release_task,
    update_task,
)
from shards.core.tasks import (
    find_duplicate_title as find_duplicate_task_title,
)
from shards.daemon.client import DaemonClient
from shards.index.warm import DEFAULT_RECENT_LIMIT
from shards.schemas.config import load_config
from shards.schemas.note import Note

app = FastMCP("shards")


# --------------------------------------------------------------------------- #
# Serialisation helpers                                                       #
# --------------------------------------------------------------------------- #


def _entry(model: Note, *, body: str | None, path: str) -> dict[str, Any]:
    """Render a note/task view as a JSON-safe dict: frontmatter (+ body) + path."""
    data: dict[str, Any] = model.model_dump(mode="json")
    if body is not None:
        data["body"] = body
    data["path"] = str(path)
    return data


def _note_get_dict(view: NoteView) -> dict[str, Any]:
    return _entry(view.note, body=view.body, path=str(view.path))


def _task_get_dict(view: TaskView) -> dict[str, Any]:
    return _entry(view.task, body=view.body, path=str(view.path))


# --------------------------------------------------------------------------- #
# Read-only tools                                                             #
# --------------------------------------------------------------------------- #


def shards_note_get(id: str) -> dict[str, Any]:
    """Read one note by id or title slug: frontmatter, body, and path."""
    config = load_config()
    return _note_get_dict(get_note(config, id))


def shards_note_list(
    tags: list[str] | None = None,
    any_tag: bool = False,
    owner: str | None = None,
    note_type: str | None = None,
    since: str | None = None,
    sort: str = "updated",
    limit: int = 20,
) -> list[dict[str, Any]]:
    """List shards notes with tag/owner/type/recency filters, sorted and capped."""
    config = load_config()
    views = DaemonClient().note_list(
        config,
        tags=tags,
        any_tag=any_tag,
        owner=owner,
        note_type=note_type,
        since=since,
        sort=sort,
        limit=limit,
    )
    return [_entry(v.note, body=None, path=str(v.path)) for v in views]


def shards_task_get(id: str) -> dict[str, Any]:
    """Read one task by id: frontmatter, body, and path."""
    config = load_config()
    return _task_get_dict(get_task(config, id))


def shards_task_list(
    status: str | None = None,
    owner: str | None = None,
    mine: bool = False,
    tags: list[str] | None = None,
    any_tag: bool = False,
    project: str | None = None,
    since: str | None = None,
    stale: str | None = None,
    available: bool = False,
    sort: str | None = None,
    limit: int = 20,
) -> list[dict[str, Any]]:
    """List shards tasks (open and done) with status/owner/mine/project filters, sorted.

    ``status`` accepts a comma-separated set for a union filter (e.g.
    ``"open,claimed"`` — team-awareness/4), passed straight through to the same
    parser the CLI's ``--status`` uses. ``since`` is a recency floor (updated
    within the window); ``stale`` (team-awareness/5) is its inverse — a ceiling,
    "not touched within the window" — and the two are conjunctive when both are
    given. ``available`` narrows to takeable work: ``status == "open"`` and
    unclaimed. ``sort`` is ``updated`` (default) / ``created`` / ``title`` /
    ``priority``; when omitted, it defaults to ``priority`` under ``available``
    and ``updated`` otherwise — the same default ``cli/task.py``'s ``list``
    applies, so an agent asking for "what's available" gets it priority-ordered
    without asking for the sort explicitly.
    """
    config = load_config()
    sort_field = sort if sort is not None else ("priority" if available else "updated")
    views = DaemonClient().task_list(
        config,
        status=status,
        owner=owner,
        mine=mine,
        tags=tags,
        any_tag=any_tag,
        project=project,
        since=since,
        stale=stale,
        available=available,
        sort=sort_field,
        limit=limit,
    )
    return [_entry(v.task, body=None, path=str(v.path)) for v in views]


def shards_search(
    query: str | None = None,
    type_filter: str | None = None,
    tags: list[str] | None = None,
    owner: str | None = None,
    status: str | None = None,
    limit: int = 10,
    threshold: float | None = None,
    meta_only: bool = False,
    full: bool = False,
) -> list[dict[str, Any]]:
    """Recall across notes + tasks: tag pull (no query) or scored match (query)."""
    config = load_config()
    if query is None:
        results = DaemonClient().tag_pull(
            config,
            tags=tags,
            type_filter=type_filter,
            owner=owner,
            status=status,
            limit=limit,
        )
    else:
        # ``None`` propagates when neither the caller nor the config key set
        # threshold explicitly, so the substring fallback applies its own floor
        # rather than a silently-defaulted cutoff (root tech.md § B5).
        effective_threshold = resolve_effective_threshold(threshold, config)
        results = query_search(
            config,
            query,
            type_filter=type_filter,
            tags=tags,
            owner=owner,
            status=status,
            limit=limit,
            threshold=effective_threshold,
            quiet=True,
        )
    return [hit_dict(result, meta_only=meta_only, full=full) for result in results]


def shards_recent_activity(
    since: str | None = None,
    owner: str | None = None,
    mine: bool = False,
    limit: int = DEFAULT_RECENT_LIMIT,
) -> list[dict[str, Any]]:
    """Recent vault changes (newest first), each row carrying identity.

    ``{id, type, title, path, mtime, owner, claimed_by}`` — team-awareness/6.
    """
    config = load_config()
    return recent_activity(config, since=since, owner=owner, mine=mine, limit=limit)


def shards_build_context(seed_id: str, depth: int = 1) -> list[dict[str, Any]]:
    """Expand the ``related`` graph around a seed id (BFS to depth, seed first)."""
    config = load_config()
    return build_context(config, seed_id, depth=depth)


def shards_graph(seed_id: str, depth: int = 1, direction: str = "out") -> dict[str, Any]:
    """Query what's connected to a seed id: ``{seed, nodes, edges}`` (BFS to depth).

    ``direction`` is ``"out"`` (default, forward ``related``), ``"in"`` (who
    mentions this node — backlinks/notify), or ``"both"``.
    """
    config = load_config()
    return graph_query(config, seed_id, depth=depth, direction=direction).to_dict()


def shards_project(project_id: str) -> dict[str, Any]:
    """Show a project note and the tasks scoped to it: ``{project, tasks}``."""
    config = load_config()
    return project_view(config, project_id).to_dict()


def shards_session_start(
    owner: str | None = None,
    team: bool = False,
    meta_only: bool = False,
) -> list[dict[str, Any]]:
    """Warm-start payload: my open/claimed tasks + mentions of me + recent activity.

    The MCP mirror of ``shards session-start`` (team-awareness/10) — the highest-
    value tool in this parity sweep, since it is the only way an MCP-only agent
    (one with no CLI stdout to read) sees its own queue and the mentions
    delivered to it. Composes the same three read-only lenses the CLI command
    does — ``task_list(mine=True)``, :func:`~shards.core.lenses.session_mentions`
    (inbound links to nodes the caller owns or has claimed), and
    ``recent_activity(since=7d)`` — via the shared
    :func:`~shards.core.lenses.session_start_entries` composer, so the ordering
    (tasks, then mentions, then remaining activity, newest-first, deduped by id)
    and the ``reason`` key on every entry match the CLI exactly.

    ``owner`` substitutes the effective identity for every source (via
    :func:`~shards.core.lenses.as_effective_agent`) — "what would that agent's
    warm start show" — the same substitution ``--owner`` performs on the CLI
    side; ``team`` drops the identity filter on the activity half only (the task
    half, and the mentions target set built from it, always stay the effective
    agent's own). ``meta_only`` omits task bodies for the token-budget path;
    mentions and activity rows never carry a body regardless. Entirely
    daemon-independent (or degrades transparently), so this reads identically
    with the daemon down.
    """
    config = load_config()
    effective_config = as_effective_agent(config, owner)
    me = effective_config.agent

    task_views = DaemonClient().task_list(effective_config, mine=True, limit=None)
    note_views = DaemonClient().note_list(effective_config, owner=me, limit=None) if me else []
    mentions = session_mentions(
        effective_config, task_views, note_views, me=me, since=SESSION_SINCE
    )
    activity = recent_activity(
        effective_config,
        since=SESSION_SINCE,
        owner=None,
        mine=not team,
        limit=DEFAULT_RECENT_LIMIT,
    )
    return session_start_entries(task_views, activity, mentions, meta_only=meta_only)


# --------------------------------------------------------------------------- #
# Write tools (no special hint)                                               #
# --------------------------------------------------------------------------- #


def _with_warnings(payload: dict[str, Any], existing_id: str | None) -> dict[str, Any]:
    """Attach the structured ``warnings`` key (R9) — empty unless a title collided.

    The MCP mirror of the CLI's stderr line (``cli/note.py``/``cli/task.py``):
    there is no stream an agent reads on this surface, so the same advisory
    travels in the JSON result instead — never silently dropped, never mixed
    into the payload's own fields.
    """
    payload["warnings"] = (
        [f"duplicate title, also used by {existing_id}"] if existing_id is not None else []
    )
    return payload


def shards_note_new(
    title: str,
    note_type: str = "note",
    tags: list[str] | None = None,
    owner: str | None = None,
    body: str = "",
) -> dict[str, Any]:
    """Create a note (routed by type) and return its frontmatter plus any warnings."""
    config = load_config()
    # Checked before the write, so a hit names the *prior* id (R9); non-blocking
    # either way — see shards.core.notes.find_duplicate_title.
    existing_id = find_duplicate_note_title(config, title)
    note = create_note(config, title, note_type=note_type, tags=tags, owner=owner, body=body)
    return _with_warnings(note.model_dump(mode="json"), existing_id)


def shards_note_append(
    target: str,
    text: str,
    section: str | None = None,
    timestamp: bool = False,
) -> dict[str, Any]:
    """Append text to a note's body (optionally under a section / timestamped)."""
    config = load_config()
    note = append_note(config, target, text, section=section, timestamp=timestamp)
    return note.model_dump(mode="json")


def shards_task_new(
    title: str,
    priority: str | None = None,
    tags: list[str] | None = None,
    owner: str | None = None,
    body: str = "",
    project: str | None = None,
    blocks: list[str] | None = None,
    blocked_by: list[str] | None = None,
) -> dict[str, Any]:
    """Create a task in tasks/open/ (status open, unclaimed) and return it plus any warnings."""
    config = load_config()
    # Checked before the write, so a hit names the *prior* id (R9); non-blocking
    # either way — see shards.core.tasks.find_duplicate_title.
    existing_id = find_duplicate_task_title(config, title)
    task = create_task(
        config,
        title,
        priority=priority,
        tags=tags,
        owner=owner,
        body=body,
        project=project,
        blocks=blocks,
        blocked_by=blocked_by,
    )
    return _with_warnings(task.model_dump(mode="json"), existing_id)


def shards_task_append(
    task_id: str,
    text: str,
    section: str | None = None,
    timestamp: bool = False,
) -> dict[str, Any]:
    """Append text to a task's body (no status/folder change; mirrors note append)."""
    config = load_config()
    task = append_task(config, task_id, text, section=section, timestamp=timestamp)
    return task.model_dump(mode="json")


# --------------------------------------------------------------------------- #
# Idempotent tools                                                            #
# --------------------------------------------------------------------------- #


def shards_note_update(
    target: str,
    tags: str | None = None,
    new_type: str | None = None,
) -> dict[str, Any]:
    """Update a note's fields (tags, type — moving its folder) and bump updated."""
    config = load_config()
    note = update_note(config, target, tags=tags, new_type=new_type)
    return note.model_dump(mode="json")


def shards_task_claim(task_id: str, claimer: str | None = None) -> dict[str, Any]:
    """Claim a task for an agent (atomic test-and-set; same-owner reclaim is a no-op)."""
    config = load_config()
    who = claimer if claimer is not None else config.agent
    if not who:
        raise ValueError("no agent identity: pass claimer or set [core].agent")
    task = claim_task(config, task_id, who)
    return task.model_dump(mode="json")


def shards_task_release(task_id: str, owner: str | None = None) -> dict[str, Any]:
    """Release a claim, returning the task to open (atomic compare-and-clear; idempotent).

    Ships as of team-awareness/10 (previously withheld as a Phase-3 item — see
    the module docstring). ``owner`` names the acting agent, falling back to
    ``[core].agent`` exactly like :func:`shards_task_claim`'s ``claimer``.

    Deliberately has **no** ``force`` parameter: the CLI's ``--force`` breaks a
    claim held by a *different* agent, and owner identity is trusted local
    input, not a verified authorization boundary (root ``AGENTS.md`` §6) — so
    overriding someone else's claim stays a human/CLI action, never something an
    agent can trigger through this surface. Released *by* the holder (or when
    already unclaimed/terminal) is unaffected and stays idempotent.
    """
    config = load_config()
    who = owner if owner is not None else config.agent
    if not who:
        raise ValueError("no agent identity: pass owner or set [core].agent")
    task = release_task(config, task_id, who)
    return task.model_dump(mode="json")


def shards_task_finish(task_id: str, outcome: str | None = None) -> dict[str, Any]:
    """Finish a task: append an outcome and move it to tasks/done/ (idempotent)."""
    config = load_config()
    task = finish_task(config, task_id, outcome)
    return task.model_dump(mode="json")


def shards_task_update(
    task_id: str,
    priority: str | None = None,
    tags: str | None = None,
    title: str | None = None,
    project: str | None = None,
    owner: str | None = None,
    blocks: list[str] | None = None,
    blocked_by: list[str] | None = None,
) -> dict[str, Any]:
    """Update a task's fields (priority, tags, title, project, owner, blocks/blocked_by).

    ``owner`` reassigns accountability (a validated identity — must be in
    ``[tasks].collections``) and never touches ``claimed_by``: use
    ``shards_task_claim``/``shards_task_release`` for the execution handle.
    """
    config = load_config()
    task = update_task(
        config,
        task_id,
        priority=priority,
        tags=tags,
        title=title,
        project=project,
        owner=owner,
        blocks=blocks,
        blocked_by=blocked_by,
    )
    return task.model_dump(mode="json")


# --------------------------------------------------------------------------- #
# Destructive tools                                                           #
# --------------------------------------------------------------------------- #


def shards_task_cancel(task_id: str, reason: str | None = None) -> dict[str, Any]:
    """Cancel a task: append a reason and move it to tasks/done/ (idempotent)."""
    config = load_config()
    task = cancel_task(config, task_id, reason)
    return task.model_dump(mode="json")


# --------------------------------------------------------------------------- #
# Registration — bind each tool with its MCP annotation class                 #
# --------------------------------------------------------------------------- #

_READ_ONLY: dict[str, Any] = {"readOnlyHint": True}
_IDEMPOTENT: dict[str, Any] = {"idempotentHint": True}
_DESTRUCTIVE: dict[str, Any] = {"destructiveHint": True}


def _guarded(fn: Callable[..., Any]) -> Callable[..., Any]:
    """The one MCP-boundary exception mapper (core-hardening/3), applied at
    registration — the tool error mirror of the CLI's ``cli_errors()``.

    Same exception families, same catch order, same one-line messages as the CLI
    mapper (``shards.cli._errors.cli_errors``): ``ShardsError`` (``LockError``
    included) first, then a bare ``ValueError`` (also covers msgspec's
    ``ValidationError``, a ``ValueError`` subclass — e.g. an unknown owner or an
    invalid ``note_type``/sort field), then any other ``OSError``. MCP has no
    process exit code to map to, so each branch raises a clean
    ``fastmcp.exceptions.ToolError`` instead of letting FastMCP's own generic
    catch-all re-wrap an arbitrary traceback string. The full structured-error
    payload (codes, fields) is agent-usability/5's contract; this stays a plain
    message. The module-level ``shards_*`` functions are left unwrapped so they
    stay directly importable/unit-testable (see the module docstring) — only the
    registered tool goes through this wrapper.
    """

    @wraps(fn)
    def wrapper(*args: Any, **kwargs: Any) -> Any:
        try:
            return fn(*args, **kwargs)
        except ShardsError as exc:
            raise ToolError(str(exc)) from exc
        except ValueError as exc:
            raise ToolError(str(exc)) from exc
        except OSError as exc:
            raise ToolError(f"io error: {exc}") from exc

    return wrapper


def _register() -> None:
    """Register every ``shards_*`` tool with its annotation; called once at import."""
    # Read-only.
    app.tool(_guarded(shards_note_get), annotations=_READ_ONLY)
    app.tool(_guarded(shards_note_list), annotations=_READ_ONLY)
    app.tool(_guarded(shards_task_get), annotations=_READ_ONLY)
    app.tool(_guarded(shards_task_list), annotations=_READ_ONLY)
    app.tool(_guarded(shards_search), annotations=_READ_ONLY)
    app.tool(_guarded(shards_recent_activity), annotations=_READ_ONLY)
    app.tool(_guarded(shards_build_context), annotations=_READ_ONLY)
    app.tool(_guarded(shards_graph), annotations=_READ_ONLY)
    app.tool(_guarded(shards_project), annotations=_READ_ONLY)
    app.tool(_guarded(shards_session_start), annotations=_READ_ONLY)
    # Write (no special hint).
    app.tool(_guarded(shards_note_new))
    app.tool(_guarded(shards_note_append))
    app.tool(_guarded(shards_task_new))
    app.tool(_guarded(shards_task_append))
    # Idempotent.
    app.tool(_guarded(shards_note_update), annotations=_IDEMPOTENT)
    app.tool(_guarded(shards_task_claim), annotations=_IDEMPOTENT)
    app.tool(_guarded(shards_task_release), annotations=_IDEMPOTENT)
    app.tool(_guarded(shards_task_finish), annotations=_IDEMPOTENT)
    app.tool(_guarded(shards_task_update), annotations=_IDEMPOTENT)
    # Destructive.
    app.tool(_guarded(shards_task_cancel), annotations=_DESTRUCTIVE)


_register()


def main() -> None:  # pragma: no cover - process entry point
    """Serve the shards MCP tools (stdio transport). Wired as ``python -m shards.mcp.server``."""
    app.run()


if __name__ == "__main__":  # pragma: no cover - process entry point
    main()
