"""tasks/6 — ``task delete``: guarded hard delete (R5).

Exercises R5 (Delete): :func:`mesh.core.tasks.delete_task` plus the
``mesh task delete`` CLI surface. Delete is a *hard* delete — the file is
removed from disk permanently (no archive, no trash) — and it resolves a task in
**any** lifecycle state, scanning both ``tasks/open/`` and ``tasks/done/``
(id-only, no title slug). The removal runs *inside* the per-entity ``O_EXCL``
lock at ``tasks/.locks/<id>.lock`` (resolution happens under the lock too, since
a concurrent finish/cancel can move the file open→done and the lock id is stable
across that move).

The CLI guards it: an interactive TTY prompts ``Delete <id>? [y/N]`` and aborts
(exit 1) unless the user confirms; a machine path (``--json`` / ``--quiet`` /
piped stdin) without ``--force`` refuses (exit 2) rather than silently destroying
data; ``--force`` skips the prompt entirely.

``sys.stdin.isatty()`` is always False under Typer's ``CliRunner``, so the
interactive-prompt paths monkeypatch :func:`mesh.cli.task._is_tty` to simulate a
terminal while still feeding the answer through the runner's stdin.
"""

from __future__ import annotations

import json
import os
import threading
import time
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

import mesh.cli.task as task_cli
import mesh.storage.locks as locks_mod
from mesh.cli.__main__ import app
from mesh.core.tasks import (
    TaskNotFoundError,
    delete_task,
    get_task,
)
from mesh.schemas.config import Config, load_config
from mesh.storage.files import task_folder
from mesh.storage.locks import acquire

_OLD = datetime(2026, 1, 1, 9, 0, 0, tzinfo=UTC)
_STALE_AGE = 400.0  # seconds; > LOCK_TTL_SECONDS (300) so a lock ages out


@pytest.fixture
def cfg(mesh_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str], *, input: str | None = None):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args, input=input)


