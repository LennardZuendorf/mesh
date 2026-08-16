"""tasks/2 — ``task new`` / ``task update``: create + update verbs (R1).

Exercises R1 (Create / update) on the shared note/storage primitives. Create
generates a hash ``t-`` id, writes ``tasks/open/<t-id>.md`` with ``status: open``
and ``claimed_by: ~`` (``created == updated`` at birth), and records
``blocks``/``blocked_by`` verbatim — inert in v1, no readiness logic. Update
mutates only the supplied fields (priority, tags, title, blocks, blocked_by),
bumps ``updated``, and writes atomically under the per-entity ``O_EXCL`` lock,
leaving ``status``/``claimed_by``/``owner`` and unknown keys untouched.

``_resolve_task_path`` scans **both** ``tasks/open/`` and ``tasks/done/`` for a
file whose stem matches the id (id-only, never a title slug); a miss raises
``TaskNotFoundError`` (CLI exit 3). An ``--owner`` outside a non-empty
``[tasks].collections`` raises ``ValueError`` in ``create_task`` (CLI exit 2).
"""

from __future__ import annotations

import json
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

import shards.cli.__main__ as main
import shards.cli.task as task_cli
import shards.core.tasks as tasks_core
import shards.storage.locks as locks_mod
from shards.cli.__main__ import app
from shards.core.tasks import (
    TaskNotFoundError,
    create_task,
    find_duplicate_title,
    update_task,
)
from shards.schemas.config import Config, load_config
from shards.schemas.task import Task
from shards.storage.files import task_folder

_OLD = datetime(2026, 1, 1, 9, 0, 0, tzinfo=UTC)
# t- prefix + one-or-more Crockford base-32 digits (no I, L, O, U), 4+ long.
_CROCKFORD = set("0123456789ABCDEFGHJKMNPQRSTVWXYZ")


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _reload(path: Path) -> frontmatter.Post:
    return frontmatter.loads(path.read_text(encoding="utf-8"))


