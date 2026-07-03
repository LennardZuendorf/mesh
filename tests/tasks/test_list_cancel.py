"""tasks/5 — list / get / cancel (R4, R5).

Exercises the task read verbs and the cancel lifecycle transition:

* :func:`brain.core.tasks.list_tasks` scans **both** ``tasks/open/`` and
  ``tasks/done/``, surfaces only files whose frontmatter validates as a
  :class:`~brain.schemas.task.Task` (``t-`` id, ``type: task``), and applies
  conjunctive ``status`` / ``owner`` / ``--mine`` / tag / ``--since`` filters with
  ``--sort`` and ``--limit`` (same semantics as notes).
* :func:`brain.core.tasks.get_task` reads one task by id from either folder into a
  :class:`~brain.core.tasks.TaskView`; not-found raises (CLI exit 3).
* :func:`brain.core.tasks.cancel_task` appends a ``## Cancelled`` section (ISO-8601
  timestamp + optional reason), sets ``status=cancelled``, bumps ``updated``, and
  moves the file to ``tasks/done/`` — all under the per-entity lock, idempotent on
  a terminal status.

Ordering tests seed *distinct* timestamps: Python's sort is stable, so ties fall
back to filesystem ``glob`` order and would be non-deterministic.
"""

from __future__ import annotations

import json
import re
from datetime import UTC, datetime, timedelta
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

import brain.cli.task as task_cli
from brain.cli.__main__ import app
from brain.core.tasks import (
    TaskNotFoundError,
    TaskView,
    _resolve_task_path,
    cancel_task,
    get_task,
    list_tasks,
)
from brain.schemas.config import Config, load_config
from brain.storage.files import task_folder

_OLD = datetime(2026, 1, 1, 9, 0, 0, tzinfo=UTC)
_ISO_UTC = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z")


@pytest.fixture
def cfg(brain_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _reload(path: Path) -> frontmatter.Post:
    return frontmatter.loads(path.read_text(encoding="utf-8"))


def _now() -> datetime:
    return datetime.now(UTC)


def _seed_task(
    vault: Path,
    *,
    task_id: str = "t-seed",
    title: str = "Seed Task",
    status: str = "open",
    priority: str | None = None,
    owner: str | None = "seed-agent",
    claimed_by: str | None = None,
    tags: list[str] | None = None,
    body: str = "Task body.",
    created: datetime = _OLD,
    updated: datetime = _OLD,
) -> Path:
    """Write a brain task straight to disk in the folder matching its status."""
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
        "blocks": [],
        "blocked_by": [],
    }
    folder = task_folder(status, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


def _seed_foreign(vault: Path, sub: str, name: str, meta: dict[str, object] | None = None) -> Path:
    """Write a non-brain Markdown file (no valid ``t-`` id / ``type: task``)."""
    folder = vault / "tasks" / sub
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{name}.md"
    path.write_text(
        frontmatter.dumps(frontmatter.Post("Foreign.", **(meta or {}))), encoding="utf-8"
    )
    return path


def _seed_malformed(vault: Path, sub: str, task_id: str) -> Path:
    """Write a ``t-`` id file whose frontmatter is unparseable YAML.

    ``frontmatter.dumps`` only ever emits valid YAML, so the corruption is written
    raw: an unterminated flow sequence (``[unclosed``) makes PyYAML raise a
    ``ParserError`` (a ``yaml.YAMLError``) on load.
    """
    folder = vault / "tasks" / sub
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text("---\ntitle: [unclosed\nstatus: open\n---\nbody\n", encoding="utf-8")
    return path


def _open_path(vault: Path, task_id: str = "t-seed") -> Path:
    return task_folder("open", vault) / f"{task_id}.md"


def _done_path(vault: Path, task_id: str = "t-seed") -> Path:
    return task_folder("done", vault) / f"{task_id}.md"


# --------------------------------------------------------------------------- #
# list_tasks (core) — brain-id/type gate + scans both folders                  #
# --------------------------------------------------------------------------- #


def test_list_scans_both_open_and_done(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=1))
    ids = {v.task.id for v in list_tasks(cfg)}
    assert ids == {"t-open", "t-done"}


