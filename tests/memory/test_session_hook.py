"""memory/4 — ``shards session-start`` warm-start lens + SessionStart hook config.

``session-start`` composes two read-only lenses into a single warm-start payload
for an agent session: the recent-activity window (``recent_activity(7d, mine)``)
and the caller's live task queue (``list_tasks(mine, open|claimed)``). The two are
merged, de-duplicated by id, and ordered *tasks first* (so the session opens on
what the agent still owes) then the remaining activity entries newest-first.

Acceptance coverage:

* **two-source compose** — the command invokes ``recent_activity`` with
  ``since="7d"`` / ``mine=True`` **and** ``list_tasks`` with ``mine=True``; both
  sources land in the output (sources are mocked so the test pins the contract,
  not disk state).
* **dedupe by id** — a task id that also appears in the activity window is emitted
  exactly once (kept in the tasks section, dropped from the remainder). Dedup is
  by *id*, not by type.
* **tasks-first ordering** — every open/claimed task precedes every remaining
  activity entry.
* **remaining newest-first** — the non-task remainder is (re)sorted by ``updated``
  descending, using the activity row's ``mtime`` as the on-disk ``updated`` proxy.
  Entries are fed scrambled so the assertion proves *session-start* sorts them.
* **status filter** — only ``open`` / ``claimed`` tasks are surfaced;
  ``done`` / ``cancelled`` are dropped.
* **--meta-only** — strips the body from output (token-budget path); without it a
  task entry carries its ``body``.
* **--json** — emits a machine-readable JSON array; command name is hyphenated
  (``session-start``) and registered as a leaf command.
* **hook config** — ``hooks/session_start.json`` matches the product.md UX spec:
  a single SessionStart ``command`` hook running ``shards session-start
  --meta-only --json``.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app
from shards.core.tasks import TaskView
from shards.schemas.config import Config, load_config
from shards.schemas.task import Task

# The hook config file, located relative to this test (repo-root/hooks/…), never
# the process cwd — the suite may run from anywhere.
_HOOK_PATH = Path(__file__).resolve().parents[2] / "hooks" / "session_start.json"

_EXPECTED_HOOK: dict[str, Any] = {
    "hooks": {
        "SessionStart": [
            {"hooks": [{"type": "command", "command": "shards session-start --meta-only --json"}]}
        ]
    }
}


# --------------------------------------------------------------------------- #
# Fixtures & builders                                                          #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _task_view(
    *,
    task_id: str,
    status: str = "open",
    title: str = "A Task",
    updated: datetime | None = None,
    owner: str = "test-agent",
    claimed_by: str | None = None,
    body: str = "Task body.",
) -> TaskView:
    """Build a real :class:`TaskView` (validated ``Task`` + body + path)."""
    when = updated or datetime.now(UTC)
    task = Task.model_validate(
        {
            "id": task_id,
            "type": "task",
            "title": title,
            "tags": [],
            "owner": owner,
            "created": when,
            "updated": when,
            "related": [],
            "status": status,
            "priority": None,
            "claimed_by": claimed_by,
            "blocks": [],
            "blocked_by": [],
        }
    )
    return TaskView(task=task, body=body, path=Path(f"/vault/tasks/open/{task_id}.md"))


def _activity(
    *,
    entry_id: str,
    entry_type: str = "note",
    title: str = "A Note",
    mtime: float | None = None,
) -> dict[str, Any]:
    """A recent-activity row in the shape ``recent_activity`` returns (no body)."""
    return {
        "id": entry_id,
        "type": entry_type,
        "title": title,
        "path": f"/vault/notes/{entry_id}.md",
        "mtime": mtime if mtime is not None else datetime.now(UTC).timestamp(),
    }


def _patch_sources(
    monkeypatch: pytest.MonkeyPatch,
    *,
    activity: list[dict[str, Any]],
    tasks: list[TaskView],
    calls: dict[str, Any] | None = None,
) -> None:
    """Replace both composed lenses with fakes; optionally record their call args."""

    def _fake_recent(
        config: Config, *, since: str | None, owner: str | None, mine: bool, limit: int
    ) -> list[dict[str, Any]]:
        if calls is not None:
            calls["recent"] = {"since": since, "owner": owner, "mine": mine, "limit": limit}
        return list(activity)

    def _fake_list_tasks(config: Config, **kwargs: Any) -> list[TaskView]:
        if calls is not None:
            calls["list_tasks"] = kwargs
        return list(tasks)

    monkeypatch.setattr("shards.cli.session.recent_activity", _fake_recent)
    monkeypatch.setattr("shards.cli.session.list_tasks", _fake_list_tasks)


def _invoke(args: list[str]) -> Any:
    return CliRunner().invoke(app, args)


def _ids(entries: list[dict[str, Any]]) -> list[str]:
    return [str(e["id"]) for e in entries]


# --------------------------------------------------------------------------- #
# two-source compose                                                           #
# --------------------------------------------------------------------------- #


def test_composes_recent_activity_and_open_claimed_tasks(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Both lenses are invoked with the spec args and both feed the output."""
    calls: dict[str, Any] = {}
    _patch_sources(
        monkeypatch,
        activity=[_activity(entry_id="n-note")],
        tasks=[_task_view(task_id="t-task", status="open")],
        calls=calls,
    )

    result = _invoke(["session-start", "--json"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.stdout)

    # recent_activity(7d, mine); list_tasks(mine=True).
    assert calls["recent"]["since"] == "7d"
    assert calls["recent"]["mine"] is True
    assert calls["list_tasks"].get("mine") is True

    assert set(_ids(arr)) == {"t-task", "n-note"}


# --------------------------------------------------------------------------- #
# dedupe by id                                                                 #
# --------------------------------------------------------------------------- #


def test_task_id_present_in_activity_is_not_duplicated(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A task also present in the activity window appears once, in the task slot."""
    _patch_sources(
        monkeypatch,
        activity=[_activity(entry_id="t-dup", entry_type="task"), _activity(entry_id="n-note")],
        tasks=[_task_view(task_id="t-dup", status="claimed")],
    )

    arr = json.loads(_invoke(["session-start", "--json"]).stdout)

    assert _ids(arr).count("t-dup") == 1  # dedup is by id
    assert arr[0]["id"] == "t-dup"  # kept in the (leading) task section


# --------------------------------------------------------------------------- #
# ordering: tasks first, then remainder newest-first                           #
# --------------------------------------------------------------------------- #


def test_tasks_appear_before_remaining_activity(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Every task id precedes every remaining (non-task) activity id."""
    _patch_sources(
        monkeypatch,
        activity=[_activity(entry_id="n-a"), _activity(entry_id="n-b")],
        tasks=[
            _task_view(task_id="t-1", status="open"),
            _task_view(task_id="t-2", status="claimed"),
        ],
    )

    ids = _ids(json.loads(_invoke(["session-start", "--json"]).stdout))

    last_task = max(ids.index("t-1"), ids.index("t-2"))
    first_note = min(ids.index("n-a"), ids.index("n-b"))
    assert last_task < first_note


def test_remaining_entries_sorted_by_updated_desc(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The non-task remainder is re-sorted newest-first (mtime = on-disk updated).

    Entries are supplied out of order so passing proves session-start sorts them
    rather than echoing the mock's order.
    """
    now = datetime.now(UTC).timestamp()
    _patch_sources(
        monkeypatch,
        activity=[
            _activity(entry_id="n-mid", mtime=now - 100),
            _activity(entry_id="n-new", mtime=now),
            _activity(entry_id="n-old", mtime=now - 1000),
        ],
        tasks=[],
    )

    ids = _ids(json.loads(_invoke(["session-start", "--json"]).stdout))

    assert ids == ["n-new", "n-mid", "n-old"]


# --------------------------------------------------------------------------- #
# status filter                                                                #
# --------------------------------------------------------------------------- #


def test_only_open_and_claimed_tasks_surface(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    """done / cancelled tasks are filtered out of the task section."""
    _patch_sources(
        monkeypatch,
        activity=[],
        tasks=[
            _task_view(task_id="t-open", status="open"),
            _task_view(task_id="t-claimed", status="claimed"),
            _task_view(task_id="t-done", status="done"),
            _task_view(task_id="t-cancelled", status="cancelled"),
        ],
    )

    ids = set(_ids(json.loads(_invoke(["session-start", "--json"]).stdout)))

    assert ids == {"t-open", "t-claimed"}


# --------------------------------------------------------------------------- #
# --meta-only strips body                                                      #
# --------------------------------------------------------------------------- #


def test_full_output_carries_task_body(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_sources(
        monkeypatch,
        activity=[],
        tasks=[_task_view(task_id="t-1", status="open", body="the body text")],
    )

    arr = json.loads(_invoke(["session-start", "--json"]).stdout)

    assert arr[0]["body"] == "the body text"


def test_meta_only_strips_body(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_sources(
        monkeypatch,
        activity=[],
        tasks=[_task_view(task_id="t-1", status="open", body="the body text")],
    )

    arr = json.loads(_invoke(["session-start", "--meta-only", "--json"]).stdout)

    assert "body" not in arr[0]


# --------------------------------------------------------------------------- #
# CLI shape: JSON array + hyphenated leaf command                              #
# --------------------------------------------------------------------------- #


def test_json_output_is_an_array(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_sources(
        monkeypatch,
        activity=[_activity(entry_id="n-a")],
        tasks=[_task_view(task_id="t-1")],
    )

    arr = json.loads(_invoke(["session-start", "--json"]).stdout)

    assert isinstance(arr, list)


def test_registered_as_hyphenated_leaf_command(cfg: Config) -> None:
    result = _invoke(["--help"])
    assert result.exit_code == 0, result.output
    assert "session-start" in result.stdout


def test_command_is_invocable_by_hyphenated_name(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    _patch_sources(monkeypatch, activity=[], tasks=[])
    result = _invoke(["session-start", "--json"])
    assert result.exit_code == 0, result.output


# --------------------------------------------------------------------------- #
# hook config schema                                                           #
# --------------------------------------------------------------------------- #


def test_hook_file_exists() -> None:
    assert _HOOK_PATH.is_file(), f"missing hook config at {_HOOK_PATH}"


def test_hook_matches_product_spec() -> None:
    """Structural (whitespace-agnostic) match against the product.md UX spec."""
    text = _HOOK_PATH.read_text(encoding="utf-8")

    assert json.loads(text) == _EXPECTED_HOOK
    # The load-bearing command string, asserted on the raw text too.
    assert "shards session-start --meta-only --json" in text


def test_hook_runs_once_at_session_start() -> None:
    """Exactly one SessionStart matcher with one command-type hook entry."""
    data = json.loads(_HOOK_PATH.read_text(encoding="utf-8"))

    matchers = data["hooks"]["SessionStart"]
    assert len(matchers) == 1
    entries = matchers[0]["hooks"]
    assert len(entries) == 1
    assert entries[0]["type"] == "command"


def test_reference_datetime_helpers_are_sane() -> None:
    """Guard the test's own time math (older < newer) so ordering tests are valid."""
    older = datetime.now(UTC) - timedelta(days=2)
    newer = datetime.now(UTC)
    assert older.timestamp() < newer.timestamp()
