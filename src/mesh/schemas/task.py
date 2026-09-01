"""Task frontmatter schema.

A task is a :class:`~mesh.schemas.note.Note` with ``type: task`` plus lifecycle
fields — coordination and handoff, no separate primitive. ``status`` drives the
folder the file lives in (``tasks/open/`` vs ``tasks/done/``); ``blocks`` and
``blocked_by`` are *recorded but inert* in v1 (readiness logic is deferred to the
Phase 3 dependency graph). Unknown frontmatter keys round-trip unchanged via the
``extra`` stash the model inherits from :class:`~mesh.schemas.note.Note`.
"""

from __future__ import annotations

from typing import Literal

import msgspec

from mesh.schemas.note import Note

TaskStatus = Literal["open", "claimed", "done", "cancelled"]


class Task(Note, kw_only=True):
    """Frontmatter for a mesh-owned task.

    Extends :class:`Note` with ``status``, ``priority``, ``claimed_by``,
    ``project``, ``blocks``, and ``blocked_by``, and pins ``type`` to the literal
    ``"task"``. ``project`` is an optional *soft* link to a ``type: project``
    note's id — like a wikilink, it carries no strict validation (any string is
    accepted, a dangling id is tolerated). It is a declared optional, so it
    serializes like ``priority``/``claimed_by`` (written as ``null`` when unset);
    a legacy/foreign task file that omits ``project`` entirely round-trips
    untouched through the in-place-mutating read-modify-write verbs.
    """

    type: Literal["task"] = "task"
    status: TaskStatus = "open"
    priority: str | None = None
    claimed_by: str | None = None
    project: str | None = None
    blocks: list[str] = msgspec.field(default_factory=list)
    blocked_by: list[str] = msgspec.field(default_factory=list)