def test_list_returns_taskviews(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-one", body="Hello.")
    views = list_tasks(cfg)
    assert all(isinstance(v, TaskView) for v in views)
    assert views[0].task.id == "t-one"
    assert views[0].body == "Hello."


def test_list_skips_non_task_files(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-real", title="Real")
    _seed_foreign(vault, "open", "plain")  # no frontmatter
    _seed_foreign(vault, "open", "wrong-prefix", {"id": "x-123", "type": "task", "title": "X"})
    _seed_foreign(vault, "done", "no-id", {"type": "task", "title": "No Id"})
    ids = [v.task.id for v in list_tasks(cfg)]
    assert ids == ["t-real"]


# --------------------------------------------------------------------------- #
# list_tasks (core) — conjunctive filters                                      #
# --------------------------------------------------------------------------- #


def test_list_status_exact_match(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-claimed", status="claimed", updated=_now() - timedelta(minutes=1))
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=2))
    ids = {v.task.id for v in list_tasks(cfg, status="claimed")}
    assert ids == {"t-claimed"}


def test_list_owner_exact_match(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a", owner="alice", updated=_now())
    _seed_task(vault, task_id="t-b", owner="alicia", updated=_now() - timedelta(minutes=1))
    ids = {v.task.id for v in list_tasks(cfg, owner="alice")}
    assert ids == {"t-a"}


def test_list_mine_matches_owner_or_claimed_by(cfg: Config, vault: Path) -> None:
    # config agent (conftest) == "test-agent".
    _seed_task(vault, task_id="t-owned", owner="test-agent", status="open", updated=_now())
    _seed_task(
        vault,
        task_id="t-claimed",
        owner="someone",
        claimed_by="test-agent",
        status="claimed",
        updated=_now() - timedelta(minutes=1),
    )
    _seed_task(
        vault,
        task_id="t-other",
        owner="other-agent",
        claimed_by="other-agent",
        status="claimed",
        updated=_now() - timedelta(minutes=2),
    )
    ids = {v.task.id for v in list_tasks(cfg, mine=True)}
    assert ids == {"t-owned", "t-claimed"}


def test_list_mine_and_status_conjunctive(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-owned", owner="test-agent", status="open", updated=_now())
    _seed_task(
        vault,
        task_id="t-claimed",
        owner="someone",
        claimed_by="test-agent",
        status="claimed",
        updated=_now() - timedelta(minutes=1),
    )
    ids = {v.task.id for v in list_tasks(cfg, mine=True, status="claimed")}
    assert ids == {"t-claimed"}


def test_list_tags_and_semantics(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-both", tags=["ndc", "flights"], updated=_now())
    _seed_task(vault, task_id="t-one", tags=["ndc"], updated=_now() - timedelta(minutes=1))
    ids = {v.task.id for v in list_tasks(cfg, tags=["ndc", "flights"])}
    assert ids == {"t-both"}


def test_list_tags_any_semantics(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-both", tags=["ndc", "flights"], updated=_now())
    _seed_task(vault, task_id="t-one", tags=["ndc"], updated=_now() - timedelta(minutes=1))
    _seed_task(vault, task_id="t-none", tags=["misc"], updated=_now() - timedelta(minutes=2))
    ids = {v.task.id for v in list_tasks(cfg, tags=["ndc", "flights"], any_tag=True)}
    assert ids == {"t-both", "t-one"}


def test_list_since_duration_days(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-recent", updated=_now() - timedelta(days=1))
    _seed_task(vault, task_id="t-old", updated=_now() - timedelta(days=30))
    ids = {v.task.id for v in list_tasks(cfg, since="7d")}
    assert ids == {"t-recent"}


# --------------------------------------------------------------------------- #
# list_tasks (core) — sort / limit                                             #
# --------------------------------------------------------------------------- #


def test_list_default_sort_updated_desc(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-mid", updated=_now() - timedelta(hours=2))
    _seed_task(vault, task_id="t-new", updated=_now())
    _seed_task(vault, task_id="t-old", updated=_now() - timedelta(hours=5))
    ids = [v.task.id for v in list_tasks(cfg)]
    assert ids == ["t-new", "t-mid", "t-old"]


def test_list_sort_created_desc(cfg: Config, vault: Path) -> None:
    base = datetime(2026, 1, 1, 0, 0, tzinfo=UTC)
    _seed_task(vault, task_id="t-first", created=base, updated=base)
    _seed_task(vault, task_id="t-second", created=base + timedelta(days=1), updated=base)
    _seed_task(vault, task_id="t-third", created=base + timedelta(days=2), updated=base)
    ids = [v.task.id for v in list_tasks(cfg, sort="created")]
    assert ids == ["t-third", "t-second", "t-first"]


def test_list_sort_title_asc(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-b", title="Bravo", updated=_now())
    _seed_task(vault, task_id="t-a", title="Alpha", updated=_now() - timedelta(minutes=1))
    _seed_task(vault, task_id="t-c", title="Charlie", updated=_now() - timedelta(minutes=2))
    titles = [v.task.title for v in list_tasks(cfg, sort="title")]
    assert titles == ["Alpha", "Bravo", "Charlie"]


def test_list_invalid_sort_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a")
    with pytest.raises(ValueError):
        list_tasks(cfg, sort="bogus")


def test_list_limit_caps_results(cfg: Config, vault: Path) -> None:
    for i in range(5):
        _seed_task(vault, task_id=f"t-{i:02d}", updated=_now() - timedelta(minutes=i))
    assert len(list_tasks(cfg, limit=3)) == 3


def test_list_default_limit_is_20(cfg: Config, vault: Path) -> None:
    for i in range(25):
        _seed_task(vault, task_id=f"t-{i:02d}", updated=_now() - timedelta(minutes=i))
    assert len(list_tasks(cfg)) == 20


# --------------------------------------------------------------------------- #
# get_task (core)                                                              #
# --------------------------------------------------------------------------- #


def test_get_task_returns_view(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-seed", title="Seed", body="Hello world.")
    view = get_task(cfg, "t-seed")
    assert isinstance(view, TaskView)
    assert view.task.id == "t-seed"
    assert view.task.title == "Seed"
    assert view.body == "Hello world."


def test_get_task_resolves_from_done(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-done", status="done", body="Done body.")
    view = get_task(cfg, "t-done")
    assert view.task.status == "done"
    assert view.path == _done_path(vault, "t-done").resolve()


def test_get_task_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here")
    with pytest.raises(TaskNotFoundError):
        get_task(cfg, "t-missing")


def test_get_task_malformed_yaml_raises_not_found(cfg: Config, vault: Path) -> None:
    """A ``t-`` id file with unparseable frontmatter is treated as not-found.

    The stem still resolves (resolution is id-only and never reads the body), but
    the read must not crash with a bare ``yaml.YAMLError`` traceback — it surfaces
    as :class:`TaskNotFoundError` (CLI exit 3), matching the docstring contract.
    """
    _seed_malformed(vault, "open", "t-bad")
    with pytest.raises(TaskNotFoundError):
        get_task(cfg, "t-bad")


# --------------------------------------------------------------------------- #
# list_tasks (core) — malformed YAML is skipped silently                       #
# --------------------------------------------------------------------------- #


def test_list_skips_malformed_yaml(cfg: Config, vault: Path) -> None:
    """A file with unparseable frontmatter is skipped silently, never crashing."""
    _seed_task(vault, task_id="t-good", updated=_now())
    _seed_malformed(vault, "open", "t-bad")
    _seed_malformed(vault, "done", "t-bad2")
    ids = [v.task.id for v in list_tasks(cfg)]
    assert ids == ["t-good"]


# --------------------------------------------------------------------------- #
# cancel_task (core) — append ## Cancelled + atomic move to done/              #
# --------------------------------------------------------------------------- #


def test_cancel_open_appends_section_and_moves(cfg: Config, vault: Path) -> None:
    _seed_task(vault, status="open", body="Original body.")
    task = cancel_task(cfg, "t-seed", "not needed")

    assert task.status == "cancelled"
    # The file moved: gone from open/, present in done/.
    assert not _open_path(vault).exists()
    assert _done_path(vault).exists()

    post = _reload(_done_path(vault))
    assert post.metadata["status"] == "cancelled"
    assert post.metadata["updated"] > _OLD  # bumped on the cancelling write
    assert post.metadata["created"] == _OLD  # birth instant untouched
    assert "Original body." in post.content  # body preserved
    assert "## Cancelled" in post.content
    assert _ISO_UTC.search(post.content) is not None
    assert "not needed" in post.content
    assert post.content.index("## Cancelled") < post.content.index("not needed")


def test_cancel_without_reason_still_appends_header_and_timestamp(cfg: Config, vault: Path) -> None:
    _seed_task(vault, status="open")
    task = cancel_task(cfg, "t-seed", None)
    assert task.status == "cancelled"
    content = _reload(_done_path(vault)).content
    assert "## Cancelled" in content
    assert _ISO_UTC.search(content) is not None


def test_cancel_timestamp_line_precedes_reason(cfg: Config, vault: Path) -> None:
    _seed_task(vault, status="open")
    cancel_task(cfg, "t-seed", "superseded")
    content = _reload(_done_path(vault)).content
    match = _ISO_UTC.search(content)
    assert match is not None
    assert match.start() < content.index("superseded")


def test_cancel_already_cancelled_is_idempotent(cfg: Config, vault: Path) -> None:
    body = "Task body.\n\n## Cancelled\n\n2026-01-01T09:00:00Z\nFirst reason."
    _seed_task(vault, status="cancelled", body=body, updated=_OLD)
    task = cancel_task(cfg, "t-seed", "Second reason.")

    assert task.status == "cancelled"
    assert _done_path(vault).exists()
    assert not _open_path(vault).exists()

    post = _reload(_done_path(vault))
    # No second '## Cancelled' section; no re-write (updated unchanged).
    assert post.content.count("## Cancelled") == 1
    assert "Second reason." not in post.content
    assert post.metadata["updated"] == _OLD


def test_cancel_already_cancelled_does_not_write(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """An already-cancelled cancel must not touch atomic_write (pure no-op)."""
    import brain.core.tasks as tasks_core

    _seed_task(vault, status="cancelled", body="Task body.\n\n## Cancelled\n\nx")
    calls: list[Path] = []
    monkeypatch.setattr(tasks_core, "atomic_write", lambda path, content: calls.append(path))
    cancel_task(cfg, "t-seed", "ignored")
    assert calls == []


def test_cancel_done_task_is_noop(cfg: Config, vault: Path) -> None:
    """Terminal 'done' never transitions to cancelled (terminal re-run = no-op)."""
    _seed_task(vault, status="done", body="Task body.\n\n## Outcome\n\nshipped")
    task = cancel_task(cfg, "t-seed", "too late")
    assert task.status == "done"
    post = _reload(_done_path(vault))
    assert post.metadata["status"] == "done"
    assert "## Cancelled" not in post.content


def test_cancel_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here")
    with pytest.raises(TaskNotFoundError):
        cancel_task(cfg, "t-nope", "reason")


def test_cancel_id_reresolves_from_done(cfg: Config, vault: Path) -> None:
    _seed_task(vault, status="open")
    cancel_task(cfg, "t-seed", "reason")
    resolved = _resolve_task_path(cfg, "t-seed")
    assert resolved == _done_path(vault).resolve()


# --------------------------------------------------------------------------- #
# CLI — brain task list                                                        #
# --------------------------------------------------------------------------- #


def test_cli_list_no_filters_returns_all(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=1))
    result = _invoke(["--quiet", "task", "list"])
    assert result.exit_code == 0, result.output
    assert set(result.output.split()) == {"t-open", "t-done"}


def test_cli_list_default_limit_caps(cfg: Config, vault: Path) -> None:
    for i in range(25):
        _seed_task(vault, task_id=f"t-{i:02d}", updated=_now() - timedelta(minutes=i))
    result = _invoke(["--quiet", "task", "list"])
    assert result.exit_code == 0, result.output
    assert len(result.output.split()) == 20


def test_cli_list_mine_status_json_is_array(cfg: Config, vault: Path) -> None:
    """Acceptance: brain task list --mine --status claimed --json → JSON array of tasks."""
    _seed_task(vault, task_id="t-owned", owner="test-agent", status="open", updated=_now())
    _seed_task(
        vault,
        task_id="t-mine",
        owner="someone",
        claimed_by="test-agent",
        status="claimed",
        updated=_now() - timedelta(minutes=1),
    )
    _seed_task(
        vault,
        task_id="t-other",
        owner="other-agent",
        claimed_by="other-agent",
        status="claimed",
        updated=_now() - timedelta(minutes=2),
    )
    result = _invoke(["--json", "task", "list", "--mine", "--status", "claimed"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.output)
    assert isinstance(arr, list)
    assert [o["id"] for o in arr] == ["t-mine"]
    assert arr[0]["status"] == "claimed"


def test_cli_list_quiet_one_id_per_line(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a", updated=_now())
    _seed_task(vault, task_id="t-b", updated=_now() - timedelta(minutes=1))
    result = _invoke(["--quiet", "task", "list"])
    assert result.output.split() == ["t-a", "t-b"]


def test_cli_list_status_filter(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=1))
    result = _invoke(["--quiet", "task", "list", "--status", "done"])
    assert result.output.split() == ["t-done"]


def test_cli_list_invalid_sort_exits_2(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a")
    result = _invoke(["task", "list", "--sort", "bogus"])
    assert result.exit_code == 2


def test_list_command_registered() -> None:
    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert "list" in names


# --------------------------------------------------------------------------- #
# CLI — brain task get                                                         #
# --------------------------------------------------------------------------- #


def test_cli_get_default_preview_truncates_at_200(cfg: Config, vault: Path) -> None:
    body = "A" * 250
    _seed_task(vault, task_id="t-seed", title="Seed", body=body)
    result = _invoke(["task", "get", "t-seed"])
    assert result.exit_code == 0, result.output
    assert "id: t-seed" in result.output
    assert "A" * 200 in result.output
    assert "A" * 201 not in result.output


def test_cli_get_full_shows_whole_body(cfg: Config, vault: Path) -> None:
    body = "A" * 250
    _seed_task(vault, task_id="t-seed", body=body)
    result = _invoke(["task", "get", "t-seed", "--full"])
    assert result.exit_code == 0, result.output
    assert "A" * 250 in result.output


def test_cli_get_json_is_full_model_dump(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-seed", title="Seed", tags=["x"], status="claimed", body="the body")
    result = _invoke(["--json", "task", "get", "t-seed"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert isinstance(obj, dict)
    assert obj["id"] == "t-seed"
    assert obj["title"] == "Seed"
    assert obj["type"] == "task"
    assert obj["status"] == "claimed"
    assert obj["tags"] == ["x"]


def test_cli_get_meta_only_omits_body(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-seed", body="UNIQUEBODYMARKER")
    result = _invoke(["task", "get", "t-seed", "--meta-only"])
    assert result.exit_code == 0, result.output
    assert "id: t-seed" in result.output
    assert "UNIQUEBODYMARKER" not in result.output


def test_cli_get_not_found_exits_3(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here")
    result = _invoke(["task", "get", "t-missing"])
    assert result.exit_code == 3, result.output


def test_cli_get_broken_task_file_exits_3(cfg: Config, vault: Path) -> None:
    """A t-id file with malformed frontmatter maps to exit 3, not a traceback."""
    # Valid t- stem (so it resolves) but frontmatter is missing required fields.
    path = task_folder("open", vault) / "t-broken.md"
    post = frontmatter.Post("body", id="t-broken", type="task")  # no title/created/updated
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    result = _invoke(["task", "get", "t-broken"])
    assert result.exit_code == 3, result.output
    assert result.exception is None or isinstance(result.exception, SystemExit), result.output


def test_cli_get_malformed_yaml_exits_3(cfg: Config, vault: Path) -> None:
    """A t-id file whose frontmatter is unparseable YAML maps to exit 3, not a crash."""
    _seed_malformed(vault, "open", "t-bad")
    result = _invoke(["task", "get", "t-bad"])
    assert result.exit_code == 3, result.output
    assert result.exception is None or isinstance(result.exception, SystemExit), result.output


def test_get_command_registered() -> None:
    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert "get" in names


# --------------------------------------------------------------------------- #
# CLI — brain task cancel                                                       #
# --------------------------------------------------------------------------- #


def test_cli_cancel_emits_cancelled(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="open")
    result = _invoke(["task", "cancel", "t-xxx"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "cancelled t-xxx"
    post = _reload(_done_path(vault, "t-xxx"))
    assert post.metadata["status"] == "cancelled"
    assert "## Cancelled" in post.content


def test_cli_cancel_with_reason_records_it(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="open")
    result = _invoke(["task", "cancel", "t-xxx", "--reason", "not needed"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "cancelled t-xxx"
    content = _reload(_done_path(vault, "t-xxx")).content
    assert "not needed" in content
    assert content.index("## Cancelled") < content.index("not needed")


def test_cli_cancel_idempotent_exits_0(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="cancelled", body="b\n\n## Cancelled\n\nx")
    result = _invoke(["task", "cancel", "t-xxx", "--reason", "again"])
    assert result.exit_code == 0, result.output
    post = _reload(_done_path(vault, "t-xxx"))
    assert post.content.count("## Cancelled") == 1
    assert "again" not in post.content


def test_cli_cancel_not_found_exits_3(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here", status="open")
    result = _invoke(["task", "cancel", "t-missing"])
    assert result.exit_code == 3, result.output


def test_cli_cancel_json_object(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="open")
    result = _invoke(["--json", "task", "cancel", "t-xxx"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["id"] == "t-xxx"
    assert obj["status"] == "cancelled"


def test_cancel_command_registered() -> None:
    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert "cancel" in names
