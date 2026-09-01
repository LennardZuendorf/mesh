"""Build-context / graph-query lens — a BFS over the ``related`` id graph.

:func:`build_context` is Phase-2's read-only "pull the neighbourhood of an id"
primitive, shared by the ``mesh build-context`` CLI command and the
``mesh_build_context`` MCP tool. :func:`graph_query` (cli-toolset-rework/3)
promotes that same traversal to a first-class "what's connected to X" query,
returning a :class:`GraphResult` that machine JSON and a readable tree are both
derived from. Starting at a seed id, both walk the ``related`` frontmatter
lists breadth-first, expanding one hop at each level until reaching ``depth``
hops from the seed (``depth=1`` = the seed plus its direct ``related`` entries;
``depth=0`` = the seed alone). They share one internal traversal (:func:`_bfs`)
— there is no second graph engine.

It is a *lens*, not a store, and entirely **daemon-independent**: every node is
read straight off disk via :func:`mesh.core.notes.get_note` (``n-`` ids) and
:func:`mesh.core.tasks.get_task` (``t-`` ids), so it behaves identically with
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

**Inbound derivation (team-awareness/1).** ``related`` is a pure function of a
node's own body (``core/notes.py``/``core/tasks.py`` recompute it from
``[[wikilinks]]`` on every write); :func:`inbound_ids` inverts it at *read*
time: ``inbound(X) = {N : X in N.related}``. This is the whole mechanism that
makes a mention deliverable — a note that says "@agent, see [[t-184G]]" is
otherwise invisible from ``t-184G``'s own frontmatter, because ``related`` only
ever points forward, out of the file that wrote it. :func:`_bfs` takes a
``direction`` (``"out"`` the existing forward walk, ``"in"`` the inverted one,
``"both"`` their union) so ``graph_query`` — and only ``graph_query``, per the
three-verbs contract — can surface backlinks via ``mesh graph <id>
--direction in``. No new frontmatter, no new store: one extra vault pass per
query, same daemon-free-by-construction shape as everything else in this
module.
"""

from __future__ import annotations

import itertools
from collections import deque
from dataclasses import dataclass
from typing import Any

from msgspec import ValidationError

from mesh.core.errors import MeshError
from mesh.core.notes import NoteError, get_note, note_rows
from mesh.core.tasks import TaskError, get_task, task_rows
from mesh.schemas.config import Config

__all__ = [
    "GraphResult",
    "SeedNotFoundError",
    "build_context",
    "graph_query",
    "inbound_ids",
]

_TASK_PREFIX = "t-"
_ID_PREFIXES = ("n-", "t-")
_DIRECTIONS = ("out", "in", "both")


class SeedNotFoundError(MeshError):
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
    :func:`mesh.core.tasks.get_task`; anything else (``n-`` ids, and — matching
    ``note`` CLI behaviour — a title slug) through
    :func:`mesh.core.notes.get_note`. Not-found, ambiguous, or malformed nodes
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


def _inbound_index(config: Config) -> dict[str, list[str]]:
    """Build the whole-vault reverse-``related`` map in one walk: target id → source ids.

    Walks the corpus once via :func:`mesh.core.notes.note_rows` (``notes/``)
    and :func:`mesh.core.tasks.task_rows` (``tasks/{open,done}/``) — the same
    two row-walks ``note list``/``task list`` already use, each already routed
    through :func:`mesh.storage.files.read_post`, so a malformed ``.md`` skips
    silently and never aborts the scan. A row with no mesh id (a foreign file
    from any writer coexisting in the same folder) is skipped: it cannot be a
    valid source. A row whose ``related`` is missing or not a list contributes
    nothing. Because ``related`` already holds *resolved* ids — title-form
    ``[[Title]]`` links are resolved to their id at write time
    (:func:`mesh.core.wikilinks.resolve_wikilinks`) — no title lookup of its
    own is needed to catch a title-form mention.

    This is the batched form :func:`_bfs` uses: a multi-hop ``in``/``both``
    query touches many nodes, and re-walking the corpus once *per visited
    node* would cost O(visited × vault) instead of the O(vault) a single query
    should cost (root tech.md § Inbound derivation, "Cost" — the same class
    :func:`mesh.core.wikilinks.find_dangling` already pays for ``mesh
    status``). One pass here builds the map; every neighbour lookup during the
    traversal is then an O(1) dict read. :func:`inbound_ids` is the equivalent
    single-target convenience wrapper for a caller that just wants one node's
    backlinks without running a BFS.

    Each target's source list is sorted (by id) before being returned: the
    underlying walk (``_iter_note_files``/``_iter_task_files``, unlike
    ``core.wikilinks``'s explicit ``sorted(rglob(...))``) makes no filesystem
    directory-order guarantee, and a node with two or more backlinks needs a
    deterministic order — the same reason :func:`mesh.core.notes.select_notes`
    tie-breaks by path rather than trusting scan order.
    """
    index: dict[str, list[str]] = {}
    for _path, meta in itertools.chain(note_rows(config), task_rows(config)):
        source_id = meta.get("id")
        if not (isinstance(source_id, str) and source_id.startswith(_ID_PREFIXES)):
            continue
        related = meta.get("related")
        if not isinstance(related, list):
            continue
        for rel_id in related:
            index.setdefault(str(rel_id), []).append(source_id)
    for sources in index.values():
        sources.sort()
    return index


