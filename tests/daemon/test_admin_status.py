"""daemon/3 — Admin: daemon lifecycle, ``shards status``, ``reindex``.

Three surfaces, matching the unit's acceptance criteria (spec R4):

* **Daemon lifecycle** — ``daemon start|stop|status`` over a PID state file that
  lives beside the socket (``$XDG_RUNTIME_DIR/shards.pid``). ``start`` is idempotent
  (a live PID means "already running", no second spawn); ``stop`` is idempotent
  (no PID file means "not running"). The PID file is written atomically. The heavy
  process spawn / signal are exercised via seams (``spawn_daemon`` /
  ``terminate_process``) rather than forking a real daemon inside the
  multi-threaded test process.
* **``shards status``** — vault health by *direct scan* (works daemon-down): note
  count, tasks-by-status, freshness (newest mtime + age), dangling wikilinks, and
  stale ``O_EXCL`` locks (PID dead **or** age > 300 s). Read-only: it never bumps
  ``updated`` nor rewrites a file.
* **``reindex``** — delegates to the search feature's ``indexed`` client
  (``indexed_client.reindex``); now that the client is built, the delegate is
  invoked (the subprocess seam is mocked here so no real ``indexed`` runs).

PID-path resolution reads ``$XDG_RUNTIME_DIR``; every test that touches it pins the
runtime dir into ``tmp_path`` so no real ``~/.shards/run`` file is ever written.
"""

from __future__ import annotations

import json
import os
import time
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

import frontmatter
import pytest
from typer.testing import CliRunner, Result

from shards.cli.__main__ import app
from shards.cli.admin import (
    _agent_breakdown,
    daemon_running,
    default_pid_path,
    read_pid,
    write_pid,
)
from shards.core.lenses import scan_stale_locks
from shards.core.tasks import list_tasks
from shards.daemon.client import DaemonClient, default_socket_path
from shards.schemas.config import Config, load_config
from shards.storage.files import note_folder, task_folder
from shards.storage.locks import LOCK_TTL_SECONDS

_STALE_AGE = LOCK_TTL_SECONDS + 100.0  # comfortably past the 300 s TTL


def vault_status(config: Config) -> dict[str, Any]:
    """The daemon-down ``vault.status`` payload: the client's own file-op fallback.

    core-hardening/5 moved the report assembly into
    :func:`shards.core.lenses.status_report` and wired ``shards status`` through
    :meth:`DaemonClient.vault_status`. Pointing this helper at a socket that
    cannot exist keeps every assertion below on the *fallback* path — the one the
    original direct-scan tests pinned — while exercising the shipped code path
    rather than a retired helper.
    """
    return DaemonClient(socket_path=Path("/nonexistent/shards-status.sock")).vault_status(config)


# --------------------------------------------------------------------------- #
# Fixtures & helpers                                                          #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


