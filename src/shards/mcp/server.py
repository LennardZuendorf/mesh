"""FastMCP server — the agent-facing ``shards_*`` tool surface (memory/1).

This is the MCP mirror of the *safe* CLI verbs. It is deliberately thin: every
tool loads the config, routes straight through the existing ``core`` domain logic
(and ``daemon/client.py`` for the recent-activity lens and search), and returns a
JSON-serialisable dict. There is **no** parallel re-implementation of note, task,
or search behaviour — the tools call the same functions the CLI does.

Unlike the CLI, MCP parameters are *typed fields*, not flag strings: ``shards_note_get``
takes ``id: str``, not ``--id <id>``. Each tool carries an MCP annotation
declaring its effect, so an agent runtime can reason about safety before calling:

* **read-only** (``readOnlyHint``) — ``note_get`` / ``note_list`` / ``task_get`` /
  ``task_list`` / ``search`` / ``recent_activity`` / ``build_context`` / ``graph`` /
  ``project``;
* **idempotent** (``idempotentHint``) — ``note_update`` / ``task_claim`` /
  ``task_finish`` / ``task_update`` (re-running lands the same state);
* **write** (no special hint) — ``note_new`` / ``note_append`` / ``task_new``;
* **destructive** (``destructiveHint``) — ``task_cancel`` (a one-way lifecycle move).

The unsafe / administrative surface is **withheld**: neither delete verb, no daemon
controls, no ``reindex`` or ``status``, and not the Phase-3 ``task_release``. Those
never reach an agent through MCP.

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
from shards.core.lenses import build_context, graph_query, project_view, recent_activity
from shards.core.notes import (
    NoteView,
    append_note,
    create_note,
    get_note,
    list_notes,
    update_note,
)
from shards.core.search import hit_dict, query_search
from shards.core.tasks import (
    TaskView,
    cancel_task,
    claim_task,
    create_task,
    finish_task,
    get_task,
    list_tasks,
    update_task,
)
from shards.index.tagpull import tagpull
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
    views = list_notes(
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
    sort: str = "updated",
    limit: int = 20,
) -> list[dict[str, Any]]:
    """List shards tasks (open and done) with status/owner/mine/project filters, sorted."""
    config = load_config()
    views = list_tasks(
        config,
        status=status,
        owner=owner,
        mine=mine,
        tags=tags,
        any_tag=any_tag,
        project=project,
        since=since,
        sort=sort,
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
        results = tagpull(
            config,
            tags=tags,
            type_filter=type_filter,
            owner=owner,
            status=status,
            limit=limit,
        )
    else:
        effective_threshold = threshold if threshold is not None else config.search.threshold
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
    """Recent vault changes (newest first) as {id, type, title, path, mtime} rows."""
    config = load_config()
    return recent_activity(config, since=since, owner=owner, mine=mine, limit=limit)


def shards_build_context(seed_id: str, depth: int = 1) -> list[dict[str, Any]]:
    """Expand the ``related`` graph around a seed id (BFS to depth, seed first)."""
    config = load_config()
    return build_context(config, seed_id, depth=depth)


def shards_graph(seed_id: str, depth: int = 1) -> dict[str, Any]:
    """Query what's connected to a seed id: ``{seed, nodes, edges}`` (BFS to depth)."""
    config = load_config()
    return graph_query(config, seed_id, depth=depth).to_dict()


def shards_project(project_id: str) -> dict[str, Any]:
    """Show a project note and the tasks scoped to it: ``{project, tasks}``."""
    config = load_config()
    return project_view(config, project_id).to_dict()


# --------------------------------------------------------------------------- #
# Write tools (no special hint)                                               #
# --------------------------------------------------------------------------- #


def shards_note_new(
    title: str,
    note_type: str = "note",
    tags: list[str] | None = None,
    owner: str | None = None,
    body: str = "",
) -> dict[str, Any]:
    """Create a note (routed by type) and return its frontmatter."""
    config = load_config()
    note = create_note(config, title, note_type=note_type, tags=tags, owner=owner, body=body)
    return note.model_dump(mode="json")


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
    """Create a task in tasks/open/ (status open, unclaimed) and return it."""
    config = load_config()
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
    blocks: list[str] | None = None,
    blocked_by: list[str] | None = None,
) -> dict[str, Any]:
    """Update a task's fields (priority, tags, title, project, blocks/blocked_by) in place."""
    config = load_config()
    task = update_task(
        config,
        task_id,
        priority=priority,
        tags=tags,
        title=title,
        project=project,
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

    Same exception families (``ShardsError`` — ``LockError`` included — and any
    other ``OSError``), same one-line message, but MCP has no process exit code
    to map to, so this raises a clean ``fastmcp.exceptions.ToolError`` instead of
    letting FastMCP's own generic catch-all re-wrap an arbitrary traceback string.
    The full structured-error payload (codes, fields) is agent-usability/5's
    contract; this stays a plain message. The module-level ``shards_*`` functions
    are left unwrapped so they stay directly importable/unit-testable (see the
    module docstring) — only the registered tool goes through this wrapper.
    """

    @wraps(fn)
    def wrapper(*args: Any, **kwargs: Any) -> Any:
        try:
            return fn(*args, **kwargs)
        except ShardsError as exc:
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
    # Write (no special hint).
    app.tool(_guarded(shards_note_new))
    app.tool(_guarded(shards_note_append))
    app.tool(_guarded(shards_task_new))
    # Idempotent.
    app.tool(_guarded(shards_note_update), annotations=_IDEMPOTENT)
    app.tool(_guarded(shards_task_claim), annotations=_IDEMPOTENT)
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
