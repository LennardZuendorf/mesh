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
* **session-start** — :func:`session_start_entries`: the warm-start *composite*
  that merges a recent-activity window with the caller's live task queue.

Only the session-start composition is defined here; ``recent_activity``,
``build_context``, and ``graph_query`` keep their existing implementation
modules and are re-exported so callers have a single lens surface to import
from. The composition takes its two already-fetched sources as arguments
(rather than fetching), so the caller owns the fetch — which keeps the source
lenses independently testable and lets a surface substitute its own inputs.
"""

from __future__ import annotations

from datetime import datetime
from typing import Any

from shards.core.activity import recent_activity
from shards.core.context import GraphResult, SeedNotFoundError, build_context, graph_query
from shards.core.tasks import TaskView

__all__ = [
    "GraphResult",
    "SeedNotFoundError",
    "build_context",
    "graph_query",
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