def inbound_ids(config: Config, target_id: str) -> list[str]:
    """Return every id whose ``related`` list names ``target_id`` — the backlink set.

    ``inbound(X) = {N : X ∈ N.related}`` (root tech.md § Inbound derivation): the
    pure inverse of the forward edge :func:`mesh.core.notes.append_note` /
    :func:`mesh.core.tasks.update_task` (and friends) recompute from a body's
    ``[[wikilinks]]`` on every write. Nothing is stored — one fresh vault walk
    per call, via :func:`_inbound_index`. Order is scan order (notes before
    tasks); a caller that needs a stable order sorts it themselves.

    Not (yet) served from the warm index — see the module docstring — so this
    is identical daemon-up and daemon-down by construction, not by an
    accelerator path that has to agree with a disk fallback.
    """
    return list(_inbound_index(config).get(target_id, []))


def _out_candidates(entry: dict[str, Any]) -> list[tuple[str, bool]]:
    """Forward (``related``) neighbours of ``entry``: ``(id, reversed=False)`` pairs."""
    related = entry.get("related", [])
    if not isinstance(related, list):
        return []
    return [(str(rel_id), False) for rel_id in related]


def _in_candidates(
    entry: dict[str, Any], inbound_map: dict[str, list[str]]
) -> list[tuple[str, bool]]:
    """Inbound (backlink) neighbours of ``entry``: ``(id, reversed=True)`` pairs."""
    return [(source_id, True) for source_id in inbound_map.get(str(entry["id"]), [])]


def _neighbour_candidates(
    entry: dict[str, Any], direction: str, inbound_map: dict[str, list[str]] | None
) -> list[tuple[str, bool]]:
    """The ``(neighbour_id, reversed)`` pairs :func:`_bfs` expands ``entry`` to.

    ``reversed`` is ``True`` exactly for an inbound neighbour — the neighbour is
    the *mentioner* and ``entry`` the *mentioned*, so the discovery edge must be
    emitted ``(neighbour, entry)`` to stay direction-true (module docstring,
    "Edges stay direction-true"). ``"both"`` concatenates out-candidates then
    in-candidates: :func:`_bfs`'s *seen* dedup then collapses a mutual link (``A``
    in ``B.related`` and ``B`` in ``A.related``) to the one edge the forward half
    already produces, so ``--direction both`` never double-counts it.
    ``inbound_map`` is ``None`` (and unused) for ``"out"`` — no reverse walk ran.
    """
    if direction == "out":
        return _out_candidates(entry)
    assert inbound_map is not None  # built by _bfs whenever direction != "out"
    if direction == "in":
        return _in_candidates(entry, inbound_map)
    return _out_candidates(entry) + _in_candidates(entry, inbound_map)


