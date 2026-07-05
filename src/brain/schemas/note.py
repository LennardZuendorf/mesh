"""Note frontmatter schema.

A note is Markdown with YAML frontmatter. Brain owns a fixed set of keys; any
other keys present in the frontmatter (Tolaria's, a user's, another tool's) must
survive a load/dump cycle unchanged — hence ``extra='allow'``. Bodies are never
modelled here: agent content is inert data, never machinery.

The task schema (a later unit) is a note with ``type: task`` plus lifecycle
fields; it will extend this model.
"""

from __future__ import annotations

from datetime import UTC, datetime
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator

NoteType = Literal["note", "log", "decision", "reference"]


class Note(BaseModel):
    """Frontmatter for a brain-owned note.

    Owns exactly: ``id``, ``type``, ``title``, ``tags``, ``owner``, ``created``,
    ``updated``, ``related``. Unknown keys round-trip unchanged.
    """

    model_config = ConfigDict(extra="allow")

    id: str
    type: NoteType = "note"
    title: str
    tags: list[str] = Field(default_factory=list)
    owner: str | None = None
    created: datetime
    updated: datetime
    related: list[str] = Field(default_factory=list)

    @field_validator("created", "updated", mode="after")
    @classmethod
    def _as_aware_utc(cls, value: datetime) -> datetime:
        """Read a naive timestamp (e.g. a bare ``date`` or a foreign note) as UTC,
        so sorting and ``--since`` never mix naive and aware datetimes."""
        return value.replace(tzinfo=UTC) if value.tzinfo is None else value
