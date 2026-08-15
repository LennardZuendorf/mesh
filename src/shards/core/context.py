"""Build-context / graph-query lens — a BFS over the ``related`` id graph.

:func:`build_context` is Phase-2's read-only "pull the neighbourhood of an id"
primitive, shared by the ``shards build-context`` CLI command and the
``shards_build_context`` MCP tool. :func:`graph_query` (cli-toolset-rework/3)
promotes that same traversal to a first-class "what's connected to X" query,
returning a :class:`GraphResult` that machine JSON and a readable tree are both
derived from. Starting at a seed id, both walk the ``related`` frontmatter
lists breadth-first, expanding one hop at each level until reaching ``depth``
hops from the seed (``depth=1`` = the seed plus its direct ``related`` entries;
``depth=0`` = the seed alone). They share one internal traversal (:func:`_bfs`)
— there is no second graph engine.

It is a *lens*, not a store, and entirely **daemon-independent**: every node is
read straight off disk via :func:`shards.core.notes.get_note` (``n-`` ids) and
:func:`shards.core.tasks.get_task` (``t-`` ids), so it behaves identically with
the daemon down. There is no degradation path and therefore no infrastructure
notice. Deliberately: nothing here imports the hybrid-search/``indexed`` layer —
this is the structural "what's connected" path; ``search`` stays the ranked-
recall path.

Each visited entry is emitted as the standard note/task frontmatter shape
(``Note`` / ``Task`` model dump) plus a ``path`` key, in BFS traversal order with
the seed first. Ids are de-duplicated by adding them to a *seen* set at enqueue
time, so cycles (``A → B → A``) terminate and diamonds (``A → {B, C} → D``) visit
and resolve ``D`` exactly once. A ``related`` id that resolves to no file (a
dangling link) is skipped silently — only an unresolvable **seed** is fatal, and
that raises :class:`SeedNotFoundError` (mapped by the CLI to exit 3).

:func:`graph_query` additionally records the parent→child "discovery edge" the
BFS produces the first (and only) time each neighbour is enqueued — exactly the
spanning tree cycle/diamond dedup already collapses the graph to. That edge list
is what lets :class:`GraphResult` render a readable tree without a second walk.
"""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass
from typing import Any

from msgspec import ValidationError

from shards.core.errors import ShardsError
from shards.core.notes import NoteError, get_note
from shards.core.tasks import TaskError, get_task
from shards.schemas.config import Config

__all__ = ["GraphResult", "SeedNotFoundError", "build_context", "graph_query"]

_TASK_PREFIX = "t-"


class SeedNotFoundError(ShardsError):
    """The build-context seed id resolves to no note or task (CLI exit 3).

    Only the *seed* is fatal: a dangling ``related`` id encountered mid-traversal
    is skipped, never raised. Carries the offending seed id for the CLI message.
    """

    code = 3

    def __init__(self, seed_id: str) -> None:
        self.seed_id = seed_id
        super().__init__(f"seed not found: {seed_id}")


def _resolve_entry(config: Config, entry_id: str) -> dict[str, Any] | None:
    """Read ``entry_id`` into a frontmatter-plus-``path`` dict, or ``None``.

    Routes by id prefix: a ``t-`` id resolves through
    :func:`shards.core.tasks.get_task`; anything else (``n-`` ids, and — matching
    ``note`` CLI behaviour — a title slug) through
    :func:`shards.core.notes.get_note`. Not-found, ambiguous, or malformed nodes
    (``NoteError`` / ``TaskError`` / ``ValidationError``) yield ``None`` so the
    caller can skip a dangling neighbour without aborting the whole traversal.
    """
    try:
        if entry_id.startswith(_TASK_PREFIX):
            task_view = get_task(config, entry_id)
            return {**task_view.task.model_dump(mode="json"), "path": str(task_view.path)}
        note_view = get_note(config, entry_id)
        return {**note_view.note.model_dump(mode="json"), "path": str(note_view.path)}
    except (NoteError, TaskError, ValidationError):
        return None


def _bfs(
    config: Config, seed_id: str, depth: int
) -> tuple[list[dict[str, Any]], list[tuple[str, str]]]:
    """Shared traversal: BFS over ``related`` from ``seed_id`` to ``depth`` hops.

    Returns ``(entries, edges)`` — ``entries`` is the seed-first, deduped BFS
    order (the shape :func:`build_context` returns as-is); ``edges`` is the
    ``(parent_id, child_id)`` discovery edge recorded the one time each
    neighbour is first enqueued, in traversal order. Every caller
    (:func:`build_context`, :func:`graph_query`) walks the folder through this
    one function — there is no second traversal.

    Ids are de-duplicated: a *seen* set (seeded with the seed's id and extended
    at enqueue time) guarantees each id is emitted, and edged, at most once, so
    cycles terminate and diamond graphs never duplicate a shared node or its
    edge. A ``related`` id that resolves to no file is skipped. An unresolvable
    **seed** raises :class:`SeedNotFoundError` (the CLI maps it to exit 3).
    """
    seed = _resolve_entry(config, seed_id)
    if seed is None:
        raise SeedNotFoundError(seed_id)

    result: list[dict[str, Any]] = []
    edges: list[tuple[str, str]] = []
    seen: set[str] = {str(seed["id"]), seed_id}
    queue: deque[tuple[dict[str, Any], int]] = deque([(seed, 0)])

    while queue:
        entry, hop = queue.popleft()
        result.append(entry)
        if hop >= depth:
            continue
        related = entry.get("related", [])
        for rel_id in related if isinstance(related, list) else []:
            key = str(rel_id)
            if key in seen:
                continue
            seen.add(key)
            neighbour = _resolve_entry(config, key)
            if neighbour is None:
                continue
            seen.add(str(neighbour["id"]))
            edges.append((str(entry["id"]), str(neighbour["id"])))
            queue.append((neighbour, hop + 1))

    return result, edges


