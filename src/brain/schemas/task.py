"""Task frontmatter schema.

A task is a :class:`~brain.schemas.note.Note` with ``type: task`` plus lifecycle
fields — coordination and handoff, no separate primitive. ``status`` drives the
folder the file lives in (``tasks/open/`` vs ``tasks/done/``); ``blocks`` and
``blocked_by`` are *recorded but inert* in v1 (readiness logic is deferred to the
Phase 3 dependency graph). Unknown frontmatter keys round-trip unchanged, so the
model inherits Note's ``extra='allow'`` config.
"""

from __future__ import annotations

from typing import Literal

from pydantic import Field

from brain.schemas.note import Note

TaskStatus = Literal["open", "claimed", "done", "cancelled"]


class Task(Note):
    """Frontmatter for a brain-owned task.

    Extends :class:`Note` with ``status``, ``priority``, ``claimed_by``,
    ``blocks``, and ``blocked_by``, and pins ``type`` to the literal ``"task"``.
    """

    type: Literal["task"] = "task"  # type: ignore[assignment]
    status: TaskStatus = "open"
    priority: str | None = None
    claimed_by: str | None = None
    blocks: list[str] = Field(default_factory=list)
    blocked_by: list[str] = Field(default_factory=list)
