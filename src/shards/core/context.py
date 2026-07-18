"""Build-context lens — a BFS over the ``related`` id graph (memory/3).

:func:`build_context` is Phase-2's read-only "pull the neighbourhood of an id"
primitive, shared by the ``shards build-context`` CLI command and (later) the
``shards_build_context`` MCP tool. Starting at a seed id it walks the ``related``
frontmatter lists breadth-first, expanding one hop at each level until it reaches
``depth`` hops from the seed (``depth=1`` = the seed plus its direct ``related``
entries; ``depth=0`` = the seed alone).

It is a *lens*, not a store, and entirely **daemon-independent**: every node is
read straight off disk via :func:`shards.core.notes.get_note` (``n-`` ids) and
:func:`shards.core.tasks.get_task` (``t-`` ids), so it behaves identically with
the daemon down. There is no degradation path and therefore no infrastructure
notice.

Each visited entry is emitted as the standard note/task frontmatter shape
(``Note`` / ``Task`` model dump) plus a ``path`` key, in BFS traversal order with
the seed first. Ids are de-duplicated by adding them to a *seen* set at enqueue
time, so cycles (``A → B → A``) terminate and diamonds (``A → {B, C} → D``) visit
and resolve ``D`` exactly once. A ``related`` id that resolves to no file (a
dangling link) is skipped silently — only an unresolvable **seed** is fatal, and
that raises :class:`SeedNotFoundError` (mapped by the CLI to exit 3).
"""

from __future__ import annotations

from collections import deque
from typing import Any

from msgspec import ValidationError

from shards.core.notes import NoteError, get_note
from shards.core.tasks import TaskError, get_task
from shards.schemas.config import Config

__all__ = ["SeedNotFoundError", "build_context"]

_TASK_PREFIX = "t-"


class SeedNotFoundError(Exception):
    """The build-context seed id resolves to no note or task (CLI exit 3).

    Only the *seed* is fatal: a dangling ``related`` id encountered mid-traversal
    is skipped, never raised. Carries the offending seed id for the CLI message.
    """

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
    seed = _resolve_entry(config, seed_id)
    if seed is None:
        raise SeedNotFoundError(seed_id)

    result: list[dict[str, Any]] = []
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
            queue.append((neighbour, hop + 1))

    return result