def _seed_task(
    vault: Path,
    *,
    task_id: str = "t-seed",
    title: str = "Seed Task",
    status: str = "open",
    priority: str | None = None,
    tags: list[str] | None = None,
    owner: str | None = "seed-agent",
    claimed_by: str | None = None,
    body: str = "Task body.",
    created: datetime = _OLD,
    updated: datetime = _OLD,
    blocks: list[str] | None = None,
    blocked_by: list[str] | None = None,
    extra: dict[str, object] | None = None,
) -> Path:
    """Write a shards task straight to disk in the folder matching its status."""
    meta: dict[str, object] = {
        "id": task_id,
        "type": "task",
        "title": title,
        "tags": list(tags or []),
        "owner": owner,
        "created": created,
        "updated": updated,
        "related": [],
        "status": status,
        "priority": priority,
        "claimed_by": claimed_by,
        "blocks": list(blocks or []),
        "blocked_by": list(blocked_by or []),
    }
    if extra:
        meta.update(extra)
    folder = task_folder(status, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


# --------------------------------------------------------------------------- #
# create_task (core)                                                           #
# --------------------------------------------------------------------------- #


def test_create_task_writes_to_open_with_defaults(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Verify NDC", body="check it")
    assert task.id.startswith("t-")
    path = task_folder("open", vault) / f"{task.id}.md"
    assert path.exists()
    assert task.status == "open"
    assert task.claimed_by is None
    assert task.blocks == []
    assert task.blocked_by == []


def test_create_task_id_shape(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Shape Check")
    body = task.id[2:]
    assert task.id.startswith("t-")
    assert len(body) >= 4
    assert set(body) <= _CROCKFORD


def test_create_task_created_equals_updated(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Fresh")
    assert task.created == task.updated


def test_create_task_default_owner_from_config(cfg: Config, vault: Path) -> None:
    # shards_config sets [core].agent = "test-agent".
    task = create_task(cfg, "Owned")
    assert task.owner == "test-agent"


def test_create_task_explicit_valid_owner(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Owned", owner="other-agent")
    assert task.owner == "other-agent"


def test_create_task_unknown_owner_raises_valueerror(cfg: Config, vault: Path) -> None:
    with pytest.raises(ValueError):
        create_task(cfg, "Ghost", owner="ghost-agent")
    # No file leaked before the raise.
    assert list((vault / "tasks").rglob("t-*.md")) == []


def test_create_task_priority_and_tags_stored(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Prioritised", priority="high", tags=["ndc", "flights"])
    assert task.priority == "high"
    assert task.tags == ["ndc", "flights"]


def test_create_task_blocks_blocked_by_recorded_inert(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Depends", blocks=["t-9xyz"], blocked_by=["t-1abc"])
    path = task_folder("open", vault) / f"{task.id}.md"
    meta = _reload(path).metadata
    assert meta["blocks"] == ["t-9xyz"]
    assert meta["blocked_by"] == ["t-1abc"]
    # Inert: the task still lands in tasks/open/ regardless of blocked_by.
    assert meta["status"] == "open"


def test_create_task_writes_only_canonical_keys(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Clean", tags=["a"], body="body")
    path = task_folder("open", vault) / f"{task.id}.md"
    meta = _reload(path).metadata
    assert set(meta) == {
        "id",
        "type",
        "title",
        "tags",
        "owner",
        "created",
        "updated",
        "related",
        "status",
        "priority",
        "claimed_by",
        "project",
        "blocks",
        "blocked_by",
    }


def test_create_task_claimed_by_serialized_as_null(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Unclaimed")
    path = task_folder("open", vault) / f"{task.id}.md"
    assert _reload(path).metadata["claimed_by"] is None


def test_create_task_resolves_wikilinks_into_related(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Linker", body="see [[n-a3f2]] and [[t-99]]")
    assert task.related == ["n-a3f2", "t-99"]


def test_create_task_uses_atomic_write(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[Path] = []
    real = tasks_core.atomic_write

    def spy(path: Path, content: str) -> None:
        calls.append(path)
        real(path, content)

    monkeypatch.setattr(tasks_core, "atomic_write", spy)
    create_task(cfg, "Atomic")
    assert calls, "create_task must route writes through storage.atomic_write"


# --------------------------------------------------------------------------- #
# _resolve_task_path (core) — id-only, scans open/ and done/                    #
# --------------------------------------------------------------------------- #


def test_resolve_task_path_finds_in_open(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-open1", status="open")
    assert tasks_core._resolve_task_path(cfg, "t-open1") == path


def test_resolve_task_path_finds_in_done(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-done1", status="done")
    assert tasks_core._resolve_task_path(cfg, "t-done1") == path


def test_resolve_task_path_is_id_only_no_slug(cfg: Config, vault: Path) -> None:
    """Resolution is by id (filename stem) only — a title slug never matches."""
    _seed_task(vault, task_id="t-abcd", title="Verify NDC")
    with pytest.raises(TaskNotFoundError):
        tasks_core._resolve_task_path(cfg, "verify-ndc")


def test_resolve_task_path_missing_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here")
    with pytest.raises(TaskNotFoundError):
        tasks_core._resolve_task_path(cfg, "t-nope")


# --------------------------------------------------------------------------- #
# update_task (core) — mutate only supplied fields, bump updated, locked        #
# --------------------------------------------------------------------------- #


def test_update_task_priority_bumps_updated(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, priority="low")
    task = update_task(cfg, "t-seed", priority="high")
    meta = _reload(path).metadata
    assert meta["priority"] == "high"
    assert task.priority == "high"
    assert meta["updated"] > _OLD
    assert meta["created"] == _OLD


def test_update_task_title(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, title="Old Title")
    update_task(cfg, "t-seed", title="New Title")
    assert _reload(path).metadata["title"] == "New Title"


def test_update_task_tags_delta(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, tags=["ndc", "stale"])
    update_task(cfg, "t-seed", tags="+flights,-stale")
    assert _reload(path).metadata["tags"] == ["ndc", "flights"]


def test_update_task_blocks_blocked_by_stored_inert(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, status="open")
    update_task(cfg, "t-seed", blocks=["t-a"], blocked_by=["t-b"])
    meta = _reload(path).metadata
    assert meta["blocks"] == ["t-a"]
    assert meta["blocked_by"] == ["t-b"]
    # Inert: recording blocked_by does not change the task's status or folder.
    assert meta["status"] == "open"
    assert path == task_folder("open", vault) / "t-seed.md"


def test_update_task_only_touches_supplied_fields(cfg: Config, vault: Path) -> None:
    """Updating priority on a *claimed* task leaves status/claimed_by/owner intact."""
    path = _seed_task(
        vault,
        status="claimed",
        owner="flights-agent",
        claimed_by="flights-agent",
        priority="low",
    )
    update_task(cfg, "t-seed", priority="high")
    meta = _reload(path).metadata
    assert meta["priority"] == "high"
    assert meta["status"] == "claimed"
    assert meta["claimed_by"] == "flights-agent"
    assert meta["owner"] == "flights-agent"


def test_update_task_roundtrips_unknown_keys(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, extra={"tolaria_pinned": True, "custom_ref": "PROJ-1"})
    update_task(cfg, "t-seed", priority="high")
    meta = _reload(path).metadata
    assert meta["tolaria_pinned"] is True
    assert meta["custom_ref"] == "PROJ-1"


def test_update_task_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault)
    with pytest.raises(TaskNotFoundError):
        update_task(cfg, "t-missing", priority="high")


def _seed_malformed(vault: Path, task_id: str = "t-bad") -> Path:
    """Write a ``t-`` id file whose frontmatter is unparseable YAML (open/).

    Mirrors ``tests/tasks/test_list_cancel.py::_seed_malformed`` — the id-only
    resolver still matches this stem (it never reads content), so the failure
    surfaces at the *content* read this unit routes through ``read_post``.
    """
    folder = task_folder("open", vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text("---\ntitle: [unclosed\nstatus: open\n---\nbody\n", encoding="utf-8")
    return path


def test_update_task_malformed_yaml_raises_not_found(cfg: Config, vault: Path) -> None:
    """A resolved-but-unreadable task's content read maps to TaskNotFoundError.

    ``_resolve_task_path`` matches on filename stem only, so a malformed target
    still resolves; the content read (routed through ``read_post``) is what
    fails here, and it maps to the same not-found contract as ``get_task``.
    """
    _seed_malformed(vault, "t-bad")
    with pytest.raises(TaskNotFoundError):
        update_task(cfg, "t-bad", priority="high")


def test_update_task_uses_atomic_write(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _seed_task(vault)
    calls: list[Path] = []
    real = tasks_core.atomic_write

    def spy(path: Path, content: str) -> None:
        calls.append(path)
        real(path, content)

    monkeypatch.setattr(tasks_core, "atomic_write", spy)
    update_task(cfg, "t-seed", priority="high")
    assert calls


def test_update_task_acquires_entity_lock(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen: list[Path] = []
    real = locks_mod.acquire

    def spy(lock_path: Path):  # type: ignore[no-untyped-def]
        seen.append(lock_path)
        return real(lock_path)

    _seed_task(vault)
    monkeypatch.setattr(locks_mod, "acquire", spy)
    update_task(cfg, "t-seed", priority="high")
    assert seen == [vault / "tasks" / ".locks" / "t-seed.lock"]


def test_update_task_resolves_inside_lock(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The entity lock is acquired *before* the path is resolved.

    A concurrent finish/cancel renames the file open→done while holding the same
    (id-derived, location-independent) entity lock. Resolving before acquiring the
    lock would strand update on a path the winner just moved (an uncaught
    ``FileNotFoundError`` → exit 1). Acquiring first closes the TOCTOU window —
    verified here by the acquire→resolve ordering.
    """
    _seed_task(vault, status="open")
    events: list[str] = []
    real_acquire = locks_mod.acquire
    real_resolve = tasks_core._resolve_task_path

    def spy_acquire(lock_path: Path):  # type: ignore[no-untyped-def]
        events.append("acquire")
        return real_acquire(lock_path)

    def spy_resolve(config: Config, task_id: str) -> Path:
        events.append("resolve")
        return real_resolve(config, task_id)

    monkeypatch.setattr(locks_mod, "acquire", spy_acquire)
    monkeypatch.setattr(tasks_core, "_resolve_task_path", spy_resolve)
    update_task(cfg, "t-seed", priority="high")
    assert events.index("acquire") < events.index("resolve")


# --------------------------------------------------------------------------- #
# CLI — shards task new                                                          #
# --------------------------------------------------------------------------- #


def test_cli_task_new_emits_created(cfg: Config, vault: Path) -> None:
    result = _invoke(["task", "new", "Verify NDC"])
    assert result.exit_code == 0, result.output
    out = result.output.strip()
    assert out.startswith("created t-")
    task_id = out.split()[1]
    assert (task_folder("open", vault) / f"{task_id}.md").exists()


def test_cli_task_new_quiet_emits_id_only(cfg: Config, vault: Path) -> None:
    result = _invoke(["--quiet", "task", "new", "Verify NDC"])
    assert result.exit_code == 0, result.output
    task_id = result.output.strip()
    assert task_id.startswith("t-")
    assert " " not in task_id


def test_cli_task_new_json_object(cfg: Config, vault: Path) -> None:
    result = _invoke(["--json", "task", "new", "Verify NDC"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["id"].startswith("t-")
    assert obj["status"] == "open"
    assert "updated" in obj


def test_cli_task_new_with_priority_and_owner(cfg: Config, vault: Path) -> None:
    result = _invoke(
        ["--quiet", "task", "new", "Prioritised", "--priority", "high", "--owner", "other-agent"]
    )
    assert result.exit_code == 0, result.output
    task_id = result.output.strip()
    meta = _reload(task_folder("open", vault) / f"{task_id}.md").metadata
    assert meta["priority"] == "high"
    assert meta["owner"] == "other-agent"


def test_cli_task_new_unknown_owner_exits_2(cfg: Config, vault: Path) -> None:
    result = _invoke(["task", "new", "Ghost", "--owner", "ghost-agent"])
    assert result.exit_code == 2, result.output
    assert list((vault / "tasks").rglob("t-*.md")) == []


def test_cli_task_new_blocks_blocked_by_stored(cfg: Config, vault: Path) -> None:
    result = _invoke(
        ["--quiet", "task", "new", "Depends", "--blocks", "t-9xyz", "--blocked-by", "t-1abc"]
    )
    assert result.exit_code == 0, result.output
    task_id = result.output.strip()
    meta = _reload(task_folder("open", vault) / f"{task_id}.md").metadata
    assert meta["blocks"] == ["t-9xyz"]
    assert meta["blocked_by"] == ["t-1abc"]


# --------------------------------------------------------------------------- #
# CLI — shards task update                                                       #
# --------------------------------------------------------------------------- #


def test_cli_task_update_priority_emits_updated(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-c7d1", priority="low")
    result = _invoke(["task", "update", "t-c7d1", "--priority", "high"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "updated t-c7d1"
    assert _reload(path).metadata["priority"] == "high"


def test_cli_task_update_not_found_exits_3(cfg: Config, vault: Path) -> None:
    _seed_task(vault)
    result = _invoke(["task", "update", "t-missing", "--priority", "high"])
    assert result.exit_code == 3, result.output


def test_cli_task_update_json_object(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-c7d1", status="claimed", claimed_by="test-agent")
    result = _invoke(["--json", "task", "update", "t-c7d1", "--priority", "high"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["id"] == "t-c7d1"
    assert obj["status"] == "claimed"
    assert "updated" in obj


# --------------------------------------------------------------------------- #
# Wiring — __main__ imports task_app rather than defining an inline stub        #
# --------------------------------------------------------------------------- #


def test_main_wires_task_app_from_cli_task() -> None:
    assert main.task_app is task_cli.task_app


def test_task_new_and_update_registered() -> None:
    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert {"new", "update"} <= names


def test_created_task_validates_as_task(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Model Check")
    path = task_folder("open", vault) / f"{task.id}.md"
    Task.model_validate(_reload(path).metadata)


# --------------------------------------------------------------------------- #
# find_duplicate_title / duplicate-title warning at create (R9)                #
# --------------------------------------------------------------------------- #


def test_find_duplicate_title_exact_match(cfg: Config, vault: Path) -> None:
    first = create_task(cfg, "Ship the Q3 report")
    assert find_duplicate_title(cfg, "Ship the Q3 report") == first.id


def test_find_duplicate_title_no_match_returns_none(cfg: Config, vault: Path) -> None:
    create_task(cfg, "Existing Task Title")
    assert find_duplicate_title(cfg, "Unrelated Task Title") is None


def test_find_duplicate_title_case_and_whitespace_do_not_collide(cfg: Config, vault: Path) -> None:
    """Mirrors ``wikilinks._title_index``'s exact-match rule — same rule the note
    side asserts (asserted, not incidental)."""
    create_task(cfg, "Ship The Report")
    assert find_duplicate_title(cfg, "ship the report") is None
    assert find_duplicate_title(cfg, "SHIP THE REPORT") is None
    assert find_duplicate_title(cfg, " Ship The Report ") is None


def test_find_duplicate_title_ignores_notes(cfg: Config, vault: Path) -> None:
    """Same-kind only: a note with the same title is invisible to the task check."""
    from shards.core.notes import create_note

    create_note(cfg, "Shared Title", body="x")
    assert find_duplicate_title(cfg, "Shared Title") is None


def test_find_duplicate_title_scans_done_too(cfg: Config, vault: Path) -> None:
    """A duplicate is detected against ``tasks/done/`` as well as ``tasks/open/``."""
    _seed_task(vault, task_id="t-finished", title="Finished Title", status="done")
    assert find_duplicate_title(cfg, "Finished Title") == "t-finished"


def test_create_task_duplicate_title_still_succeeds(cfg: Config, vault: Path) -> None:
    """Non-blocking: creating a second task with an existing title still creates
    it (no exception, id returned, file on disk) rather than refusing."""
    first = create_task(cfg, "Ship the Q3 report")
    second = create_task(cfg, "Ship the Q3 report")
    assert second.id != first.id
    assert (task_folder("open", vault) / f"{second.id}.md").exists()
    assert (task_folder("open", vault) / f"{first.id}.md").exists()


# --------------------------------------------------------------------------- #
# CLI — duplicate-title warning (R9)                                          #
# --------------------------------------------------------------------------- #


def test_cli_task_new_duplicate_title_warns_and_still_creates(cfg: Config, vault: Path) -> None:
    """Load-bearing: the create SUCCEEDS (exit 0, id on stdout, file on disk)
    *and* a warning naming the prior id lands on stderr."""
    first = _invoke(["--quiet", "task", "new", "Ship the Q3 report"])
    assert first.exit_code == 0, first.output
    first_id = first.output.strip()

    second = _invoke(["task", "new", "Ship the Q3 report"])
    assert second.exit_code == 0, second.output
    assert first_id in second.stderr
    assert "duplicate title" in second.stderr
    second_id = second.output.strip().split()[-1]
    assert (task_folder("open", vault) / f"{second_id}.md").exists()


def test_cli_task_new_unique_title_emits_no_warning(cfg: Config, vault: Path) -> None:
    result = _invoke(["task", "new", "A Wholly Unique Task Title"])
    assert result.exit_code == 0, result.output
    assert result.stderr == ""


def test_cli_task_new_duplicate_title_quiet_suppresses_warning(cfg: Config, vault: Path) -> None:
    _invoke(["--quiet", "task", "new", "Repeat Task Title"])
    second = _invoke(["--quiet", "task", "new", "Repeat Task Title"])
    assert second.exit_code == 0, second.output
    assert second.stderr == ""


def test_cli_task_new_duplicate_title_json_never_carries_warning(cfg: Config, vault: Path) -> None:
    """``--json`` payload never carries the advisory text; the warning still
    reaches stderr (only ``--quiet`` suppresses it)."""
    _invoke(["--quiet", "task", "new", "JSON Dup Task"])
    second = _invoke(["--json", "task", "new", "JSON Dup Task"])
    assert second.exit_code == 0, second.output
    obj = json.loads(second.stdout)
    assert "warning" not in json.dumps(obj)
    assert "duplicate title" in second.stderr


def test_cli_task_new_note_task_same_title_no_warning(cfg: Config, vault: Path) -> None:
    """A note and a task sharing a title do not warn (same-kind only, R9)."""
    note_result = _invoke(["note", "new", "Cross-Kind Task Title", "--body", "x"])
    assert note_result.exit_code == 0, note_result.output

    task_result = _invoke(["task", "new", "Cross-Kind Task Title"])
    assert task_result.exit_code == 0, task_result.output
    assert task_result.stderr == ""
