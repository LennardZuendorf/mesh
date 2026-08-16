"""memory/2 — recent-activity lens: ``core/activity.py`` + ``shards recent-activity``.

Acceptance coverage:

* **Daemon-up delegation** — ``recent_activity`` calls
  :meth:`DaemonClient.activity_recent` (mocked here) and passes its entries
  through untouched (no filter → no frontmatter re-read).
* **Daemon-down fallback** — with no daemon reachable, the same call degrades to
  :func:`shards.index.warm.scan_recent` (asserted both behaviourally over a seeded
  vault and by spying on the fallback seam).
* **``--since`` window** — applied as an *mtime* cutoff on the returned entries;
  an unparseable value raises ``ValueError`` (mapped to CLI exit 2).
* **``--mine`` / ``--owner``** — require re-reading frontmatter per entry (the
  activity row carries no ``owner``); ``--mine`` = ``owner`` **or** ``claimed_by``
  equals the configured agent. Filters run *before* the ``--limit`` display cap.
* **CLI** — ``shards recent-activity`` is a leaf command: ``--json`` emits a clean
  array of ``{id, type, title, path, mtime}``; infra notices stay on stderr and
  ``--quiet`` suppresses them.

Every test pins ``$XDG_RUNTIME_DIR`` into ``tmp_path`` (autouse) so the client's
socket path can never collide with a real shards daemon on the dev machine — the
"daemon-down" assertions depend on there being no reachable socket.
"""

from __future__ import annotations

import json
import os
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner, Result

import shards.daemon.client as daemon_client
from shards.cli.__main__ import app
from shards.core.activity import recent_activity
from shards.daemon.client import DaemonClient, DaemonError
from shards.index.warm import DEFAULT_RECENT_LIMIT
from shards.schemas.config import Config, load_config
from shards.storage.files import note_folder, task_folder

_NOW = datetime.now(UTC).timestamp()
_DAY = 86_400.0


# --------------------------------------------------------------------------- #
# Fixtures & seeding helpers                                                  #
# --------------------------------------------------------------------------- #


