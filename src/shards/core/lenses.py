"""Read-lens layer — the one composable home for shards's read-only views.

Three read-only lenses over the single Markdown folder share the same
warm-index-or-fallback shape, so they are grouped here as one layer that surfaces
(CLI, MCP) and later units plug into rather than re-wiring ad hoc:

* **recent-activity** — :func:`recent_activity` (re-exported from
  :mod:`shards.core.activity`): time-ordered "what changed lately", served from
  the warm index when the daemon is up, an mtime dir-scan when it is down.
* **build-context** — :func:`build_context` (re-exported from
  :mod:`shards.core.context`): a daemon-independent BFS over the ``related`` id
  graph, seed first, cycle/diamond-deduped.
* **graph-query** — :func:`graph_query` (re-exported from
  :mod:`shards.core.context`): the same BFS promoted to a first-class "what's
  connected to X" query, returning a :class:`GraphResult` that renders as both
  machine JSON and a readable tree from one traversal.
* **project** — :func:`project_view`: given a ``type: project`` note id, returns
  that note plus every task whose ``project`` soft link points at it — "my
  project and its work in one call" — as a :class:`ProjectResult`.
* **session-start** — :func:`session_start_entries`: the warm-start *composite*
  that merges a recent-activity window with the caller's live task queue.

The session-start composition and the project lens are defined here;
``recent_activity``, ``build_context``, and ``graph_query`` keep their existing
implementation modules and are re-exported so callers have a single lens surface
to import from. The composition takes its two already-fetched sources as
arguments (rather than fetching), so the caller owns the fetch — which keeps the
source lenses independently testable and lets a surface substitute its own inputs.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime
from typing import Any

from shards.core.activity import recent_activity
from shards.core.context import GraphResult, SeedNotFoundError, build_context, graph_query
from shards.core.errors import ShardsError
from shards.core.notes import NoteError, get_note
from shards.core.tasks import TaskView, list_tasks
from shards.schemas.config import Config

__all__ = [
    "GraphResult",
    "ProjectNotFoundError",
    "ProjectResult",
    "SeedNotFoundError",
    "build_context",
    "graph_query",
    "project_view",
    "recent_activity",
    "session_start_entries",
]

# session-start only surfaces the caller's *live* queue — the non-terminal
# statuses. Terminal (done/cancelled) tasks are dropped from the task section.
_OPEN_STATUSES: frozenset[str] = frozenset({"open", "claimed"})


def _updated_key(entry: dict[str, Any]) -> float:
    """Descending-sort key for the activity remainder: ``updated`` then ``mtime``.

    Remaining entries come from :func:`recent_activity`, whose rows carry
    ``mtime`` (the on-disk ``updated`` proxy) rather than a parsed ``updated``
    field — so ``mtime`` is the effective sort. A parsed ``updated`` ISO string
    is honoured first for robustness; a missing/unparseable value → ``0.0``
    (sorts oldest).
    """
    updated = entry.get("updated")
    if isinstance(updated, str):
        try:
            return datetime.fromisoformat(updated.replace("Z", "+00:00")).timestamp()
        except ValueError:
            pass
    mtime = entry.get("mtime")
    if isinstance(mtime, (int, float)):
        return float(mtime)
    return 0.0


def session_start_entries(
    task_views: list[TaskView],
    activity: list[dict[str, Any]],
    *,
    meta_only: bool,
) -> list[dict[str, Any]]:
    """Compose the warm-start payload from a live task queue + a recent-activity window.

    Keeps only ``open``/``claimed`` tasks (the caller's live queue), renders each
    as its frontmatter dict plus ``path`` (and ``body`` unless ``meta_only``), then
    appends the recent-activity remainder — every activity row whose id is not
    already in the task section — re-sorted newest-first (``updated`` proxied by
    ``mtime``). The result is *tasks first* (what the agent still owes) then the
    remaining activity, de-duplicated by id.
    """
    open_views = [view for view in task_views if view.task.status in _OPEN_STATUSES]
    task_ids = {view.task.id for view in open_views}
    task_entries: list[dict[str, Any]] = []
    for view in open_views:
        entry = view.task.model_dump(mode="json")
        entry["path"] = str(view.path)
        if not meta_only:
            entry["body"] = view.body
        task_entries.append(entry)

    remaining = [entry for entry in activity if entry.get("id") not in task_ids]
    remaining.sort(key=_updated_key, reverse=True)

    return task_entries + remaining


# --------------------------------------------------------------------------- #
# project lens — a project note + the tasks scoped to it                        #
# --------------------------------------------------------------------------- #


class ProjectNotFoundError(ShardsError):
    """The project id resolves to no note (CLI exit 3).

    Only the *project note* is fatal: the lens is "the project note + its tasks",
    so an id that names no note has nothing to return. Carries the offending id
    for the CLI message.
    """

    code = 3

    def __init__(self, project_id: str) -> None:
        self.project_id = project_id
        super().__init__(f"project not found: {project_id}")


@dataclass(frozen=True, eq=False)
class ProjectResult:
    """One :func:`project_view` result — the project note and its scoped tasks.

    ``project`` is the project note's frontmatter dict plus a ``path`` key (the
    same node shape :func:`build_context`/:func:`graph_query` emit); ``tasks`` is
    the list of task nodes whose ``project`` soft link matches, newest-first.
    :meth:`to_dict` is a pure presentation over the two already-fetched fields.
    """

    project: dict[str, Any]
    tasks: list[dict[str, Any]]

    def to_dict(self) -> dict[str, Any]:
        """Machine JSON: ``{"project": <note>, "tasks": [<task>, ...]}``."""
        return {"project": self.project, "tasks": self.tasks}


def project_view(config: Config, project_id: str) -> ProjectResult:
    """Return a project note and every task scoped to it — the project lens (C2).

    Resolves ``project_id`` to its note (an ``n-`` id, or a title slug — the same
    resolution :func:`shards.core.notes.get_note` performs) and pairs it with
    :func:`shards.core.tasks.list_tasks`'s ``project``-filtered result (all
    statuses, newest-first). The ``project:`` link is *soft* — the note need not
    be ``type: project`` and tasks may reference a project id freely — so the lens
    resolves whatever note the id names. It is entirely daemon-independent (nodes
    read straight off disk). An id that resolves to no note raises
    :class:`ProjectNotFoundError` (the CLI maps it to exit 3); a project with no
    scoped tasks yields an empty ``tasks`` list, never an error.
    """
    try:
        note_view = get_note(config, project_id)
    except NoteError as exc:
        raise ProjectNotFoundError(project_id) from exc

    project_entry = {**note_view.note.model_dump(mode="json"), "path": str(note_view.path)}
    task_views = list_tasks(config, project=project_id, limit=None)
    tasks = [{**view.task.model_dump(mode="json"), "path": str(view.path)} for view in task_views]
    return ProjectResult(project=project_entry, tasks=tasks)