def _bfs(
    config: Config, seed_id: str, depth: int, direction: str = "out"
) -> tuple[list[dict[str, Any]], list[tuple[str, str]], list[tuple[str, str]]]:
    """Shared traversal: BFS over ``related`` from ``seed_id`` to ``depth`` hops.

    ``direction`` selects the neighbour function: ``"out"`` (default) walks
    ``related`` forward exactly as before; ``"in"`` walks the whole-vault
    reverse index :func:`_inbound_index` builds *once* up front (who mentions
    this node); ``"both"`` unions them. Raises ``ValueError`` (CLI exit 2) for
    anything else — validated before the seed is even resolved.

    Returns ``(entries, edges, tree_edges)``:

    * ``entries`` — the seed-first, deduped BFS order (the shape
      :func:`build_context` returns as-is).
    * ``edges`` — the discovery edge recorded the one time each neighbour is
      first enqueued, always ``(source_id, target_id)`` in *link* direction
      regardless of which way the traversal walked to find it (see
      :func:`_neighbour_candidates`) — what :meth:`GraphResult.to_dict` reports.
    * ``tree_edges`` — the same discoveries as ``(traversal_parent,
      traversal_child)`` pairs, i.e. always ``(entry_id, neighbour_id)``
      regardless of direction. For ``"out"`` this is identical to ``edges``;
      for ``"in"``/``"both"`` an inbound discovery's *link* direction is the
      reverse of who-discovered-whom, so a second list is kept purely for
      :meth:`GraphResult.tree_lines`'s nesting — the JSON contract (``edges``)
      never bends to serve the tree renderer.

    Every caller (:func:`build_context`, :func:`graph_query`) walks the folder
    through this one function — there is no second traversal.

    Ids are de-duplicated: a *seen* set (seeded with the seed's id and extended
    at enqueue time) guarantees each id is emitted, and edged, at most once, so
    cycles terminate and diamond graphs never duplicate a shared node or its
    edge — including a diamond built from mixed directions under ``"both"``. A
    neighbour id that resolves to no file is skipped. An unresolvable **seed**
    raises :class:`SeedNotFoundError` (the CLI maps it to exit 3) in every
    direction, since seed resolution happens before any direction-specific walk.
    """
    if direction not in _DIRECTIONS:
        raise ValueError(f"invalid direction: {direction!r} (use {', '.join(_DIRECTIONS)})")

    seed = _resolve_entry(config, seed_id)
    if seed is None:
        raise SeedNotFoundError(seed_id)

    # One whole-vault reverse-``related`` pass for the entire traversal (not one
    # per visited node) — see :func:`_inbound_index`. ``depth=0`` never expands a
    # node, so skip the walk entirely when it could not matter.
    inbound_map: dict[str, list[str]] | None = None
    if direction in ("in", "both") and depth > 0:
        inbound_map = _inbound_index(config)

    result: list[dict[str, Any]] = []
    edges: list[tuple[str, str]] = []
    tree_edges: list[tuple[str, str]] = []
    seen: set[str] = {str(seed["id"]), seed_id}
    queue: deque[tuple[dict[str, Any], int]] = deque([(seed, 0)])

    while queue:
        entry, hop = queue.popleft()
        result.append(entry)
        if hop >= depth:
            continue
        entry_id = str(entry["id"])
        for key, reversed_edge in _neighbour_candidates(entry, direction, inbound_map):
            if key in seen:
                continue
            seen.add(key)
            neighbour = _resolve_entry(config, key)
            if neighbour is None:
                continue
            neighbour_id = str(neighbour["id"])
            seen.add(neighbour_id)
            edge = (neighbour_id, entry_id) if reversed_edge else (entry_id, neighbour_id)
            edges.append(edge)
            tree_edges.append((entry_id, neighbour_id))
            queue.append((neighbour, hop + 1))

    return result, edges, tree_edges


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
    :class:`SeedNotFoundError` (the CLI maps it to exit 3). Always the forward
    (``"out"``) direction — :func:`graph_query` is the surface that exposes
    ``in``/``both``.
    """
    entries, _edges, _tree_edges = _bfs(config, seed_id, depth)
    return entries


@dataclass(frozen=True, eq=False)
class GraphResult:
    """One :func:`graph_query` result — ready to render as JSON or a tree.

    ``entries`` is the same seed-first, deduped BFS order :func:`build_context`
    returns (frontmatter dict + ``path`` per node). ``edges`` is the discovery
    spanning tree :func:`_bfs` records while walking, always in link direction —
    ``(mentioner_id, mentioned_id)`` — one pair per node the *first* (and only)
    time it is discovered, exactly the tree a cycle or diamond collapses to once
    dedup applies. ``tree_edges`` is the same discoveries as ``(traversal_parent,
    traversal_child)`` pairs, used only for nesting :meth:`tree_lines`'s output;
    it equals ``edges`` for a forward-only traversal and only diverges for an
    ``in``/``both`` query, where a discovery's link direction runs opposite the
    direction the BFS walked to find it.

    :meth:`to_dict` and :meth:`tree_lines` are both pure presentations over
    these already-fetched fields — deriving either never touches disk again, so
    JSON and tree output come from the one upstream traversal.
    """

    entries: list[dict[str, Any]]
    edges: list[tuple[str, str]]
    tree_edges: list[tuple[str, str]]

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
        shape the other session-lens commands print, just nested. Nests by
        ``tree_edges`` (traversal parent → child), not ``edges`` (link
        direction) — the two agree for a forward-only query but an ``in``/
        ``both`` query can discover a node whose link direction points the
        other way, and the tree must still nest by *who found whom*.
        """
        by_id = {str(entry["id"]): entry for entry in self.entries}
        children: dict[str, list[str]] = {}
        for parent, child in self.tree_edges:
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


def graph_query(
    config: Config, seed_id: str, depth: int = 1, direction: str = "out"
) -> GraphResult:
    """Query "what's connected to ``seed_id``" — the first-class graph surface.

    Runs the exact same BFS :func:`build_context` performs (via :func:`_bfs`,
    the single shared traversal) and additionally captures the discovery
    edges, returning both as one :class:`GraphResult`. Callers render machine
    JSON (:meth:`GraphResult.to_dict`) and/or a readable tree
    (:meth:`GraphResult.tree_lines`) from that one result — never by calling
    this twice. An unresolvable seed raises :class:`SeedNotFoundError` (the CLI
    maps it to exit 3), matching :func:`build_context`, in every direction.

    ``direction`` (team-awareness/1) is ``"out"`` (default — the forward
    ``related`` walk, unchanged), ``"in"`` (:func:`inbound_ids`: who mentions
    this node — the backlink/notify view), or ``"both"`` (their union, each
    node and edge emitted once). An unrecognised value raises ``ValueError``
    (CLI exit 2). This is the *only* surface ``direction`` is exposed on —
    :func:`build_context` stays forward-only, per the three-verbs contract:
    inbound is a lens flag, not a new primitive.
    """
    entries, edges, tree_edges = _bfs(config, seed_id, depth, direction=direction)
    return GraphResult(entries=entries, edges=edges, tree_edges=tree_edges)
