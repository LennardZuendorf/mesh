"""cli-toolset-rework/4 — the task ``project:`` field + ``--project`` scoping.

A task may carry an optional ``project: <note-id>`` — a *soft* link to a
``type: project`` note (no strict validation, like a wikilink). It is a DECLARED
optional on the :class:`~shards.schemas.task.Task` schema, so it serializes like
any other known optional (``priority``/``claimed_by``): a task created without a
project writes ``project: null`` exactly as it writes ``priority: null`` today,
and a legacy/foreign task file that carries *no* ``project`` key round-trips
byte-for-byte through the read-modify-write verbs (``update``/``claim``/… mutate
the parsed metadata in place, never reserializing the model) — no ``project: null``
is injected. ``task new``/``task update`` set the field; ``task list --project``
filters to a project's tasks. No new verb.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app
from shards.core.tasks import create_task, list_tasks, update_task
from shards.schemas.config import Config, load_config
from shards.schemas.task import Task
from shards.storage.files import task_folder

_OLD = datetime(2026, 1, 1, 9, 0, 0, tzinfo=UTC)


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _reload(path: Path) -> frontmatter.Post:
    return frontmatter.loads(path.read_text(encoding="utf-8"))


def _now() -> datetime:
    return datetime(2026, 7, 3, 12, 0, 0, tzinfo=UTC)


def _seed_task(
    vault: Path,
    *,
    task_id: str = "t-seed",
    status: str = "open",
    owner: str | None = "seed-agent",
    project: str | None = None,
    body: str = "Task body.",
    extra: dict[str, object] | None = None,
) -> Path:
    """Write a shards task straight to disk. ``project`` is only added when given."""
    meta: dict[str, object] = {
        "id": task_id,
        "type": "task",
        "title": "Seed Task",
        "tags": [],
        "owner": owner,
        "created": _OLD,
        "updated": _OLD,
        "related": [],
        "status": status,
        "priority": None,
        "claimed_by": None,
        "blocks": [],
        "blocked_by": [],
    }
    if project is not None:
        meta["project"] = project
    if extra:
        meta.update(extra)
    folder = task_folder(status, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


# --------------------------------------------------------------------------- #
# Schema — declared optional, default None                                      #
# --------------------------------------------------------------------------- #


def test_task_project_defaults_none() -> None:
    task = Task(id="t-c7d1", title="x", created=_now(), updated=_now())
    assert task.project is None


def test_task_project_accepts_value() -> None:
    task = Task.model_validate(
        {"id": "t-c7d1", "title": "x", "created": _now(), "updated": _now(), "project": "n-proj"}
    )
    assert task.project == "n-proj"


def test_task_with_project_roundtrips_value() -> None:
    payload = {
        "id": "t-c7d1",
        "title": "x",
        "created": _now(),
        "updated": _now(),
        "project": "n-proj",
    }
    dumped = Task.model_validate(payload).model_dump()
    assert dumped["project"] == "n-proj"


def test_task_project_is_soft_link_any_string_accepted() -> None:
    """No strict validation: any string is accepted (soft link, like a wikilink)."""
    task = Task.model_validate(
        {
            "id": "t-1",
            "title": "x",
            "created": _now(),
            "updated": _now(),
            "project": "anything-goes",
        }
    )
    assert task.project == "anything-goes"


# --------------------------------------------------------------------------- #
# Round-trip byte-fidelity — a task WITHOUT project is unaffected               #
# --------------------------------------------------------------------------- #


def test_update_task_without_project_injects_no_key(cfg: Config, vault: Path) -> None:
    """A legacy task file that carries no ``project`` key gains none on update.

    The read-modify-write verbs mutate the parsed metadata dict in place and write
    *that* (never the model dump), so an absent optional stays absent — the
    load-bearing round-trip invariant for foreign/legacy files.
    """
    path = _seed_task(vault, task_id="t-noproj")
    assert "project" not in _reload(path).metadata  # precondition

    update_task(cfg, "t-noproj", priority="high")

    assert "project" not in _reload(path).metadata


def test_update_task_without_project_roundtrips_foreign_keys(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-f", extra={"tolaria_pinned": True, "legacy_project": "X"})
    update_task(cfg, "t-f", priority="high")
    meta = _reload(path).metadata
    assert "project" not in meta
    assert meta["tolaria_pinned"] is True
    # A foreign/legacy ``project``-shaped value under a different key round-trips.
    assert meta["legacy_project"] == "X"


def test_update_task_sets_project(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-set")
    update_task(cfg, "t-set", project="n-proj")
    assert _reload(path).metadata["project"] == "n-proj"


def test_update_task_preserves_existing_project_when_arg_absent(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-keep", project="n-proj")
    update_task(cfg, "t-keep", priority="high")
    assert _reload(path).metadata["project"] == "n-proj"


# --------------------------------------------------------------------------- #
# create_task — project stored; declared optional written like priority         #
# --------------------------------------------------------------------------- #


def test_create_task_with_project(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Scoped", project="n-proj")
    assert task.project == "n-proj"
    path = task_folder("open", vault) / f"{task.id}.md"
    assert _reload(path).metadata["project"] == "n-proj"


def test_create_task_without_project_writes_null_like_priority(cfg: Config, vault: Path) -> None:
    """A created task serializes ``project`` as null, exactly like ``priority``."""
    task = create_task(cfg, "Unscoped")
    path = task_folder("open", vault) / f"{task.id}.md"
    meta = _reload(path).metadata
    assert meta["project"] is None
    assert meta["priority"] is None  # same optional-field on-disk behaviour


# --------------------------------------------------------------------------- #
# list_tasks — --project filter                                                 #
# --------------------------------------------------------------------------- #


def test_list_tasks_filters_by_project(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a", project="n-proj")
    _seed_task(vault, task_id="t-b", project="n-proj")
    _seed_task(vault, task_id="t-c", project="n-other")
    _seed_task(vault, task_id="t-d")  # no project

    views = list_tasks(cfg, project="n-proj", limit=None)

    assert {v.task.id for v in views} == {"t-a", "t-b"}


def test_list_tasks_without_project_filter_returns_all(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a", project="n-proj")
    _seed_task(vault, task_id="t-d")
    views = list_tasks(cfg, limit=None)
    assert {v.task.id for v in views} == {"t-a", "t-d"}


# --------------------------------------------------------------------------- #
# CLI — task new / update / list --project on the EXISTING task verb            #
# --------------------------------------------------------------------------- #


def test_cli_task_new_project(cfg: Config, vault: Path) -> None:
    result = _invoke(["--quiet", "task", "new", "Scoped", "--project", "n-proj"])
    assert result.exit_code == 0, result.output
    task_id = result.output.strip()
    assert _reload(task_folder("open", vault) / f"{task_id}.md").metadata["project"] == "n-proj"


def test_cli_task_update_project(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-c7d1")
    result = _invoke(["task", "update", "t-c7d1", "--project", "n-proj"])
    assert result.exit_code == 0, result.output
    assert _reload(path).metadata["project"] == "n-proj"


def test_cli_task_list_project_filter(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a", project="n-proj")
    _seed_task(vault, task_id="t-c", project="n-other")
    result = _invoke(["--quiet", "task", "list", "--project", "n-proj"])
    assert result.exit_code == 0, result.output
    ids = [ln.strip() for ln in result.output.splitlines() if ln.strip()]
    assert ids == ["t-a"]


def test_cli_task_new_project_json_roundtrips(cfg: Config, vault: Path) -> None:
    result = _invoke(["--json", "task", "new", "Scoped", "--project", "n-proj"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["id"].startswith("t-")
    # get it back and confirm the field persisted
    got = _invoke(["--json", "task", "get", obj["id"]])
    assert json.loads(got.output)["project"] == "n-proj"
