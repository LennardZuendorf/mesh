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
  machine JSON and a readable tree from one traversal. ``direction`` (out /
  in / both — team-awareness/1) selects forward ``related``, inverted
  (:func:`~shards.core.context.inbound_ids`, the backlink/notify view), or
  their union.
* **project** — :func:`project_view`: given a ``type: project`` note id, returns
  that note plus every task whose ``project`` soft link points at it — "my
  project and its work in one call" — as a :class:`ProjectResult`.
* **session-start** — :func:`session_start_entries`: the warm-start *composite*
  that merges a recent-activity window, the caller's live task queue, and
  (team-awareness/7) :func:`session_mentions` — inbound links to nodes the
  caller owns or has claimed, the notify half that turns a peer's
  ``[[t-184G]]`` reference into something the addressee's warm start actually
  surfaces.
* **vault-status** — :func:`status_inputs` + :func:`status_report`: the
  vault-health composite behind ``shards status``. The daemon's ``vault.status``
  handler computes the index-derivable half (:func:`status_inputs`) and the client
  finishes the report, so the warm and on-disk paths share one shape without
  putting a whole-vault body scan on the daemon's event loop.

The session-start composition and the project lens are defined here;
``recent_activity``, ``build_context``, and ``graph_query`` keep their existing
implementation modules and are re-exported so callers have a single lens surface
to import from. The composition takes its two already-fetched sources as
arguments (rather than fetching), so the caller owns the fetch — which keeps the
source lenses independently testable and lets a surface substitute its own inputs.
"""

from __future__ import annotations

import time
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

import msgspec.structs

from shards.core.activity import recent_activity
from shards.core.context import (
    GraphResult,
    SeedNotFoundError,
    _inbound_index,
    _resolve_entry,
    build_context,
    graph_query,
)
from shards.core.errors import ShardsError
from shards.core.notes import NoteError, NoteView, _parse_since, get_note
from shards.core.tasks import TaskView
from shards.core.wikilinks import find_dangling
from shards.daemon.client import DaemonClient
from shards.schemas.config import Config
from shards.storage.files import read_body

# Reuse the canonical liveness + staleness rule rather than re-deriving it: a lock
# is stale iff its PID is dead OR its age exceeds LOCK_TTL_SECONDS (300 s).
from shards.storage.locks import _is_stale

__all__ = [
    "GraphResult",
    "ProjectNotFoundError",
    "ProjectResult",
    "SESSION_SINCE",
    "SeedNotFoundError",
    "as_effective_agent",
    "build_context",
    "graph_query",
    "project_view",
    "recent_activity",
    "scan_stale_locks",
    "session_mentions",
    "session_start_entries",
    "status_inputs",
    "status_report",
]

# Zero-filled in every report so the ``tasks`` payload has a stable shape.
TASK_STATUSES: tuple[str, ...] = ("open", "claimed", "done", "cancelled")

# session-start only surfaces the caller's *live* queue — the non-terminal
# statuses. Terminal (done/cancelled) tasks are dropped from the task section.
_OPEN_STATUSES: frozenset[str] = frozenset({"open", "claimed"})

# The recency window ``session_start_entries``' mentions/activity halves share
# (team-awareness/7) — one constant so the CLI's ``session-start`` and the MCP
# ``shards_session_start`` mirror cannot drift apart on the window itself.
SESSION_SINCE: str = "7d"


def as_effective_agent(config: Config, agent: str | None) -> Config:
    """``config`` with ``[core].agent`` substituted for ``agent`` (a no-op if unset).

    ``--owner``/``owner=`` on ``session-start`` means "show me *that* agent's
    warm start", and every source the composite draws on (``task_list(mine=True)``,
    ``note_list(owner=...)``, ``recent_activity(mine=True)``,
    :func:`session_mentions`) resolves "me" from ``config.agent`` — so
    substituting the identity once, here, drives every source through its
    existing ``mine``/``me`` semantics unchanged, rather than threading a second
    "effective agent" parameter through each one. Shared by both surfaces'
    ``session-start`` (``cli/session.py`` and ``mcp/server.py``) so the identity
    swap has exactly one implementation. ``Config`` is an unfrozen
    :class:`msgspec.Struct`, so this is a cheap, local copy — the caller's own
    ``config`` (and anyone else holding it) is never mutated.
    """
    if agent is None:
        return config
    return msgspec.structs.replace(config, core=msgspec.structs.replace(config.core, agent=agent))


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


def session_mentions(
    config: Config,
    task_views: list[TaskView],
    note_views: list[NoteView],
    *,
    me: str | None,
    since: str,
) -> list[dict[str, Any]]:
    """Inbound mentions of my nodes — the notify half of session-start (team-awareness/7).

    "My nodes" are every task I own or have claimed (``owner == me or
    claimed_by == me`` — exactly ``task_views`` as the caller already fetched it
    for the task half: unfiltered by status, since a mention of a task I already
    finished is still a mention) plus every note I own (``owner == me``,
    ``note_views``). For each target id, the whole-vault reverse-``related`` map
    (:func:`shards.core.context._inbound_index`) is built **once** — not once per
    target, which is exactly the batched form its own docstring calls for — and
    every target's inbound (mentioning) ids are unioned into one mentioner set,
    so a node mentioning two of my targets is still resolved once.

    Each mentioner is resolved to a full entry via
    :func:`shards.core.context._resolve_entry` — the same single-node resolver
    :func:`~shards.core.context.build_context`/:func:`~shards.core.context.graph_query`
    use, so there is no second id→entry lookup implementation. A resolved
    mentioner is dropped when it is unresolvable (a dangling/deleted mentioner,
    matching every other inbound-consuming path), when its own ``owner`` is
    ``me`` (a mention *by* me of my own node — R7's "mentions by me of my own
    nodes are excluded"), or when its ``updated`` falls outside the ``since``
    recency floor (the identical window :func:`shards.core.activity.recent_activity`
    applies, parsed once via :func:`shards.core.notes._parse_since`, and reusing
    :func:`_updated_key` so the mtime-fallback rule cannot drift between the two
    windows). Surviving entries are sorted newest-first, matching the
    remaining-activity ordering rule.

    Entirely daemon-independent: :func:`_inbound_index` reads straight off disk
    (module docstring, "Inbound derivation"), so this composes to an identical
    result with the daemon up or down — no infrastructure notice is ever owed
    from this half.
    """
    my_ids: set[str] = {view.task.id for view in task_views}
    my_ids.update(view.note.id for view in note_views)
    if not my_ids:
        return []

    cutoff = _parse_since(since).timestamp()
    inbound_map = _inbound_index(config)
    mentioner_ids: set[str] = set()
    for target_id in my_ids:
        mentioner_ids.update(inbound_map.get(target_id, []))

    entries: list[dict[str, Any]] = []
    for mentioner_id in mentioner_ids:
        entry = _resolve_entry(config, mentioner_id)
        if entry is None or entry.get("owner") == me:
            continue
        if _updated_key(entry) < cutoff:
            continue
        entries.append(entry)

    entries.sort(key=_updated_key, reverse=True)
    return entries


def session_start_entries(
    task_views: list[TaskView],
    activity: list[dict[str, Any]],
    mentions: list[dict[str, Any]],
    *,
    meta_only: bool,
) -> list[dict[str, Any]]:
    """Compose the warm-start payload: live task queue + mentions + recent activity.

    Keeps only ``open``/``claimed`` tasks (the caller's live queue), renders each
    as its frontmatter dict plus ``path`` (and ``body`` unless ``meta_only``);
    appends the already-resolved ``mentions`` (:func:`session_mentions`) whose id
    is not already a task; appends the recent-activity remainder — every activity
    row whose id is not already a task or a mention — re-sorted newest-first
    (``updated`` proxied by ``mtime``). Every entry gains a ``reason`` key —
    ``"task"``, ``"mention"``, or ``"activity"`` — so a flat JSON array stays
    self-describing (team-awareness/7).

    The result is **tasks, then mentions, then remaining activity**, de-duplicated
    by id across all three sections. Precedence when an id would otherwise appear
    twice: the *earlier* section wins — a mention that is also one of my own
    tasks (I hold it, and something else I hold links it) surfaces once, under
    ``reason="task"``, never duplicated as a mention; likewise a mention that
    also falls in the recent-activity window surfaces once, under
    ``reason="mention"``. This is the same "tasks first" precedence the compose
    already used before mentions existed, extended one section further.

    Bodies are read here, per surviving task, rather than taken off the passed
    views: a list-shaped read is served from the daemon's warm index, which holds
    frontmatter only, so its rows carry no body. Reading them here keeps the
    payload identical daemon-up and daemon-down, and costs one ``open()`` per
    *live* task instead of a body-carrying walk of the whole vault. Mentions and
    activity rows never carry a body regardless of ``meta_only`` — neither
    ``_resolve_entry`` nor a warm-index row reads one — so this stays sane under
    ``--meta-only`` by construction, not by a special case here.
    """
    open_views = [view for view in task_views if view.task.status in _OPEN_STATUSES]
    seen_ids: set[Any] = {view.task.id for view in open_views}
    task_entries: list[dict[str, Any]] = []
    for view in open_views:
        entry = view.task.model_dump(mode="json")
        entry["path"] = str(view.path)
        entry["reason"] = "task"
        if not meta_only:
            entry["body"] = read_body(view.path)
        task_entries.append(entry)

    mention_entries: list[dict[str, Any]] = []
    for entry in mentions:
        entry_id = entry.get("id")
        if entry_id in seen_ids:
            continue
        seen_ids.add(entry_id)
        mention_entries.append({**entry, "reason": "mention"})

    remaining = []
    for entry in activity:
        entry_id = entry.get("id")
        if entry_id in seen_ids:
            continue
        seen_ids.add(entry_id)
        remaining.append({**entry, "reason": "activity"})
    remaining.sort(key=_updated_key, reverse=True)

    return task_entries + mention_entries + remaining


# --------------------------------------------------------------------------- #
# vault-status lens — counts + freshness + dangling links + stale locks          #
# --------------------------------------------------------------------------- #


def scan_stale_locks(config: Config) -> list[Path]:
    """Every stale ``O_EXCL`` lock under ``notes/.locks`` and ``tasks/.locks``.

    A lock is stale iff its PID is dead **or** its age exceeds the 300 s TTL — the
    exact rule enforced on acquire (:func:`shards.storage.locks._is_stale`), reused
    here so ``shards status`` and the locker never disagree. Lock files are not
    vault Markdown, so the warm index does not track them; this stays a directory
    listing on both the warm and the on-disk path (a listing, not a parse).
    """
    vault = config.core.vault_path
    stale: list[Path] = []
    for kind in ("notes", "tasks"):
        locks = vault / kind / ".locks"
        if not locks.is_dir():
            continue
        for lock in sorted(locks.glob("*.lock")):
            if _is_stale(lock):
                stale.append(lock)
    return stale


def status_inputs(notes: list[NoteView], tasks: list[TaskView]) -> dict[str, Any]:
    """Reduce the note/task lenses to the two primitives :func:`status_report` needs.

    Split out from the report so the daemon can compute *only this half* from its
    warm index and ship it: the rest of the report reads bodies and lock files off
    disk, and ``DaemonServer._dispatch`` runs handlers synchronously on the event
    loop, so doing that work daemon-side would block every other agent's warm read
    behind one ``shards status``. Both callers reduce their views through this one
    function, so the warm and on-disk inputs cannot drift.
    """
    return {"note_count": len(notes), "task_statuses": [view.task.status for view in tasks]}


def status_report(
    config: Config,
    *,
    note_count: int,
    task_statuses: Sequence[str],
    newest: list[dict[str, Any]],
) -> dict[str, Any]:
    """Assemble the vault-health payload from already-fetched lens primitives.

    Like :func:`session_start_entries`, this composes sources the *caller* fetched
    rather than fetching them itself — which is exactly what lets the client feed
    it disk-scanned counts when the daemon is down and index-derived counts (over
    the socket) when it is up, from one implementation of the shape. ``newest`` is
    a one-row recency window (:meth:`~shards.index.warm.VaultIndex.recent` or
    :func:`~shards.index.warm.scan_recent`) whose ``mtime`` drives freshness.

    ``dangling_links`` and ``stale_locks`` are computed here, on disk, by whoever
    calls this — always the client. ``dangling_links`` needs note/task **bodies**,
    which the warm index deliberately does not hold, and ``stale_locks`` is a
    lock-directory listing rather than a parse. So ``shards status`` is warm in its
    *counts* and unchanged in its link scan; that scan is a per-invocation cost in
    the caller's own process, where it blocks nobody else. Strictly read-only:
    nothing here writes.
    """
    task_counts = dict.fromkeys(TASK_STATUSES, 0)
    for status in task_statuses:
        task_counts[status] = task_counts.get(status, 0) + 1

    mtime: float | None = None
    age: float | None = None
    if newest:
        mtime = float(newest[0]["mtime"])
        age = max(0.0, time.time() - mtime)

    return {
        "notes": note_count,
        "tasks": task_counts,
        "tasks_total": len(task_statuses),
        "freshness": {"mtime": mtime, "age_seconds": age},
        "dangling_links": find_dangling(config.core.vault_path),
        "stale_locks": [str(p) for p in scan_stale_locks(config)],
    }


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
    resolves whatever note the id names. The project note is a *point* read
    straight off disk (one ``open()`` the id already determines — a socket hop
    would buy nothing); its task list is the list-shaped read, so it goes through
    :meth:`DaemonClient.task_list <shards.daemon.client.DaemonClient.task_list>`
    and is served from the warm index when the daemon is up, from the identical
    disk walk when it is down. An id that resolves to no note raises
    :class:`ProjectNotFoundError` (the CLI maps it to exit 3); a project with no
    scoped tasks yields an empty ``tasks`` list, never an error.
    """
    try:
        note_view = get_note(config, project_id)
    except NoteError as exc:
        raise ProjectNotFoundError(project_id) from exc

    project_entry = {**note_view.note.model_dump(mode="json"), "path": str(note_view.path)}
    task_views = DaemonClient().task_list(config, project=project_id, limit=None)
    tasks = [{**view.task.model_dump(mode="json"), "path": str(view.path)} for view in task_views]
    return ProjectResult(project=project_entry, tasks=tasks)