@pytest.fixture(autouse=True)
def _runtime_dir(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Pin ``$XDG_RUNTIME_DIR`` into tmp so the daemon socket path stays sandboxed.

    Guarantees no reachable socket in the test process, so ``activity_recent``
    always takes its daemon-down fallback unless a test explicitly mocks it.
    """
    run = tmp_path / "xdg-run"
    run.mkdir()
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(run))
    return run


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _seed_note(
    vault: Path,
    *,
    note_id: str,
    note_type: str = "note",
    title: str = "A Note",
    owner: str = "test-agent",
    body: str = "Body line.",
    mtime: float | None = None,
) -> Path:
    when = datetime.now(UTC)
    meta: dict[str, object] = {
        "id": note_id,
        "type": note_type,
        "title": title,
        "tags": [],
        "owner": owner,
        "created": when,
        "updated": when,
        "related": [],
    }
    folder = note_folder(note_type, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{note_id}.md"
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    if mtime is not None:
        os.utime(path, (mtime, mtime))
    return path


def _seed_task(
    vault: Path,
    *,
    task_id: str,
    status: str = "open",
    title: str = "Seed Task",
    owner: str = "test-agent",
    claimed_by: str | None = None,
    mtime: float | None = None,
) -> Path:
    when = datetime.now(UTC)
    meta: dict[str, object] = {
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
    folder = task_folder(status, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    post = frontmatter.Post("body")
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    if mtime is not None:
        os.utime(path, (mtime, mtime))
    return path


def _invoke(args: list[str]) -> Result:
    return CliRunner().invoke(app, args)


# --------------------------------------------------------------------------- #
# core: daemon-up delegation                                                  #
# --------------------------------------------------------------------------- #


def test_recent_activity_delegates_to_daemon_when_up(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Daemon up → entries come straight from ``DaemonClient.activity_recent``."""
    captured: dict[str, object] = {}
    entries = [
        {"id": "n-1", "type": "note", "title": "One", "path": "/vault/n-1.md", "mtime": _NOW},
        {"id": "t-2", "type": "task", "title": "Two", "path": "/vault/t-2.md", "mtime": _NOW},
    ]

    def _fake(self: DaemonClient, config: Config, limit: int = DEFAULT_RECENT_LIMIT) -> object:
        captured["config"] = config
        captured["limit"] = limit
        return {"entries": entries}

    monkeypatch.setattr(DaemonClient, "activity_recent", _fake)

    out = recent_activity(cfg, since=None, owner=None, mine=False, limit=5)

    assert out == entries  # passed through untouched (no filter → no re-read)
    assert captured["config"] is cfg
    assert captured["limit"] == 5  # unfiltered call forwards the caller's limit


# --------------------------------------------------------------------------- #
# core: daemon-down fallback → scan_recent                                     #
# --------------------------------------------------------------------------- #


def test_recent_activity_scans_when_daemon_down(cfg: Config, vault: Path) -> None:
    """No reachable daemon → a real on-disk scan surfaces the seeded files."""
    _seed_note(vault, note_id="n-a", title="Alpha")
    _seed_task(vault, task_id="t-b", title="Bravo")

    out = recent_activity(cfg, since=None, owner=None, mine=False, limit=20)

    assert {e["id"] for e in out} == {"n-a", "t-b"}
    for e in out:
        assert set(e.keys()) == {"id", "type", "title", "path", "mtime", "owner", "claimed_by"}


def test_recent_activity_fallback_is_scan_recent(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The daemon-down path routes through ``shards.index.warm.scan_recent``."""
    sentinel: list[dict[str, object]] = [
        {"id": "n-z", "type": "note", "title": "Z", "path": "/z.md", "mtime": _NOW}
    ]
    seen: dict[str, object] = {}

    def _spy(config: Config, limit: int = DEFAULT_RECENT_LIMIT) -> list[dict[str, object]]:
        seen["limit"] = limit
        return sentinel

    # activity_recent's fallback lambda resolves ``scan_recent`` in the client module.
    monkeypatch.setattr(daemon_client, "scan_recent", _spy)

    out = recent_activity(cfg, since=None, owner=None, mine=False, limit=7)

    assert out == sentinel
    assert seen["limit"] == 7


def test_recent_activity_falls_back_on_daemon_error(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A *live* daemon that answers activity.recent with a ``DaemonError`` (the
    config-less 503 stub, or a 500 from its warm-index handler) must not crash the
    lens: the accelerator is never a gate, so it scans the folder for the same rows.
    """
    _seed_note(vault, note_id="n-a", title="Alpha")
    _seed_task(vault, task_id="t-b", title="Bravo")

    def _boom(self: DaemonClient, config: Config, limit: int = DEFAULT_RECENT_LIMIT) -> object:
        raise DaemonError(503, "not yet available")

    monkeypatch.setattr(DaemonClient, "activity_recent", _boom)

    out = recent_activity(cfg, since=None, owner=None, mine=False, limit=20)

    assert {e["id"] for e in out} == {"n-a", "t-b"}  # scanned, never raised


# --------------------------------------------------------------------------- #
# core: --since mtime window                                                   #
# --------------------------------------------------------------------------- #


def test_since_filters_by_mtime(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-old", title="Old", mtime=_NOW - 10 * _DAY)
    _seed_note(vault, note_id="n-new", title="New", mtime=_NOW - 1 * _DAY)

    out = recent_activity(cfg, since="7d", owner=None, mine=False, limit=20)

    assert [e["id"] for e in out] == ["n-new"]  # the 10-day-old entry is outside 7d


def test_bad_since_raises_valueerror(cfg: Config) -> None:
    with pytest.raises(ValueError):
        recent_activity(cfg, since="not-a-date", owner=None, mine=False, limit=20)


# --------------------------------------------------------------------------- #
# core: --mine / --owner (frontmatter re-read)                                 #
# --------------------------------------------------------------------------- #


def test_mine_reads_frontmatter_owner_and_claimed_by(cfg: Config, vault: Path) -> None:
    """``--mine`` matches ``owner`` == me *or* ``claimed_by`` == me, per re-read frontmatter."""
    _seed_note(vault, note_id="n-mine", title="Mine", owner="test-agent")
    _seed_note(vault, note_id="n-other", title="Theirs", owner="other-agent")
    # Owned by someone else but claimed by me → mine via claimed_by.
    _seed_task(
        vault,
        task_id="t-claimed",
        title="Claimed",
        status="claimed",
        owner="other-agent",
        claimed_by="test-agent",
    )

    out = recent_activity(cfg, since=None, owner=None, mine=True, limit=20)

    assert {e["id"] for e in out} == {"n-mine", "t-claimed"}


def test_owner_filters_on_frontmatter_owner(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-mine", title="Mine", owner="test-agent")
    _seed_note(vault, note_id="n-other", title="Theirs", owner="other-agent")

    out = recent_activity(cfg, since=None, owner="other-agent", mine=False, limit=20)

    assert {e["id"] for e in out} == {"n-other"}


def test_filters_apply_before_limit_cap(cfg: Config, vault: Path) -> None:
    """A filter fetches unbounded then caps: ``--mine`` is not starved by newer foreign rows."""
    _seed_note(vault, note_id="n-other1", title="O1", owner="other-agent", mtime=_NOW)
    _seed_note(vault, note_id="n-other2", title="O2", owner="other-agent", mtime=_NOW - 1.0)
    _seed_note(vault, note_id="n-mine1", title="M1", owner="test-agent", mtime=_NOW - 2.0)
    _seed_note(vault, note_id="n-mine2", title="M2", owner="test-agent", mtime=_NOW - 3.0)

    out = recent_activity(cfg, since=None, owner=None, mine=True, limit=2)

    # Newest-first among *mine*, capped at 2 — the two foreign rows must not crowd them out.
    assert [e["id"] for e in out] == ["n-mine1", "n-mine2"]


# --------------------------------------------------------------------------- #
# core: identity on the row (team-awareness/6)                                 #
# --------------------------------------------------------------------------- #


def test_row_carries_peers_identity_not_callers(cfg: Config, vault: Path) -> None:
    """A peer's row carries *their* owner/claimed_by, not the calling agent's.

    ``cfg.agent`` is ``test-agent`` (the ``shards_config`` fixture); the row for a
    note owned by someone else must say so, not silently inherit the caller.
    """
    _seed_note(vault, note_id="n-peer", title="Peer's Note", owner="other-agent")
    _seed_task(
        vault,
        task_id="t-peer",
        title="Peer's Task",
        status="claimed",
        owner="other-agent",
        claimed_by="third-agent",
    )

    out = recent_activity(cfg, since=None, owner=None, mine=False, limit=20)

    rows = {e["id"]: e for e in out}
    assert rows["n-peer"]["owner"] == "other-agent"
    assert rows["n-peer"]["claimed_by"] is None  # notes never carry claimed_by
    assert rows["t-peer"]["owner"] == "other-agent"
    assert rows["t-peer"]["claimed_by"] == "third-agent"
    assert cfg.agent == "test-agent"  # sanity: neither row is the caller's identity


def test_json_dumps_survives_owner_and_claimed_by(cfg: Config, vault: Path) -> None:
    """The row (with its new identity keys) still round-trips through ``json.dumps``."""
    _seed_task(
        vault, task_id="t-x", title="X", status="claimed", owner="other-agent", claimed_by="me"
    )

    out = recent_activity(cfg, since=None, owner=None, mine=False, limit=20)

    encoded = json.dumps(out)  # no datetime leak, no TypeError
    decoded = json.loads(encoded)
    assert decoded == out


def test_mine_filter_reads_the_row_not_disk_when_owner_key_present(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """``--mine`` costs no per-row disk read once the row already carries ``owner``."""
    import shards.core.activity as activity_mod

    _seed_note(vault, note_id="n-mine", title="Mine", owner="test-agent")
    _seed_note(vault, note_id="n-other", title="Theirs", owner="other-agent")

    def _boom(path: str) -> dict[str, object] | None:
        raise AssertionError("must not re-read frontmatter when the row carries owner")

    monkeypatch.setattr(activity_mod, "_read_meta", _boom)

    out = recent_activity(cfg, since=None, owner=None, mine=True, limit=20)

    assert {e["id"] for e in out} == {"n-mine"}


def test_legacy_row_missing_owner_key_falls_back_to_disk(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A row from an older peer daemon (no ``owner`` key at all) still filters
    correctly — via a per-row frontmatter read, exactly like before this unit."""
    mine_path = _seed_note(vault, note_id="n-mine", title="Mine", owner="test-agent")
    other_path = _seed_note(vault, note_id="n-other", title="Theirs", owner="other-agent")

    legacy_entries = [
        {"id": "n-mine", "type": "note", "title": "Mine", "path": str(mine_path), "mtime": _NOW},
        {
            "id": "n-other",
            "type": "note",
            "title": "Theirs",
            "path": str(other_path),
            "mtime": _NOW,
        },
    ]
    for entry in legacy_entries:
        assert "owner" not in entry  # simulating an old daemon's reply, verbatim

    def _fake(self: DaemonClient, config: Config, limit: int = DEFAULT_RECENT_LIMIT) -> object:
        return {"entries": legacy_entries}

    monkeypatch.setattr(DaemonClient, "activity_recent", _fake)

    out = recent_activity(cfg, since=None, owner=None, mine=True, limit=20)

    assert {e["id"] for e in out} == {"n-mine"}  # resolved by re-reading disk


# --------------------------------------------------------------------------- #
# CLI: shards recent-activity                                                   #
# --------------------------------------------------------------------------- #


def test_cli_registered_as_leaf_command(cfg: Config) -> None:
    result = _invoke(["--help"])
    assert result.exit_code == 0, result.output
    assert "recent-activity" in result.stdout


def test_cli_json_shape(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", title="Alpha")
    _seed_task(vault, task_id="t-b", title="Bravo")

    result = _invoke(["recent-activity", "--json"])
    assert result.exit_code == 0, result.output

    arr = json.loads(result.stdout)  # stdout is a clean JSON array
    assert isinstance(arr, list)
    assert {e["id"] for e in arr} == {"n-a", "t-b"}
    for e in arr:
        assert set(e.keys()) == {"id", "type", "title", "path", "mtime", "owner", "claimed_by"}


def test_cli_exit_2_on_bad_since(cfg: Config, vault: Path) -> None:
    result = _invoke(["recent-activity", "--since", "garbage"])
    assert result.exit_code == 2


def test_cli_mine_filter(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-mine", title="Mine", owner="test-agent")
    _seed_note(vault, note_id="n-other", title="Theirs", owner="other-agent")

    result = _invoke(["recent-activity", "--mine", "--json"])
    assert result.exit_code == 0, result.output
    ids = {e["id"] for e in json.loads(result.stdout)}
    assert ids == {"n-mine"}


def test_cli_since_filter(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-old", title="Old", mtime=_NOW - 10 * _DAY)
    _seed_note(vault, note_id="n-new", title="New", mtime=_NOW - 1 * _DAY)

    result = _invoke(["recent-activity", "--since", "7d", "--json"])
    assert result.exit_code == 0, result.output
    ids = [e["id"] for e in json.loads(result.stdout)]
    assert ids == ["n-new"]


def test_cli_quiet_emits_ids_only_and_no_notice(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", title="Alpha")

    result = _invoke(["--quiet", "recent-activity"])
    assert result.exit_code == 0, result.output
    lines = [ln for ln in result.stdout.splitlines() if ln.strip()]
    assert lines == ["n-a"]
    assert "daemon" not in result.stderr.lower()  # notice suppressed under --quiet


def test_cli_degradation_notice_on_stderr_only(cfg: Config, vault: Path) -> None:
    """Daemon down → an informational notice on stderr, never in the JSON payload."""
    _seed_note(vault, note_id="n-a", title="Alpha")

    result = _invoke(["recent-activity", "--json"])
    assert result.exit_code == 0, result.output
    json.loads(result.stdout)  # stdout stays parseable JSON
    assert "daemon" in result.stderr.lower()


def test_cli_no_notice_when_daemon_up(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _seed_note(vault, note_id="n-a", title="Alpha")
    monkeypatch.setattr("shards.cli.session._daemon_up", lambda: True)

    result = _invoke(["recent-activity", "--json"])
    assert result.exit_code == 0, result.output
    assert "daemon" not in result.stderr.lower()


# --------------------------------------------------------------------------- #
# CLI text rows: identity carry-over (team-awareness/7)                        #
# --------------------------------------------------------------------------- #
#
# team-awareness/6 put owner/claimed_by on every activity row (JSON, MCP); its
# brief scoped it to index/warm.py + core/activity.py, leaving the human text
# rows still silent on "who did that?". team-awareness/7 closes that gap here,
# following 35f7301's ``claimed_by or "-"`` convention rather than inventing a
# second text-row style: ``id / type / owner / claimed_by / title / path``.


def test_cli_text_rows_carry_owner_and_claimed_by(cfg: Config, vault: Path) -> None:
    _seed_task(
        vault,
        task_id="t-peer",
        title="Peer's Task",
        status="claimed",
        owner="other-agent",
        claimed_by="third-agent",
    )

    result = _invoke(["recent-activity"])
    assert result.exit_code == 0, result.output

    fields = result.stdout.strip().splitlines()[0].split("\t")
    assert fields[0] == "t-peer"
    assert fields[1] == "task"
    assert fields[2] == "other-agent"
    assert fields[3] == "third-agent"
    assert fields[4] == "Peer's Task"


def test_cli_text_rows_use_dash_for_absent_claimed_by(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", title="Alpha", owner="test-agent")

    result = _invoke(["recent-activity"])
    assert result.exit_code == 0, result.output

    fields = result.stdout.strip().splitlines()[0].split("\t")
    assert fields[2] == "test-agent"
    assert fields[3] == "-"  # notes never carry claimed_by