def _seed_task(
    vault: Path,
    *,
    task_id: str = "t-seed",
    title: str = "Seed Task",
    status: str = "open",
    owner: str | None = "seed-agent",
    claimed_by: str | None = None,
    body: str = "Task body.",
) -> Path:
    """Write a mesh task straight to disk in the folder matching its status."""
    meta: dict[str, object] = {
        "id": task_id,
        "type": "task",
        "title": title,
        "tags": [],
        "owner": owner,
        "created": _OLD,
        "updated": _OLD,
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


def _open_path(vault: Path, task_id: str = "t-seed") -> Path:
    return task_folder("open", vault) / f"{task_id}.md"


def _done_path(vault: Path, task_id: str = "t-seed") -> Path:
    return task_folder("done", vault) / f"{task_id}.md"


def _lock_path(vault: Path, task_id: str) -> Path:
    return vault / "tasks" / ".locks" / f"{task_id}.lock"


def _age_out(lock: Path) -> None:
    """Backdate a lock's mtime past the TTL so it is treated as stale residue."""
    old = time.time() - _STALE_AGE
    os.utime(lock, (old, old))


# --------------------------------------------------------------------------- #
# delete_task (core) — hard delete, returns id, resolves any lifecycle state   #
# --------------------------------------------------------------------------- #


def test_delete_task_removes_open_file_and_returns_id(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-gone", status="open")
    assert path.exists()
    returned = delete_task(cfg, "t-gone")
    assert returned == "t-gone"
    assert not path.exists()


def test_delete_task_resolves_from_done(cfg: Config, vault: Path) -> None:
    """A terminal (done) task is resolvable and deletable — resolver scans both."""
    path = _seed_task(vault, task_id="t-done", status="done", body="b\n\n## Outcome\n\nx")
    assert delete_task(cfg, "t-done") == "t-done"
    assert not path.exists()
    assert not _done_path(vault, "t-done").exists()


def test_delete_task_resolves_claimed(cfg: Config, vault: Path) -> None:
    """A claimed (mid-lifecycle) task deletes just like any other state."""
    path = _seed_task(vault, task_id="t-clm", status="claimed", claimed_by="seed-agent")
    assert delete_task(cfg, "t-clm") == "t-clm"
    assert not path.exists()


def test_delete_task_missing_raises_not_found(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-seed")
    with pytest.raises(TaskNotFoundError):
        delete_task(cfg, "t-missing")


def test_delete_task_is_hard_no_archive_or_trash(cfg: Config, vault: Path) -> None:
    """Hard delete: nothing named <id>.md survives anywhere; no trash/archive dir."""
    _seed_task(vault, task_id="t-hard", status="open")
    delete_task(cfg, "t-hard")

    survivors = list(vault.rglob("t-hard.md"))
    assert survivors == []
    assert not (vault / "tasks" / ".trash").exists()
    assert not (vault / "tasks" / ".archive").exists()
    assert not _lock_path(vault, "t-hard").exists()


def test_delete_task_then_get_raises_not_found(cfg: Config, vault: Path) -> None:
    """After delete, the id resolves to nothing — a subsequent get is not-found."""
    _seed_task(vault, task_id="t-getgone", status="open")
    delete_task(cfg, "t-getgone")
    with pytest.raises(TaskNotFoundError):
        get_task(cfg, "t-getgone")


# --------------------------------------------------------------------------- #
# delete_task (core) — holds the entity lock (no race with a concurrent edit)  #
# --------------------------------------------------------------------------- #


def test_delete_task_acquires_entity_lock(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """delete_task must take the per-entity lock, not blindly unlink the file."""
    seen: list[Path] = []
    real = locks_mod.acquire

    def spy(lock_path: Path):  # type: ignore[no-untyped-def]
        seen.append(lock_path)
        return real(lock_path)

    _seed_task(vault, task_id="t-lockacq", status="open")
    monkeypatch.setattr(locks_mod, "acquire", spy)
    delete_task(cfg, "t-lockacq")
    assert seen == [_lock_path(vault, "t-lockacq")]


def test_delete_task_serializes_behind_a_held_lock(cfg: Config, vault: Path) -> None:
    """A live lock blocks the delete until released — the task survives meanwhile."""
    path = _seed_task(vault, task_id="t-ser", status="open")
    lock = _lock_path(vault, "t-ser")
    done = threading.Event()

    def _delete() -> None:
        delete_task(cfg, "t-ser")
        done.set()

    with acquire(lock):  # hold the entity lock live (simulates a concurrent edit)
        worker = threading.Thread(target=_delete)
        worker.start()
        time.sleep(0.1)
        assert path.exists()  # delete is blocked; task not yet removed
        assert not done.is_set()
    worker.join(timeout=20)
    assert not worker.is_alive()
    assert done.is_set()
    assert not path.exists()  # delete completed once the lock was released


def test_delete_task_clears_stale_lock_residue(cfg: Config, vault: Path) -> None:
    """A *stale* lock (aged past the TTL) is cleared as delete acquires the lock."""
    _seed_task(vault, task_id="t-lock", status="open")
    lock = _lock_path(vault, "t-lock")
    lock.parent.mkdir(parents=True, exist_ok=True)
    lock.write_text(f"{os.getpid()}\n", encoding="utf-8")
    _age_out(lock)  # aged out -> stale, so _hold_lock clears and re-acquires it
    assert lock.exists()

    delete_task(cfg, "t-lock")

    assert not lock.exists()


# --------------------------------------------------------------------------- #
# CLI — --force skips the prompt                                              #
# --------------------------------------------------------------------------- #


def test_cli_force_deletes_and_exits_0(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-force", status="open")
    result = _invoke(["task", "delete", "t-force", "--force"])
    assert result.exit_code == 0, result.output
    assert not path.exists()
    assert "t-force" in result.output


def test_cli_force_json_emits_object(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-json", status="open")
    result = _invoke(["--json", "task", "delete", "t-json", "--force"])
    assert result.exit_code == 0, result.output
    assert not path.exists()
    obj = json.loads(result.output)
    assert obj == {"id": "t-json", "deleted": True}


def test_cli_force_quiet_prints_id_only(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-quiet", status="open")
    result = _invoke(["--quiet", "task", "delete", "t-quiet", "--force"])
    assert result.exit_code == 0, result.output
    assert not path.exists()
    assert result.output.strip() == "t-quiet"


def test_cli_force_skips_prompt_even_on_tty(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """On a TTY, --force must NOT prompt: with no stdin a prompt would EOF→Abort."""
    monkeypatch.setattr(task_cli, "_is_tty", lambda: True)
    path = _seed_task(vault, task_id="t-noprompt", status="open")
    result = _invoke(["task", "delete", "t-noprompt", "--force"], input="")
    assert result.exit_code == 0, result.output
    assert not path.exists()
    assert "Delete t-noprompt?" not in result.output


def test_cli_force_deletes_done_task(cfg: Config, vault: Path) -> None:
    """--force deletes a terminal task from tasks/done/ too."""
    path = _seed_task(vault, task_id="t-fdone", status="done", body="b\n\n## Outcome\n\nx")
    result = _invoke(["task", "delete", "t-fdone", "--force"])
    assert result.exit_code == 0, result.output
    assert not path.exists()


# --------------------------------------------------------------------------- #
# CLI — machine path without --force refuses (exit 2), file survives          #
# --------------------------------------------------------------------------- #


def test_cli_non_tty_without_force_exits_2_keeps_file(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-keep", status="open")
    # CliRunner stdin is not a TTY -> machine path.
    result = _invoke(["task", "delete", "t-keep"])
    assert result.exit_code == 2, result.output
    # A real error message, not just a bare usage/exit code.
    assert "--force" in result.output
    assert path.exists()  # nothing destroyed


def test_cli_json_without_force_exits_2_keeps_file(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-jkeep", status="open")
    result = _invoke(["--json", "task", "delete", "t-jkeep"])
    assert result.exit_code == 2, result.output
    assert "--force" in result.output
    assert path.exists()


def test_cli_quiet_without_force_exits_2_keeps_file(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-qkeep", status="open")
    result = _invoke(["--quiet", "task", "delete", "t-qkeep"])
    assert result.exit_code == 2, result.output
    assert path.exists()


# --------------------------------------------------------------------------- #
# CLI — not found                                                             #
# --------------------------------------------------------------------------- #


def test_cli_missing_exits_3(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here", status="open")
    result = _invoke(["task", "delete", "t-missing", "--force"])
    assert result.exit_code == 3, result.output
    assert "t-missing" in result.output


def test_cli_delete_then_get_exits_3(cfg: Config, vault: Path) -> None:
    """After a CLI delete, `task get` on the same id exits 3 (gone from disk)."""
    _seed_task(vault, task_id="t-gg", status="open")
    deleted = _invoke(["task", "delete", "t-gg", "--force"])
    assert deleted.exit_code == 0, deleted.output
    got = _invoke(["task", "get", "t-gg"])
    assert got.exit_code == 3, got.output


# --------------------------------------------------------------------------- #
# CLI — interactive TTY prompt                                                #
# --------------------------------------------------------------------------- #


def test_cli_tty_confirm_yes_deletes(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(task_cli, "_is_tty", lambda: True)
    path = _seed_task(vault, task_id="t-yes", status="open")
    result = _invoke(["task", "delete", "t-yes"], input="y\n")
    assert result.exit_code == 0, result.output
    assert "Delete t-yes? [y/N]" in result.output
    assert not path.exists()


def test_cli_tty_abort_no_keeps_file(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(task_cli, "_is_tty", lambda: True)
    path = _seed_task(vault, task_id="t-no", status="open")
    result = _invoke(["task", "delete", "t-no"], input="n\n")
    assert result.exit_code == 1  # declining aborts (click.Abort -> exit 1)
    assert "Delete t-no? [y/N]" in result.output
    assert path.exists()  # declined -> nothing destroyed


def test_delete_command_registered() -> None:
    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert "delete" in names
