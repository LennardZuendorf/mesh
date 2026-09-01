"""tasks/1 — Task schema and status-driven folder routing.

A task is a note (``type: task``) plus lifecycle fields. This unit pins the
msgspec model (defaults, unknown-key round-trip, status enum) and the
``task_folder`` router that maps a status to ``tasks/open/`` or ``tasks/done/``.
"""

from __future__ import annotations

from datetime import UTC, datetime
from pathlib import Path

import pytest
from msgspec import ValidationError

from mesh.core.ids import generate_task_id
from mesh.schemas.task import Task, TaskStatus
from mesh.storage.files import task_folder

# --------------------------------------------------------------------------- #
# Task schema                                                                   #
# --------------------------------------------------------------------------- #


def _now() -> datetime:
    return datetime(2026, 7, 3, 12, 0, 0, tzinfo=UTC)


def test_task_carries_required_fields_and_defaults() -> None:
    task = Task(id="t-c7d1", title="Verify NDC", created=_now(), updated=_now())
    assert task.id == "t-c7d1"
    assert task.type == "task"
    assert task.title == "Verify NDC"
    # Lifecycle defaults.
    assert task.status == "open"
    assert task.priority is None
    assert task.claimed_by is None
    assert task.blocks == []
    assert task.blocked_by == []
    # Inherited note defaults.
    assert task.tags == []
    assert task.owner is None
    assert task.related == []


def test_task_accepts_full_lifecycle_payload() -> None:
    task = Task(
        id="t-c7d1",
        title="Verify NDC",
        tags=["ndc", "flights"],
        owner="flights-agent",
        created=_now(),
        updated=_now(),
        related=["n-b1c2"],
        status="claimed",
        priority="high",
        claimed_by="flights-agent",
        blocks=["t-9xyz"],
        blocked_by=["t-1abc"],
    )
    assert task.status == "claimed"
    assert task.priority == "high"
    assert task.claimed_by == "flights-agent"
    assert task.blocks == ["t-9xyz"]
    assert task.blocked_by == ["t-1abc"]


def test_task_type_is_pinned_to_task() -> None:
    # ``type`` must be the literal "task"; any other value is rejected. msgspec
    # validates on ``model_validate`` (the frontmatter entry point), not on
    # direct construction — mirroring how production reads tasks.
    with pytest.raises(ValidationError):
        Task.model_validate(
            {"id": "t-c7d1", "type": "note", "title": "x", "created": _now(), "updated": _now()}
        )


def test_task_status_rejects_unknown_value() -> None:
    with pytest.raises(ValidationError):
        Task.model_validate(
            {
                "id": "t-c7d1",
                "title": "x",
                "created": _now(),
                "updated": _now(),
                "status": "in-progress",
            }
        )


@pytest.mark.parametrize("status", ["open", "claimed", "done", "cancelled"])
def test_task_status_accepts_all_lifecycle_values(status: TaskStatus) -> None:
    task = Task(id="t-c7d1", title="x", created=_now(), updated=_now(), status=status)
    assert task.status == status


def test_task_priority_accepts_free_form_value() -> None:
    """R5 tolerant read: ``priority`` stays ``str | None`` — no strict ``Literal``.

    ``list_tasks``/``select_tasks`` skip any row that fails ``Task.model_validate``,
    so a legacy value outside the ``high``/``normal``/``low`` write-boundary
    vocabulary must still validate here, or every listing containing that task
    would silently drop it (the spec's rejected-design failure mode).
    """
    task = Task.model_validate(
        {
            "id": "t-c7d1",
            "title": "x",
            "created": _now(),
            "updated": _now(),
            "priority": "urgent-ish",
        }
    )
    assert task.priority == "urgent-ish"


def test_task_priority_accepts_none() -> None:
    task = Task.model_validate(
        {"id": "t-c7d1", "title": "x", "created": _now(), "updated": _now(), "priority": None}
    )
    assert task.priority is None


def test_task_unknown_keys_round_trip_unchanged() -> None:
    payload = {
        "id": "t-c7d1",
        "type": "task",
        "title": "has extras",
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
        # Keys mesh does not own must survive a load/dump cycle untouched.
        "othertool_pinned": True,
        "custom_ref": "PROJ-123",
    }
    task = Task.model_validate(payload)
    dumped = task.model_dump()
    assert dumped["othertool_pinned"] is True
    assert dumped["custom_ref"] == "PROJ-123"


# --------------------------------------------------------------------------- #
# task_folder routing                                                           #
# --------------------------------------------------------------------------- #


def test_task_folder_routes_open_and_claimed_to_open(tmp_path: Path) -> None:
    base = tmp_path / "vault"
    assert task_folder("open", base) == base / "tasks" / "open"
    assert task_folder("claimed", base) == base / "tasks" / "open"


def test_task_folder_routes_done_and_cancelled_to_done(tmp_path: Path) -> None:
    base = tmp_path / "vault"
    assert task_folder("done", base) == base / "tasks" / "done"
    assert task_folder("cancelled", base) == base / "tasks" / "done"


def test_task_folder_unknown_status_raises(tmp_path: Path) -> None:
    with pytest.raises(ValueError):
        task_folder("archived", tmp_path)


# --------------------------------------------------------------------------- #
# IDs                                                                           #
# --------------------------------------------------------------------------- #


def test_generate_task_id_has_task_prefix() -> None:
    tid = generate_task_id("2026-07-03T12:00:00Z", "Verify NDC")
    assert tid.startswith("t-")
    body = tid[2:]
    assert len(body) >= 4
    crockford = set("0123456789ABCDEFGHJKMNPQRSTVWXYZ")
    assert set(body) <= crockford
