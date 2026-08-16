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

team-awareness/7 widens the composite with a third source — inbound mentions of
the caller's own nodes — and two flags:

* **mentions delivery (end-to-end)** — a note by one agent linking a task
  another agent holds surfaces in the holder's payload, ``reason=mention``,
  ordered after tasks and before remaining activity.
* **exclusions** — a mention *by* me of my own node, and a mention outside the
  7-day window, are both excluded from the mentions section (a self-authored
  mention may still surface as plain ``reason=activity``).
* **dedupe precedence** — a mentioner that is also one of my own open/claimed
  tasks appears exactly once, under ``reason="task"`` (the earlier section
  wins).
* **``--owner``** drives every composed source — task queue, mention targets,
  and activity — and is honoured on both sides of the command name.
* **``--team``** widens the activity half only; the task half (and the mention
  target set built from it) stays the effective agent's own.
* **daemon parity** — the same payload, byte-identical, warm and cold, with no
  infrastructure notice either way.
"""

from __future__ import annotations

import json
import shutil
import tempfile
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

import frontmatter
import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app
from shards.core.notes import NoteView
from shards.core.tasks import TaskView
from shards.daemon.client import DaemonClient
from shards.schemas.config import Config, load_config
from shards.schemas.note import Note
from shards.schemas.task import Task
from shards.storage.files import note_folder, task_folder
from tests.daemon.conftest import running_daemon

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


def _note_view(
    *,
    note_id: str,
    title: str = "A Note",
    owner: str = "test-agent",
    body: str = "Note body.",
) -> NoteView:
    """Build a real :class:`NoteView` (validated ``Note`` + body + path)."""
    when = datetime.now(UTC)
    note = Note.model_validate(
        {
            "id": note_id,
            "type": "note",
            "title": title,
            "tags": [],
            "owner": owner,
            "created": when,
            "updated": when,
            "related": [],
        }
    )
    return NoteView(note=note, body=body, path=Path(f"/vault/notes/{note_id}.md"))


def _patch_sources(
    monkeypatch: pytest.MonkeyPatch,
    *,
    activity: list[dict[str, Any]],
    tasks: list[TaskView],
    notes: list[NoteView] | None = None,
    mentions: list[dict[str, Any]] | None = None,
    calls: dict[str, Any] | None = None,
) -> None:
    """Replace every composed lens with a fake; optionally record their call args.

    ``notes``/``mentions`` default to ``[]`` — a test that does not care about
    the mentions half (most of the pre-team-awareness/7 suite) gets an empty
    mentions section for free, never a real disk walk of the (empty) test vault.
    Every fake also records the ``config.agent`` it was called with, so a test
    can assert ``--owner``/``--team`` actually reached each source.
    """

    def _fake_recent(
        config: Config, *, since: str | None, owner: str | None, mine: bool, limit: int
    ) -> list[dict[str, Any]]:
        if calls is not None:
            calls["recent"] = {
                "since": since,
                "owner": owner,
                "mine": mine,
                "limit": limit,
                "agent": config.agent,
            }
        return list(activity)

    def _fake_task_list(_self: Any, config: Config, **kwargs: Any) -> list[TaskView]:
        if calls is not None:
            calls["list_tasks"] = {**kwargs, "agent": config.agent}
        return list(tasks)

    def _fake_note_list(_self: Any, config: Config, **kwargs: Any) -> list[NoteView]:
        if calls is not None:
            calls["list_notes"] = {**kwargs, "agent": config.agent}
        return list(notes or [])

    def _fake_mentions(
        config: Config,
        task_views: list[TaskView],
        note_views: list[NoteView],
        *,
        me: str | None,
        since: str,
    ) -> list[dict[str, Any]]:
        if calls is not None:
            calls["mentions"] = {"me": me, "since": since}
        return list(mentions or [])

    monkeypatch.setattr("shards.cli.session.recent_activity", _fake_recent)
    # The live queue and note ownership are fetched through the daemon client
    # (core-hardening/5): warm index when it is up, the identical disk walk when
    # it is down. Faking the client verbs keeps this suite about the
    # *composition*, not any one source.
    monkeypatch.setattr(DaemonClient, "task_list", _fake_task_list)
    monkeypatch.setattr(DaemonClient, "note_list", _fake_note_list)
    monkeypatch.setattr("shards.cli.session.session_mentions", _fake_mentions)


def _invoke(args: list[str]) -> Any:
    return CliRunner().invoke(app, args)


def _ids(entries: list[dict[str, Any]]) -> list[str]:
    return [str(e["id"]) for e in entries]


# --------------------------------------------------------------------------- #
# Real-vault seeding — the end-to-end delivery / dedupe / window / daemon-      #
# parity tests below need actual files an inbound scan can walk, not fakes.    #
# --------------------------------------------------------------------------- #


def _seed_note(
    vault: Path,
    *,
    note_id: str,
    title: str = "A Note",
    owner: str = "test-agent",
    related: list[str] | None = None,
    updated: datetime | None = None,
    body: str = "Body line.",
) -> Path:
    when = updated or datetime.now(UTC)
    meta: dict[str, Any] = {
        "id": note_id,
        "type": "note",
        "title": title,
        "tags": [],
        "owner": owner,
        "created": when,
        "updated": when,
        "related": list(related or []),
    }
    folder = note_folder("note", vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{note_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


def _seed_task(
    vault: Path,
    *,
    task_id: str,
    title: str = "A Task",
    status: str = "open",
    owner: str = "test-agent",
    claimed_by: str | None = None,
    related: list[str] | None = None,
    updated: datetime | None = None,
    body: str = "Task body.",
) -> Path:
    when = updated or datetime.now(UTC)
    meta: dict[str, Any] = {
        "id": task_id,
        "type": "task",
        "title": title,
        "tags": [],
        "owner": owner,
        "created": when,
        "updated": when,
        "related": list(related or []),
        "status": status,
        "priority": None,
        "claimed_by": claimed_by,
        "blocks": [],
        "blocked_by": [],
    }
    folder = task_folder(status, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


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


def test_full_output_carries_task_body(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Without ``--meta-only`` each live task carries its body, read off disk.

    core-hardening/5: a *list* row no longer carries a body (the warm index holds
    frontmatter only), so the composite reads the body per surviving task from the
    row's ``path``. The task file therefore has to exist on disk — which is the
    behaviour under test, not a fixture detail.
    """
    view = _task_view(task_id="t-1", status="open", body="the body text")
    path = vault / "tasks" / "open" / "t-1.md"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        frontmatter.dumps(frontmatter.Post("the body text", **view.task.model_dump())),
        encoding="utf-8",
    )
    _patch_sources(
        monkeypatch,
        activity=[],
        tasks=[TaskView(task=view.task, body="", path=path)],
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
# team-awareness/7 — end-to-end mention delivery (the headline claim)          #
# --------------------------------------------------------------------------- #


def test_mention_delivered_across_two_agent_identities(cfg: Config, vault: Path) -> None:
    """A note by one agent linking a task another agent holds reaches that agent.

    The simulation case the unit exists to fix: research-agent writes a note
    that mentions a task flights-agent holds (owned by a third party,
    ops-agent, and claimed by flights-agent — so "holds" exercises the
    ``claimed_by`` half of "my nodes", not just ``owner``). Nothing ever writes
    to the task itself; the mention is only findable by inverting ``related``.
    ``--owner`` puts the CLI in flights-agent's seat from a session whose own
    configured identity (``test-agent``, from ``shards_config``) is a third
    identity again — four distinct agents appear across this module's fixtures.
    """
    _seed_task(
        vault,
        task_id="t-184g",
        title="Book flights",
        status="open",
        owner="ops-agent",
        claimed_by="flights-agent",
    )
    _seed_note(
        vault,
        note_id="n-9qq2",
        title="Overlap heads-up",
        owner="research-agent",
        related=["t-184g"],
    )

    result = _invoke(["session-start", "--owner", "flights-agent", "--json"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.stdout)

    by_id = {e["id"]: e for e in arr}
    assert by_id["t-184g"]["reason"] == "task"
    assert by_id["n-9qq2"]["reason"] == "mention"
    ids = _ids(arr)
    assert ids.index("t-184g") < ids.index("n-9qq2")  # tasks before mentions


def test_mentions_ordered_between_tasks_and_activity(cfg: Config, vault: Path) -> None:
    """Full section order: tasks, then mentions, then remaining activity."""
    _seed_task(vault, task_id="t-a", status="open", owner="flights-agent")
    _seed_note(vault, note_id="n-mention", owner="research-agent", related=["t-a"])
    # A task of mine that is not open/claimed: absent from the task section,
    # present in the activity remainder instead.
    _seed_task(vault, task_id="t-done", status="done", owner="flights-agent")

    result = _invoke(["session-start", "--owner", "flights-agent", "--json"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.stdout)

    reasons = {e["id"]: e["reason"] for e in arr}
    assert reasons["t-a"] == "task"
    assert reasons["n-mention"] == "mention"
    assert reasons["t-done"] == "activity"
    ids = _ids(arr)
    assert ids.index("t-a") < ids.index("n-mention") < ids.index("t-done")


# --------------------------------------------------------------------------- #
# team-awareness/7 — mention exclusions                                        #
# --------------------------------------------------------------------------- #


def test_self_authored_mention_is_excluded(cfg: Config, vault: Path) -> None:
    """A mention *by* me of my own node is not surfaced as a mention.

    It may still appear under ``reason="activity"`` (it is a real recent change
    of mine) — only the mentions section excludes it, per R7's "mentions by me
    of my own nodes are excluded".
    """
    _seed_task(vault, task_id="t-a", status="open", owner="flights-agent")
    _seed_note(vault, note_id="n-self", owner="flights-agent", related=["t-a"])

    result = _invoke(["session-start", "--owner", "flights-agent", "--json"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.stdout)

    assert {e["id"]: e["reason"] for e in arr}.get("n-self") == "activity"


def test_mention_outside_window_is_excluded(cfg: Config, vault: Path) -> None:
    """A mentioner last touched outside the 7-day window never surfaces at all."""
    _seed_task(vault, task_id="t-a", status="open", owner="flights-agent")
    _seed_note(
        vault,
        note_id="n-old",
        owner="research-agent",
        related=["t-a"],
        updated=datetime.now(UTC) - timedelta(days=10),
    )

    result = _invoke(["session-start", "--owner", "flights-agent", "--json"])
    assert result.exit_code == 0, result.output
    assert "n-old" not in _ids(json.loads(result.stdout))


# --------------------------------------------------------------------------- #
# team-awareness/7 — dedupe precedence across all three sections               #
# --------------------------------------------------------------------------- #


def test_mention_matching_own_task_appears_once_under_task(cfg: Config, vault: Path) -> None:
    """A mentioner that is also one of my own open/claimed tasks: one entry, ``task``.

    Dedupe precedence: tasks are composed first (unchanged from before this
    unit), so a node that is *both* one of my open/claimed tasks *and* an
    inbound mentioner of another of my nodes keeps its earlier, task-section
    slot rather than appearing a second time as a mention.
    """
    _seed_task(vault, task_id="t-target", status="open", owner="flights-agent")
    _seed_task(
        vault,
        task_id="t-x",
        status="open",
        owner="ops-agent",
        claimed_by="flights-agent",
        related=["t-target"],
    )

    result = _invoke(["session-start", "--owner", "flights-agent", "--json"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.stdout)

    matches = [e for e in arr if e["id"] == "t-x"]
    assert len(matches) == 1
    assert matches[0]["reason"] == "task"


# --------------------------------------------------------------------------- #
# team-awareness/7 — --team widens activity, never the task queue              #
# --------------------------------------------------------------------------- #


def test_team_widens_activity_half_only(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    calls: dict[str, Any] = {}
    _patch_sources(monkeypatch, activity=[], tasks=[], calls=calls)

    result = _invoke(["session-start", "--team", "--json"])
    assert result.exit_code == 0, result.output

    assert calls["list_tasks"]["mine"] is True  # task half stays mine
    assert calls["recent"]["mine"] is False  # activity half widens


def test_without_team_activity_stays_mine(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    calls: dict[str, Any] = {}
    _patch_sources(monkeypatch, activity=[], tasks=[], calls=calls)

    result = _invoke(["session-start", "--json"])
    assert result.exit_code == 0, result.output

    assert calls["recent"]["mine"] is True


# --------------------------------------------------------------------------- #
# team-awareness/7 — --owner drives every composed source                      #
# --------------------------------------------------------------------------- #


def test_owner_flag_drives_every_source(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    calls: dict[str, Any] = {}
    _patch_sources(monkeypatch, activity=[], tasks=[], calls=calls)

    result = _invoke(["session-start", "--owner", "flights-agent", "--json"])
    assert result.exit_code == 0, result.output

    assert calls["list_tasks"]["agent"] == "flights-agent"
    assert calls["list_notes"]["agent"] == "flights-agent"
    assert calls["recent"]["agent"] == "flights-agent"
    assert calls["mentions"]["me"] == "flights-agent"


def test_owner_honoured_on_root_side_of_command_name(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """``shards --owner X session-start`` is equivalent to ``session-start --owner X``."""
    calls: dict[str, Any] = {}
    _patch_sources(monkeypatch, activity=[], tasks=[], calls=calls)

    result = _invoke(["--owner", "flights-agent", "session-start", "--json"])
    assert result.exit_code == 0, result.output

    assert calls["list_tasks"]["agent"] == "flights-agent"


def test_no_owner_flag_uses_configured_agent(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    calls: dict[str, Any] = {}
    _patch_sources(monkeypatch, activity=[], tasks=[], calls=calls)

    result = _invoke(["session-start", "--json"])
    assert result.exit_code == 0, result.output

    assert calls["list_tasks"]["agent"] == cfg.agent == "test-agent"


# --------------------------------------------------------------------------- #
# team-awareness/7 — mentions carry no body under --meta-only                  #
# --------------------------------------------------------------------------- #


def test_meta_only_mention_entries_have_no_body(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    mention = {
        "id": "n-m",
        "type": "note",
        "title": "Mention",
        "path": "/vault/notes/n-m.md",
        "owner": "research-agent",
        "claimed_by": None,
        "updated": datetime.now(UTC).isoformat(),
    }
    _patch_sources(monkeypatch, activity=[], tasks=[], mentions=[mention])

    arr = json.loads(_invoke(["session-start", "--meta-only", "--json"]).stdout)

    entry = next(e for e in arr if e["id"] == "n-m")
    assert entry["reason"] == "mention"
    assert "body" not in entry


def test_full_output_mention_entries_have_no_body_either(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Mentions never carry a body — even *without* ``--meta-only``.

    ``_resolve_entry`` reads frontmatter only; there is no body-reading branch
    for the mentions section to skip in the first place.
    """
    mention = {
        "id": "n-m",
        "type": "note",
        "title": "Mention",
        "path": "/vault/notes/n-m.md",
        "owner": "research-agent",
        "claimed_by": None,
        "updated": datetime.now(UTC).isoformat(),
    }
    _patch_sources(monkeypatch, activity=[], tasks=[], mentions=[mention])

    arr = json.loads(_invoke(["session-start", "--json"]).stdout)

    assert "body" not in next(e for e in arr if e["id"] == "n-m")


# --------------------------------------------------------------------------- #
# team-awareness/7 — daemon parity: mentions are daemon-free by construction   #
# --------------------------------------------------------------------------- #


def test_daemon_up_and_down_produce_identical_payload_with_a_mention(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A real vault with a live mention: identical JSON, no notice, warm or cold."""
    _seed_task(vault, task_id="t-target", status="open", claimed_by="test-agent")
    _seed_note(vault, note_id="n-mentioner", owner="other-agent", related=["t-target"])

    sock_root = Path(tempfile.mkdtemp(prefix="ses-", dir="/tmp"))
    try:
        cold_dir = sock_root / "cold"
        cold_dir.mkdir()
        monkeypatch.setenv("XDG_RUNTIME_DIR", str(cold_dir))
        cold = _invoke(["session-start", "--json"])
        assert cold.exit_code == 0, cold.output

        warm_dir = sock_root / "warm"
        warm_dir.mkdir()
        with running_daemon(warm_dir / "shards.sock", config=cfg):
            monkeypatch.setenv("XDG_RUNTIME_DIR", str(warm_dir))
            warm = _invoke(["session-start", "--json"])
        assert warm.exit_code == 0, warm.output

        assert warm.stdout == cold.stdout
        assert json.loads(cold.stdout)  # sanity: the mention actually landed
        assert "daemon" not in cold.stderr.lower()
        assert "daemon" not in warm.stderr.lower()
    finally:
        shutil.rmtree(sock_root, ignore_errors=True)


# --------------------------------------------------------------------------- #
# team-awareness/7 carry-over — identity in session-start's text rows          #
# --------------------------------------------------------------------------- #


def test_text_rows_carry_reason_and_identity(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    """``id / type / reason / owner / claimed_by / title / path`` — one convention.

    Follows ``35f7301``'s ``claimed_by or "-"`` fallback rather than inventing a
    second text-row style (the carry-over this unit owns: ``recent-activity``'s
    text rows, and now ``session-start``'s, both render identity).
    """
    _patch_sources(
        monkeypatch,
        activity=[],
        tasks=[
            _task_view(task_id="t-1", status="open", owner="ops-agent", claimed_by="test-agent")
        ],
    )

    result = _invoke(["session-start"])
    assert result.exit_code == 0, result.output

    fields = result.stdout.strip().splitlines()[0].split("\t")
    assert fields[0] == "t-1"
    assert fields[2] == "task"
    assert fields[3] == "ops-agent"
    assert fields[4] == "test-agent"


def test_text_rows_use_dash_for_absent_claimed_by(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    _patch_sources(monkeypatch, activity=[], tasks=[_task_view(task_id="t-1", status="open")])

    result = _invoke(["session-start"])
    assert result.exit_code == 0, result.output

    fields = result.stdout.strip().splitlines()[0].split("\t")
    assert fields[4] == "-"  # unclaimed


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