def build_context(config: Config, seed_id: str, depth: int = 1) -> list[dict[str, Any]]:
    """Return the ``related``-graph neighbourhood of ``seed_id`` as a BFS list.

    Walks ``related`` breadth-first from ``seed_id`` out to ``depth`` hops
    (``depth=0`` → seed only; ``depth=1`` → seed + direct related; and so on),
    resolving each id through :func:`_resolve_entry` (``n-`` → note, ``t-`` →
    task). Returns the visited entries in traversal order, seed first, each shaped
    as the standard note/task frontmatter dict plus ``path`` — JSON-serialisable
    end to end.

    Ids are de-duplicated: a *seen* set (seeded with the seed's id and extended at
    enqueue time) guarantees each id is emitted at most once, so cycles terminate
    and diamond graphs never duplicate a shared node. A ``related`` id that
    resolves to no file is skipped. An unresolvable **seed** raises
    :class:`SeedNotFoundError` (the CLI maps it to exit 3).
    """
    entries, _edges = _bfs(config, seed_id, depth)
    return entries


@dataclass(frozen=True, eq=False)
class GraphResult:
    """One :func:`graph_query` result — ready to render as JSON or a tree.

    ``entries`` is the same seed-first, deduped BFS order :func:`build_context`
    returns (frontmatter dict + ``path`` per node). ``edges`` is the discovery
    spanning tree :func:`_bfs` records while walking: ``(parent_id, child_id)``
    pairs, one per node the *first* (and only) time it is discovered — exactly
    the tree a cycle or diamond collapses to once dedup applies.

    :meth:`to_dict` and :meth:`tree_lines` are both pure presentations over
    these two already-fetched fields — deriving either never touches disk again,
    so JSON and tree output come from the one upstream traversal.
    """

    entries: list[dict[str, Any]]
    edges: list[tuple[str, str]]

    @property
    def seed_id(self) -> str:
        """The resolved seed id (first entry — ``_bfs`` guarantees non-empty)."""
        return str(self.entries[0]["id"])

    @property
    def ids(self) -> list[str]:
        """Every visited id, in BFS traversal order, seed first."""
        return [str(entry["id"]) for entry in self.entries]

    def to_dict(self) -> dict[str, Any]:
        """Machine JSON: ``{"seed": id, "nodes": [...], "edges": [[parent, child], ...]}``."""
        return {
            "seed": self.seed_id,
            "nodes": self.entries,
            "edges": [[parent, child] for parent, child in self.edges],
        }

    def tree_lines(self) -> list[str]:
        """A readable tree: DFS pre-order over the discovery spanning tree.

        Each line is ``"  " * depth + "id\\ttype\\ttitle"``, so indentation alone
        encodes parent/child structure — the same terse ``id/type/title`` row
        shape the other session-lens commands print, just nested.
        """
        by_id = {str(entry["id"]): entry for entry in self.entries}
        children: dict[str, list[str]] = {}
        for parent, child in self.edges:
            children.setdefault(parent, []).append(child)

        lines: list[str] = []

        def _walk(node_id: str, hop: int) -> None:
            entry = by_id[node_id]
            indent = "  " * hop
            lines.append(
                f"{indent}{entry.get('id', '')}\t{entry.get('type', '')}\t{entry.get('title', '')}"
            )
            for child_id in children.get(node_id, []):
                _walk(child_id, hop + 1)

        _walk(self.seed_id, 0)
        return lines


def graph_query(config: Config, seed_id: str, depth: int = 1) -> GraphResult:
    """Query "what's connected to ``seed_id``" — the first-class graph surface.

    Runs the exact same BFS :func:`build_context` performs (via :func:`_bfs`,
    the single shared traversal) and additionally captures the discovery edges,
    returning both as one :class:`GraphResult`. Callers render machine JSON
    (:meth:`GraphResult.to_dict`) and/or a readable tree
    (:meth:`GraphResult.tree_lines`) from that one result — never by calling
    this twice. An unresolvable seed raises :class:`SeedNotFoundError` (the CLI
    maps it to exit 3), matching :func:`build_context`.
    """
    entries, edges = _bfs(config, seed_id, depth)
    return GraphResult(entries=entries, edges=edges)
