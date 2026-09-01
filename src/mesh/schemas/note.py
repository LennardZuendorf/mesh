"""Note frontmatter schema.

A note is Markdown with YAML frontmatter. Mesh owns a fixed set of keys; any
other keys present in the frontmatter (a user's, another tool's) must
survive a load/dump cycle unchanged (root ``tech.md`` Invariant 3). msgspec
``Struct``s drop unknown fields on decode, so :class:`_Frontmatter` restores the
invariant explicitly: it validates the *known* field subset with msgspec and
stashes every other key in an ``extra`` dict that :meth:`_Frontmatter.model_dump`
merges back on serialize. Bodies are never modelled here: agent content is inert
data, never machinery.

The two mesh schemas that carry foreign keys — :class:`Note` and
:class:`~mesh.schemas.task.Task` — inherit this base; the task schema is a note
with ``type: task`` plus lifecycle fields.
"""

from __future__ import annotations

import functools
from collections.abc import Mapping
from datetime import UTC, date, datetime
from typing import Any, Literal, Self

import msgspec

NoteType = Literal["note", "log", "decision", "reference", "project"]

# The stash field name. Excluded from the "known schema field" set so a foreign
# frontmatter key *literally* named ``extra`` is treated as unknown (stashed and
# round-tripped) rather than colliding with the field.
_STASH = "extra"


def _iso_z(value: datetime) -> str:
    """Render a datetime the way pydantic v2's JSON mode did: UTC as a ``Z`` suffix."""
    text = value.isoformat()
    return f"{text[:-6]}Z" if text.endswith("+00:00") else text


@functools.cache
def _schema_fields(cls: type[_Frontmatter]) -> frozenset[str]:
    """The known-field set for ``cls`` (excludes the stash field) — cached per class.

    ``__struct_fields__`` is fixed once a msgspec ``Struct`` subclass is defined,
    so this is safe to compute once and reuse for every :meth:`_Frontmatter.model_validate`
    call on that class rather than rebuilding the ``frozenset`` on every call.
    """
    return frozenset(cls.__struct_fields__) - {_STASH}


class _Frontmatter(msgspec.Struct, kw_only=True):
    """Base for mesh frontmatter schemas: known-field validation + unknown-key round-trip.

    ``extra`` holds every frontmatter key mesh does not own, captured on
    :meth:`model_validate` and merged back on :meth:`model_dump` so foreign keys
    survive unchanged. Subclasses declare the owned fields; they must *not*
    declare a field named ``extra``.
    """

    extra: dict[str, Any] = msgspec.field(default_factory=dict)

    @classmethod
    def model_validate(cls, data: Mapping[str, Any]) -> Self:
        """Validate the known-field subset of ``data``; stash the rest in ``extra``.

        Known fields are converted (and type-checked) by msgspec — an invalid
        value raises :class:`msgspec.ValidationError`. Every other key, including
        one named ``extra``, is preserved verbatim for a lossless round-trip.
        """
        schema_fields = _schema_fields(cls)
        known: dict[str, Any] = {}
        stash: dict[str, Any] = {}
        for key, value in data.items():
            if key not in schema_fields:
                stash[key] = value
                continue
            # A bare YAML date (``created: 2026-01-01``) parses to ``datetime.date``;
            # promote it to midnight ``datetime`` for the timestamp fields, matching
            # the prior pydantic coercion (msgspec would otherwise reject it).
            if isinstance(value, date) and not isinstance(value, datetime):
                value = datetime(value.year, value.month, value.day)
            known[key] = value
        obj = msgspec.convert(known, cls)
        obj.extra = stash
        return obj

    def model_dump(self, *, mode: str = "python") -> dict[str, Any]:
        """Serialize to a dict of owned fields plus the stashed foreign keys.

        ``mode="python"`` keeps ``datetime``/``date`` objects (for
        ``frontmatter.dumps``); ``mode="json"`` renders them as ISO strings (UTC
        as ``Z``, matching the prior pydantic behaviour) for ``json.dumps``. The
        stashed foreign keys are appended after the owned fields, exactly as
        pydantic's ``extra="allow"`` dumped them.
        """
        if mode == "json":
            data = msgspec.to_builtins(self)
            stash = data.pop(_STASH, {}) or {}
            for key in ("created", "updated"):
                current = getattr(self, key, None)
                if isinstance(current, datetime):
                    data[key] = _iso_z(current)
            data.update(stash)
            return data
        data = msgspec.to_builtins(self, builtin_types=(datetime, date))
        data.pop(_STASH, None)
        data.update(self.extra)
        return data


class Note(_Frontmatter, kw_only=True):
    """Frontmatter for a mesh-owned note.

    Owns exactly: ``id``, ``type``, ``title``, ``tags``, ``owner``, ``created``,
    ``updated``, ``related``. Unknown keys round-trip unchanged via the inherited
    ``extra`` stash. A naive ``created``/``updated`` (a bare YAML date or a
    foreign note) is read as UTC so sorting and ``--since`` never mix naive and
    aware datetimes.
    """

    id: str
    type: NoteType = "note"
    title: str
    tags: list[str] = msgspec.field(default_factory=list)
    owner: str | None = None
    created: datetime
    updated: datetime
    related: list[str] = msgspec.field(default_factory=list)

    def __post_init__(self) -> None:
        if self.created.tzinfo is None:
            self.created = self.created.replace(tzinfo=UTC)
        if self.updated.tzinfo is None:
            self.updated = self.updated.replace(tzinfo=UTC)
