"""Gating spike (cli-toolset-rework/2) — unknown-frontmatter-key round-trip.

The pydantic v2 -> msgspec swap is *gated* on this test. Root ``tech.md``
Invariant 3 ("unknown frontmatter keys round-trip") is load-bearing: shards
coexists with Tolaria, which writes its own frontmatter keys, and those foreign
keys must survive a shards load/dump cycle byte-for-byte. pydantic's
``extra="allow"`` gave this for free; a msgspec ``Struct`` drops unknown fields
unless a mechanism preserves them.

This module proves that mechanism end-to-end on both the note and the task
schema, through the same ``model_validate`` -> ``model_dump`` path production
uses. It is implementation-agnostic: it passed under pydantic and must keep
passing under msgspec. If it cannot, the swap is reverted (see the task brief).
"""

from __future__ import annotations

from datetime import UTC, datetime

import frontmatter

from shards.schemas.note import Note
from shards.schemas.task import Task

# Foreign keys shards does not own — including a scalar, a string, a bool, a
# nested mapping, a list, and the adversarial case of a key *literally* named
# ``extra`` (which a naive stash field would clobber).
_FOREIGN = {
    "tolaria_pinned": True,
    "custom_ref": "PROJ-123",
    "priority_hint": 7,
    "tolaria_meta": {"nested": {"deep": "value"}},
    "aliases": ["a", "b"],
    "extra": {"reserved": "name"},
}


def _now() -> datetime:
    return datetime(2026, 7, 3, 12, 0, 0, tzinfo=UTC)


def test_note_unknown_keys_survive_model_validate_dump() -> None:
    """Every foreign key survives a ``Note.model_validate`` -> ``model_dump`` cycle."""
    payload = {
        "id": "n-a3f2",
        "type": "note",
        "title": "Has foreign keys",
        "tags": ["x"],
        "owner": None,
        "created": _now(),
        "updated": _now(),
        "related": [],
        **_FOREIGN,
    }
    dumped = Note.model_validate(payload).model_dump()
    for key, value in _FOREIGN.items():
        assert dumped[key] == value, f"foreign key {key!r} not preserved"


def test_task_unknown_keys_survive_model_validate_dump() -> None:
    """Every foreign key survives a ``Task.model_validate`` -> ``model_dump`` cycle."""
    payload = {
        "id": "t-c7d1",
        "type": "task",
        "title": "Has foreign keys",
        "tags": [],
        "owner": None,
        "created": _now(),
        "updated": _now(),
        "related": [],
        "status": "open",
        "priority": None,
        "claimed_by": None,
        "blocks": [],
        "blocked_by": [],
        **_FOREIGN,
    }
    dumped = Task.model_validate(payload).model_dump()
    for key, value in _FOREIGN.items():
        assert dumped[key] == value, f"foreign key {key!r} not preserved"


def test_note_unknown_keys_survive_on_disk_frontmatter_roundtrip() -> None:
    """A note serialized from the model re-parses with every foreign key intact.

    Exercises the real on-disk shape: model -> ``frontmatter.dumps`` -> text ->
    ``frontmatter.loads`` -> dict, asserting the foreign keys and their values
    survive the YAML round-trip unchanged (byte-for-byte in value)."""
    payload = {
        "id": "n-a3f2",
        "type": "note",
        "title": "Has foreign keys",
        "tags": [],
        "owner": None,
        "created": _now(),
        "updated": _now(),
        "related": [],
        **_FOREIGN,
    }
    note = Note.model_validate(payload)
    post = frontmatter.Post("body text")
    post.metadata = note.model_dump(mode="python")
    reparsed = frontmatter.loads(frontmatter.dumps(post))
    for key, value in _FOREIGN.items():
        assert reparsed.metadata[key] == value, f"foreign key {key!r} lost on disk round-trip"


def test_task_unknown_keys_survive_on_disk_frontmatter_roundtrip() -> None:
    """A task serialized from the model re-parses with every foreign key intact."""
    payload = {
        "id": "t-c7d1",
        "type": "task",
        "title": "Has foreign keys",
        "tags": [],
        "owner": None,
        "created": _now(),
        "updated": _now(),
        "related": [],
        "status": "open",
        "priority": None,
        "claimed_by": None,
        "blocks": [],
        "blocked_by": [],
        **_FOREIGN,
    }
    task = Task.model_validate(payload)
    post = frontmatter.Post("body text")
    post.metadata = task.model_dump(mode="python")
    reparsed = frontmatter.loads(frontmatter.dumps(post))
    for key, value in _FOREIGN.items():
        assert reparsed.metadata[key] == value, f"foreign key {key!r} lost on disk round-trip"
