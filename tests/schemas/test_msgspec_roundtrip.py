"""Gating spike (cli-toolset-rework/2) — unknown-frontmatter-key round-trip.

The pydantic v2 -> msgspec swap is *gated* on this test. Root ``tech.md``
Invariant 3 ("unknown frontmatter keys round-trip") is load-bearing: shards
coexists with other tools, which write their own frontmatter keys, and those
foreign keys must survive a shards load/dump cycle byte-for-byte. pydantic's
``extra="allow"`` gave this for free; a msgspec ``Struct`` drops unknown fields
unless a mechanism preserves them.

This module proves that mechanism end-to-end on both the note and the task
schema, through the same ``model_validate`` -> ``model_dump`` path production
uses. It is implementation-agnostic: it passed under pydantic and must keep
passing under msgspec. If it cannot, the swap is reverted (see the task brief).
"""

from __future__ import annotations

import json
from datetime import UTC, date, datetime

import frontmatter

from shards.schemas.note import Note
from shards.schemas.task import Task

# Foreign keys shards does not own — including a scalar, a string, a bool, a
# nested mapping, a list, and the adversarial case of a key *literally* named
# ``extra`` (which a naive stash field would clobber).
_FOREIGN = {
    "othertool_pinned": True,
    "custom_ref": "PROJ-123",
    "priority_hint": 7,
    "othertool_meta": {"nested": {"deep": "value"}},
    "aliases": ["a", "b"],
    "extra": {"reserved": "name"},
}


def _now() -> datetime:
    return datetime(2026, 7, 3, 12, 0, 0, tzinfo=UTC)


# Foreign temporal keys — the make-or-break case for the pydantic -> msgspec
# swap (cli-toolset-rework polish): a bare YAML ``date`` value and a ``datetime``
# value on keys shards does not own. ``model_validate`` never runs the known-field
# bare-date-to-datetime promotion on these (they land straight in ``extra``), so
# they must reach ``model_dump`` — and the disk round-trip — as the exact objects
# frontmatter parsed.
_FOREIGN_TEMPORAL = {
    "othertool_review_date": date(2026, 8, 1),
    "othertool_synced_at": datetime(2026, 7, 10, 9, 30, 0, tzinfo=UTC),
}


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


def test_note_foreign_temporal_keys_survive_on_disk_frontmatter_roundtrip() -> None:
    """A foreign bare-``date`` key and a foreign ``datetime`` key survive the
    real on-disk path (``model_validate`` -> ``model_dump(mode="python")`` ->
    ``frontmatter.dumps`` -> ``frontmatter.loads``) with their original values
    and types intact — the gating case Invariant 3 exists for."""
    payload = {
        "id": "n-a3f2",
        "type": "note",
        "title": "Has foreign temporal keys",
        "tags": [],
        "owner": None,
        "created": _now(),
        "updated": _now(),
        "related": [],
        **_FOREIGN_TEMPORAL,
    }
    note = Note.model_validate(payload)
    post = frontmatter.Post("body text")
    post.metadata = note.model_dump(mode="python")
    reparsed = frontmatter.loads(frontmatter.dumps(post))
    for key, value in _FOREIGN_TEMPORAL.items():
        assert reparsed.metadata[key] == value, (
            f"foreign temporal key {key!r} lost on disk round-trip"
        )
        assert type(reparsed.metadata[key]) is type(value), (
            f"foreign temporal key {key!r} changed type on disk round-trip"
        )


def test_note_foreign_temporal_keys_json_dump_is_json_dumpable() -> None:
    """``model_dump(mode="json")`` must stringify foreign temporal values so the
    result is ``json.dumps``-able without raising (the MCP/``--json`` path)."""
    payload = {
        "id": "n-a3f2",
        "type": "note",
        "title": "Has foreign temporal keys",
        "tags": [],
        "owner": None,
        "created": _now(),
        "updated": _now(),
        "related": [],
        **_FOREIGN_TEMPORAL,
    }
    note = Note.model_validate(payload)
    dumped = note.model_dump(mode="json")

    text = json.dumps(dumped)  # must not raise (foreign date/datetime stringify)

    reloaded = json.loads(text)
    assert reloaded["othertool_review_date"] == "2026-08-01"
    assert reloaded["othertool_synced_at"] == "2026-07-10T09:30:00Z"
    # The model's own created/updated fields use the same Z convention.
    assert reloaded["created"].endswith("Z")
    assert reloaded["updated"].endswith("Z")
    assert "+00:00" not in reloaded["created"]
    assert "+00:00" not in reloaded["updated"]


def test_note_bare_date_on_known_field_promotes_and_roundtrips() -> None:
    """A bare YAML ``date`` on a *known* temporal field (``created``) is promoted
    to midnight UTC ``datetime`` on ``model_validate`` (matching the prior
    pydantic coercion), and that promoted value survives the on-disk round-trip."""
    payload = {
        "id": "n-a3f2",
        "type": "note",
        "title": "Bare date on a known field",
        "tags": [],
        "owner": None,
        "created": date(2026, 7, 3),
        "updated": _now(),
        "related": [],
    }
    note = Note.model_validate(payload)
    assert note.created == datetime(2026, 7, 3, tzinfo=UTC)

    post = frontmatter.Post("body text")
    post.metadata = note.model_dump(mode="python")
    reparsed = frontmatter.loads(frontmatter.dumps(post))
    assert reparsed.metadata["created"] == datetime(2026, 7, 3, tzinfo=UTC)