@pytest.fixture
def runtime_dir(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Pin ``$XDG_RUNTIME_DIR`` into tmp so the PID/socket paths stay sandboxed."""
    run = tmp_path / "xdg-run"
    run.mkdir()
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(run))
    return run


def _invoke(args: list[str]) -> Result:
    return CliRunner().invoke(app, args)


def _find_dead_pid() -> int:
    """Return a PID that is not currently alive (mirrors tests/notes/test_storage)."""
    for candidate in range(999_999, 900_000, -1):
        try:
            os.kill(candidate, 0)
        except ProcessLookupError:
            return candidate
        except PermissionError:  # pragma: no cover - alive but not ours
            continue
    raise RuntimeError("could not find a dead pid")  # pragma: no cover


def _seed_note(
    vault: Path,
    *,
    note_id: str,
    note_type: str = "note",
    title: str = "A Note",
    body: str = "Body line.",
) -> Path:
    when = datetime.now(UTC)
    meta: dict[str, object] = {
        "id": note_id,
        "type": note_type,
        "title": title,
        "tags": [],
        "owner": "seed-agent",
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
    return path


def _seed_task(
    vault: Path,
    *,
    task_id: str,
    status: str = "open",
    title: str = "Seed Task",
    body: str = "t",
    owner: str = "seed-agent",
    claimed_by: str | None = None,
    updated: datetime | None = None,
) -> Path:
    when = updated if updated is not None else datetime.now(UTC)
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
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def _write_lock(vault: Path, kind: str, name: str, *, pid: int, age: float = 0.0) -> Path:
    """Create ``<kind>/.locks/<name>.lock`` holding ``pid``, aged by ``age`` seconds."""
    locks = vault / kind / ".locks"
    locks.mkdir(parents=True, exist_ok=True)
    lock = locks / f"{name}.lock"
    lock.write_text(f"{pid}\n", encoding="utf-8")
    if age:
        old = time.time() - age
        os.utime(lock, (old, old))
    return lock


# --------------------------------------------------------------------------- #
# PID path resolution                                                         #
# --------------------------------------------------------------------------- #


def test_default_pid_path_uses_xdg_runtime_dir(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    assert default_pid_path() == tmp_path / "shards.pid"


def test_default_pid_path_falls_back_to_shards_run(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("XDG_RUNTIME_DIR", raising=False)
    assert default_pid_path() == Path.home() / ".shards" / "run" / "shards.pid"


# --------------------------------------------------------------------------- #
# PID file read/write (atomic) + liveness                                     #
# --------------------------------------------------------------------------- #


def test_write_pid_roundtrips_and_leaves_only_the_pid(tmp_path: Path) -> None:
    pid_path = tmp_path / "shards.pid"
    write_pid(pid_path, 4242)
    assert read_pid(pid_path) == 4242
    assert pid_path.read_text(encoding="utf-8").strip() == "4242"
    # Atomic write leaves no sibling temp file behind.
    assert list(tmp_path.iterdir()) == [pid_path]


def test_read_pid_missing_returns_none(tmp_path: Path) -> None:
    assert read_pid(tmp_path / "absent.pid") is None


def test_read_pid_malformed_returns_none(tmp_path: Path) -> None:
    pid_path = tmp_path / "shards.pid"
    pid_path.write_text("not-a-pid\n", encoding="utf-8")
    assert read_pid(pid_path) is None


def test_daemon_running_true_for_live_pid(tmp_path: Path) -> None:
    pid_path = tmp_path / "shards.pid"
    write_pid(pid_path, os.getpid())
    assert daemon_running(pid_path) == os.getpid()


def test_daemon_running_none_for_dead_pid(tmp_path: Path) -> None:
    pid_path = tmp_path / "shards.pid"
    write_pid(pid_path, _find_dead_pid())
    assert daemon_running(pid_path) is None


def test_daemon_running_none_without_pid_file(tmp_path: Path) -> None:
    assert daemon_running(tmp_path / "absent.pid") is None


# --------------------------------------------------------------------------- #
# vault_status — counts, freshness, dangling links                            #
# --------------------------------------------------------------------------- #


def test_vault_status_counts_notes_and_tasks_by_status(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", title="A")
    _seed_note(vault, note_id="n-b", title="B", note_type="decision")
    _seed_task(vault, task_id="t-o1", status="open")
    _seed_task(vault, task_id="t-o2", status="open")
    _seed_task(vault, task_id="t-d1", status="done")
    status = vault_status(cfg)
    assert status["notes"] == 2
    assert status["tasks"]["open"] == 2
    assert status["tasks"]["done"] == 1
    # Zero-filled for a stable shape even when absent.
    assert status["tasks"]["claimed"] == 0
    assert status["tasks"]["cancelled"] == 0


def test_vault_status_empty_vault(cfg: Config) -> None:
    status = vault_status(cfg)
    assert status["notes"] == 0
    assert status["tasks"] == {"open": 0, "claimed": 0, "done": 0, "cancelled": 0}
    assert status["freshness"]["mtime"] is None
    assert status["freshness"]["age_seconds"] is None
    assert status["dangling_links"] == []
    assert status["stale_locks"] == []


def test_vault_status_reports_dangling_wikilinks(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-src", title="Source", body="see [[No Such Note]] here")
    status = vault_status(cfg)
    assert "No Such Note" in status["dangling_links"]


def test_vault_status_no_dangling_when_link_resolves(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-target", title="Target Title")
    _seed_note(vault, note_id="n-src", title="Source", body="see [[Target Title]]")
    status = vault_status(cfg)
    assert status["dangling_links"] == []


def test_vault_status_reports_dangling_wikilinks_in_task_bodies(cfg: Config, vault: Path) -> None:
    # core-hardening/4, root tech.md § B6: dangling counts cover tasks/, not just notes/.
    _seed_task(vault, task_id="t-src", body="Blocked on [[No Such Design Doc]].")
    status = vault_status(cfg)
    assert "No Such Design Doc" in status["dangling_links"]


def test_vault_status_task_id_form_link_is_not_dangling(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-src2", body="See [[n-nope]] and [[t-nope]].")
    status = vault_status(cfg)
    assert status["dangling_links"] == []


def test_vault_status_freshness_tracks_newest_mtime(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-fresh", title="Fresh")
    status = vault_status(cfg)
    assert status["freshness"]["mtime"] is not None
    assert status["freshness"]["age_seconds"] >= 0.0


# --------------------------------------------------------------------------- #
# Stale-lock detection: PID dead OR age > 300 s                               #
# --------------------------------------------------------------------------- #


def test_scan_stale_locks_flags_dead_pid(cfg: Config, vault: Path) -> None:
    _write_lock(vault, "notes", "n-x", pid=_find_dead_pid())
    stale = scan_stale_locks(cfg)
    assert [p.name for p in stale] == ["n-x.lock"]


def test_scan_stale_locks_flags_aged_out_live_pid(cfg: Config, vault: Path) -> None:
    # Owned by a live PID (us) but far past the 300 s TTL -> stale by age.
    _write_lock(vault, "notes", "n-old", pid=os.getpid(), age=_STALE_AGE)
    stale = scan_stale_locks(cfg)
    assert [p.name for p in stale] == ["n-old.lock"]


def test_scan_stale_locks_ignores_fresh_live_lock(cfg: Config, vault: Path) -> None:
    _write_lock(vault, "notes", "n-live", pid=os.getpid())
    assert scan_stale_locks(cfg) == []


def test_scan_stale_locks_scans_notes_and_tasks(cfg: Config, vault: Path) -> None:
    _write_lock(vault, "notes", "n-dead", pid=_find_dead_pid())
    _write_lock(vault, "tasks", "t-dead", pid=_find_dead_pid())
    _write_lock(vault, "notes", "n-live", pid=os.getpid())  # fresh -> excluded
    names = {p.name for p in scan_stale_locks(cfg)}
    assert names == {"n-dead.lock", "t-dead.lock"}


# --------------------------------------------------------------------------- #
# shards status (CLI) — direct scan, daemon-down, read-only                    #
# --------------------------------------------------------------------------- #


def test_status_json_shape(cfg: Config, vault: Path, runtime_dir: Path) -> None:
    _seed_note(vault, note_id="n-a", title="A")
    _seed_task(vault, task_id="t-o", status="open")
    result = _invoke(["--json", "status"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["notes"] == 1
    assert obj["tasks"]["open"] == 1
    assert "freshness" in obj
    assert "dangling_links" in obj
    assert "stale_locks" in obj
    assert obj["daemon"]["running"] is False


def test_status_human_output(cfg: Config, vault: Path, runtime_dir: Path) -> None:
    _seed_note(vault, note_id="n-a", title="A")
    result = _invoke(["status"])
    assert result.exit_code == 0, result.output
    assert "notes" in result.output.lower()


def test_status_works_with_daemon_down(cfg: Config, vault: Path, runtime_dir: Path) -> None:
    # No daemon, no socket, no PID file: status must still succeed.
    result = _invoke(["--json", "status"])
    assert result.exit_code == 0, result.output
    assert json.loads(result.output)["daemon"]["running"] is False


def test_status_is_read_only(cfg: Config, vault: Path, runtime_dir: Path) -> None:
    note = _seed_note(vault, note_id="n-ro", title="Read Only", body="untouched")
    before_mtime = note.stat().st_mtime_ns
    before_text = note.read_text(encoding="utf-8")
    result = _invoke(["--json", "status"])
    assert result.exit_code == 0, result.output
    assert note.stat().st_mtime_ns == before_mtime
    assert note.read_text(encoding="utf-8") == before_text


def test_status_reports_daemon_running_when_live_pid(
    cfg: Config, vault: Path, runtime_dir: Path
) -> None:
    write_pid(default_pid_path(), os.getpid())  # a live daemon
    result = _invoke(["--json", "status"])
    assert result.exit_code == 0, result.output
    daemon = json.loads(result.output)["daemon"]
    assert daemon["running"] is True
    assert daemon["pid"] == os.getpid()


def test_status_reports_stale_locks(cfg: Config, vault: Path, runtime_dir: Path) -> None:
    _write_lock(vault, "notes", "n-dead", pid=_find_dead_pid())
    result = _invoke(["--json", "status"])
    assert result.exit_code == 0, result.output
    stale = json.loads(result.output)["stale_locks"]
    assert any(p.endswith("n-dead.lock") for p in stale)


# --------------------------------------------------------------------------- #
# _agent_breakdown (team-awareness/4) — per-agent open/claimed/stale-claim      #
# --------------------------------------------------------------------------- #


def test_agent_breakdown_counts_owns_open_and_claimed(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-owned", status="open", owner="agent-a")
    _seed_task(vault, task_id="t-held", status="claimed", owner="agent-b", claimed_by="agent-a")
    agents = _agent_breakdown(list_tasks(cfg, limit=None))
    assert agents["agent-a"] == {"owns_open": 1, "claimed": 1, "stale_claims": 0}
    assert agents["agent-b"] == {"owns_open": 0, "claimed": 0, "stale_claims": 0}


def test_agent_breakdown_flags_stale_claims(cfg: Config, vault: Path) -> None:
    """A claim untouched for four days counts as stale; a fresh one does not."""
    _seed_task(
        vault,
        task_id="t-idle",
        status="claimed",
        owner="operator",
        claimed_by="agent-a",
        updated=datetime.now(UTC) - timedelta(days=4),
    )
    _seed_task(
        vault,
        task_id="t-fresh",
        status="claimed",
        owner="operator",
        claimed_by="agent-b",
        updated=datetime.now(UTC),
    )
    agents = _agent_breakdown(list_tasks(cfg, limit=None))
    assert agents["agent-a"]["claimed"] == 1
    assert agents["agent-a"]["stale_claims"] == 1
    assert agents["agent-b"]["claimed"] == 1
    assert agents["agent-b"]["stale_claims"] == 0


def test_agent_breakdown_includes_an_agent_holding_nothing_current(
    cfg: Config, vault: Path
) -> None:
    """An agent who owns only a finished task still gets a zero-filled row."""
    _seed_task(vault, task_id="t-done", status="done", owner="agent-a")
    agents = _agent_breakdown(list_tasks(cfg, limit=None))
    assert agents["agent-a"] == {"owns_open": 0, "claimed": 0, "stale_claims": 0}


def test_agent_breakdown_empty_vault_is_empty_dict(cfg: Config, vault: Path) -> None:
    assert _agent_breakdown(list_tasks(cfg, limit=None)) == {}


# --------------------------------------------------------------------------- #
# shards status — the agents breakdown end to end (team-awareness/4)           #
# --------------------------------------------------------------------------- #


def test_status_json_includes_agents_breakdown(cfg: Config, vault: Path, runtime_dir: Path) -> None:
    _seed_task(vault, task_id="t-owned", status="open", owner="agent-a")
    _seed_task(vault, task_id="t-held", status="claimed", owner="agent-b", claimed_by="agent-a")
    result = _invoke(["--json", "status"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["agents"]["agent-a"] == {"owns_open": 1, "claimed": 1, "stale_claims": 0}


def test_status_human_output_lists_agents(cfg: Config, vault: Path, runtime_dir: Path) -> None:
    _seed_task(vault, task_id="t-owned", status="open", owner="agent-a")
    result = _invoke(["status"])
    assert result.exit_code == 0, result.output
    assert "agents:" in result.output
    assert "agent-a" in result.output


def test_status_human_output_says_none_when_no_agents(
    cfg: Config, vault: Path, runtime_dir: Path
) -> None:
    result = _invoke(["status"])
    assert result.exit_code == 0, result.output
    assert "agents: (none)" in result.output


def test_status_agents_warm_and_cold_agree(cfg: Config, vault: Path, sock_dir: Path) -> None:
    """The breakdown is derived from ``task.list``, so it inherits warm/cold parity."""
    from tests.daemon.conftest import running_daemon

    _seed_task(vault, task_id="t-owned", status="open", owner="agent-a")
    _seed_task(vault, task_id="t-held", status="claimed", owner="agent-b", claimed_by="agent-a")
    cold_agents = _agent_breakdown(
        DaemonClient(socket_path=sock_dir / "nonexistent.sock").task_list(cfg, limit=None)
    )
    with running_daemon(sock_dir / "shards.sock", config=cfg):
        warm_agents = _agent_breakdown(
            DaemonClient(socket_path=sock_dir / "shards.sock").task_list(cfg, limit=None)
        )
    assert cold_agents == warm_agents
    assert cold_agents["agent-a"] == {"owns_open": 1, "claimed": 1, "stale_claims": 0}


# --------------------------------------------------------------------------- #
# reindex — delegates to the indexed client                                   #
# --------------------------------------------------------------------------- #


def test_reindex_delegates_to_indexed(
    cfg: Config, runtime_dir: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from shards.index import indexed_client

    calls: list[Config] = []
    monkeypatch.setattr(indexed_client, "reindex", calls.append)
    result = _invoke(["reindex"])
    assert result.exit_code == 0, result.output
    assert len(calls) == 1
    assert isinstance(calls[0], Config)  # the delegate ran with the loaded config


def test_reindex_quiet_still_delegates(
    cfg: Config, runtime_dir: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from shards.index import indexed_client

    calls: list[Config] = []
    monkeypatch.setattr(indexed_client, "reindex", calls.append)
    result = _invoke(["--quiet", "reindex"])
    assert result.exit_code == 0, result.output
    assert len(calls) == 1


def test_reindex_degrades_when_indexed_binary_missing(
    cfg: Config, runtime_dir: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A missing/failed ``indexed`` binary degrades with a notice — never a crash."""
    from shards.index import indexed_client

    def _missing(config: Config) -> None:
        raise FileNotFoundError("indexed")

    monkeypatch.setattr(indexed_client, "reindex", _missing)
    result = _invoke(["reindex"])
    assert result.exit_code == 0, result.output  # caught, not propagated
    assert result.exception is None


# --------------------------------------------------------------------------- #
# daemon status (CLI)                                                         #
# --------------------------------------------------------------------------- #


def test_daemon_status_stopped_when_down(cfg: Config, runtime_dir: Path) -> None:
    result = _invoke(["daemon", "status"])
    assert result.exit_code == 0, result.output
    assert "stopped" in result.output.lower()


def test_daemon_status_json_when_down(cfg: Config, runtime_dir: Path) -> None:
    result = _invoke(["--json", "daemon", "status"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["running"] is False
    assert obj["pid"] is None
    # The socket file is keyed on the vault it serves, so the reported path is
    # this vault's socket — not a machine-wide ``shards.sock``.
    assert obj["socket"] == str(default_socket_path())


def test_daemon_status_running_when_live_pid(cfg: Config, runtime_dir: Path) -> None:
    write_pid(default_pid_path(), os.getpid())
    result = _invoke(["--json", "daemon", "status"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["running"] is True
    assert obj["pid"] == os.getpid()


# --------------------------------------------------------------------------- #
# daemon start / stop — idempotent, seams mocked (no real fork)               #
# --------------------------------------------------------------------------- #


def test_daemon_start_idempotent_when_running(
    cfg: Config, runtime_dir: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    write_pid(default_pid_path(), os.getpid())  # a live daemon

    def _no_spawn() -> int:  # pragma: no cover - must never run
        raise AssertionError("start must not spawn a second daemon")

    monkeypatch.setattr("shards.cli.admin.spawn_daemon", _no_spawn)
    result = _invoke(["daemon", "start"])
    assert result.exit_code == 0, result.output
    assert "already running" in result.output.lower()


def test_daemon_start_spawns_when_down(
    cfg: Config, runtime_dir: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[bool] = []

    def _fake_spawn() -> int:
        calls.append(True)
        return 54321

    monkeypatch.setattr("shards.cli.admin.spawn_daemon", _fake_spawn)
    result = _invoke(["daemon", "start"])
    assert result.exit_code == 0, result.output
    assert calls == [True]
    assert read_pid(default_pid_path()) == 54321


def test_daemon_stop_idempotent_when_not_running(cfg: Config, runtime_dir: Path) -> None:
    result = _invoke(["daemon", "stop"])
    assert result.exit_code == 0, result.output
    assert "not running" in result.output.lower()


def test_daemon_stop_terminates_and_cleans_up(
    cfg: Config, runtime_dir: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    pid_path = default_pid_path()
    write_pid(pid_path, os.getpid())  # a "live" daemon (us)
    socket_path = default_socket_path()  # this vault's socket, under runtime_dir
    socket_path.write_text("", encoding="utf-8")  # stand-in for the socket file

    killed: list[int] = []
    monkeypatch.setattr("shards.cli.admin.terminate_process", lambda pid: killed.append(pid))

    result = _invoke(["daemon", "stop"])
    assert result.exit_code == 0, result.output
    assert killed == [os.getpid()]  # SIGTERM'd via the seam, not really killed
    assert not pid_path.exists()
    assert not socket_path.exists()
