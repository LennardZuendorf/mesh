"""core-hardening/3 — the CLI boundary mapper: exit codes on exceptions.

Exercises R3/R4: domain exceptions (``shards.core.errors.ShardsError`` and its
subclasses, plus ``storage.locks.LockError``) carry their own ``code``; the one
boundary mapper (:func:`shards.cli._errors.cli_errors`) reads it and maps a bare
``ValueError``/``OSError`` to 2/1 — instead of every CLI handler hardcoding a
``typer.Exit(N)`` literal. Two scenarios are new behaviour (the defects this unit
fixes):

* a live, fresh, contended lock (:class:`~shards.storage.locks.LockError`) now
  exits 4 with a stderr line naming the entity, instead of hanging out the wait
  budget and printing a traceback;
* an ``OSError`` at the write boundary (ENOSPC, a read-only vault, ...) now exits
  1 with an ``io error:`` line, instead of a traceback.

The rest of the file is the exit-code matrix — one row per exception class ×
CLI surface — proving the mapper is behaviour-preserving for the cases that were
already handled (2 validation, 3 not found, 4 claim conflict), plus the two
binary verification gates from the brief: no hardcoded ``typer.Exit(2|3|4)``
literal survives in ``src/shards/cli/``, and none of these failure paths ever
print a Python traceback.

Root-cause note on the read-only-vault scenario: this sandbox runs as root,
which bypasses filesystem permission bits (``chmod`` cannot simulate a
read-only vault here — verified: root writes through 0o500 directories without
error). The OSError path is instead exercised by monkeypatching
``atomic_write`` to raise ``OSError`` directly, the same substitution
``tests/notes/test_storage.py`` already uses to simulate a crash between the
temp-write and the rename — it reaches the identical CLI boundary code path an
ENOSPC/EROFS would, without depending on process privilege.
"""

from __future__ import annotations

import os
import subprocess
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

import shards.core.notes as notes_core
import shards.core.tasks as tasks_core
import shards.storage.locks as locks_mod
from shards.cli.__main__ import app
from shards.core.notes import create_note
from shards.schemas.config import Config, load_config
from shards.storage.files import note_folder, task_folder

_NOW = datetime(2026, 1, 1, 9, 0, 0, tzinfo=UTC)


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _seed_task(
    vault: Path,
    *,
    task_id: str,
    status: str = "open",
    claimed_by: str | None = None,
) -> Path:
    meta: dict[str, object] = {
        "id": task_id,
        "type": "task",
        "title": "Seed Task",
        "tags": [],
        "owner": "seed-agent",
        "created": _NOW,
        "updated": _NOW,
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
    post = frontmatter.Post("Task body.")
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def _write_live_lock(lock_path: Path) -> None:
    """Pre-create a *real*, live, fresh O_EXCL lock owned by this test process.

    Matches the pattern ``tests/notes/test_storage.py`` uses for
    ``test_held_lock_by_live_pid_raises``: the PID is this very process (always
    alive) and the mtime is fresh (just written), so :func:`~shards.storage.locks._is_stale`
    genuinely judges it non-stale — this is the real contended path, not a mock.
    """
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    lock_path.write_text(f"{os.getpid()}\n", encoding="utf-8")


@pytest.fixture(autouse=True)
def _fast_lock_wait(monkeypatch: pytest.MonkeyPatch) -> None:
    """Shrink the lock wait/poll budget so the live-lock tests stay fast.

    Still exercises the real ``hold()`` retry-until-deadline loop against a real
    live lock file — only the deadline itself shrinks (15s -> 0.1s), not the
    mechanism.
    """
    monkeypatch.setattr(locks_mod, "_LOCK_WAIT_SECONDS", 0.1)
    monkeypatch.setattr(locks_mod, "_LOCK_POLL_SECONDS", 0.01)


# --------------------------------------------------------------------------- #
# New behaviour — LockError: a live, fresh, contended lock -> exit 4          #
# --------------------------------------------------------------------------- #


def test_cli_task_claim_live_lock_exits_4_no_traceback(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-locked", status="open", claimed_by=None)
    _write_live_lock(vault / "tasks" / ".locks" / "t-locked.lock")

    result = _invoke(["task", "claim", "t-locked"])

    assert result.exit_code == 4, result.output
    assert "Traceback" not in result.output
    assert "t-locked" in result.output  # stderr line names the contended entity


def test_cli_note_append_live_lock_exits_4_no_traceback(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "Locked Note", body="x")
    _write_live_lock(vault / "notes" / ".locks" / f"{note.id}.lock")

    result = _invoke(["note", "append", note.id, "more text"])

    assert result.exit_code == 4, result.output
    assert "Traceback" not in result.output
    assert note.id in result.output


def test_cli_task_new_create_lock_live_exits_4_no_traceback(cfg: Config, vault: Path) -> None:
    """A create also takes the per-kind allocator lock (commit d374029) — covered too."""
    _write_live_lock(vault / "tasks" / ".locks" / "_create.lock")

    result = _invoke(["task", "new", "Blocked Create"])

    assert result.exit_code == 4, result.output
    assert "Traceback" not in result.output


# --------------------------------------------------------------------------- #
# New behaviour — OSError at the write boundary -> exit 1, "io error:"        #
# --------------------------------------------------------------------------- #


def test_cli_note_append_write_oserror_exits_1_no_traceback(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    note = create_note(cfg, "IO Note", body="x")

    def boom(path: Path, content: str) -> None:  # noqa: ARG001
        raise OSError(28, "No space left on device")

    monkeypatch.setattr(notes_core, "atomic_write", boom)

    result = _invoke(["note", "append", note.id, "more text"])

    assert result.exit_code == 1, result.output
    assert "io error:" in result.output
    assert "Traceback" not in result.output


def test_cli_task_claim_write_oserror_exits_1_no_traceback(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _seed_task(vault, task_id="t-io", status="open", claimed_by=None)

    def boom(path: Path, content: str) -> None:  # noqa: ARG001
        raise OSError(30, "Read-only file system")

    monkeypatch.setattr(tasks_core, "atomic_write", boom)

    result = _invoke(["task", "claim", "t-io"])

    assert result.exit_code == 1, result.output
    assert "io error:" in result.output
    assert "Traceback" not in result.output


def test_cli_status_scan_oserror_exits_1_no_traceback(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Admin surface coverage: ``shards status`` maps an unexpected OSError too."""

    def boom(*_args: object, **_kwargs: object) -> None:
        raise OSError(13, "Permission denied")

    # ``status`` now runs through the daemon client, whose file-op fallback is the
    # same ``list_notes`` scan the direct call used to make.
    monkeypatch.setattr("shards.daemon.client.list_notes", boom)

    result = _invoke(["status"])

    assert result.exit_code == 1, result.output
    assert "io error:" in result.output
    assert "Traceback" not in result.output


# --------------------------------------------------------------------------- #
# The exit-code matrix — one row per exception class × CLI surface            #
# (behaviour-preserving: every one of these already exited this way)         #
# --------------------------------------------------------------------------- #


def test_cli_note_get_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["note", "get", "n-nope"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_note_append_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["note", "append", "n-nope", "text"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_note_update_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["note", "update", "n-nope", "--tags", "x"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_note_delete_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["note", "delete", "n-nope", "--force"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_note_get_ambiguous_slug_exits_2(cfg: Config, vault: Path) -> None:
    create_note(cfg, "Same Title", body="a")
    create_note(cfg, "Same Title", body="b")
    result = _invoke(["note", "get", "same-title"])
    assert result.exit_code == 2, result.output
    assert "Traceback" not in result.output


def test_cli_task_get_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["task", "get", "t-nope"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_task_update_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["task", "update", "t-nope", "--title", "x"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_task_finish_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["task", "finish", "t-nope"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_task_cancel_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["task", "cancel", "t-nope"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_task_delete_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["task", "delete", "t-nope", "--force"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_task_claim_conflict_exits_4(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-taken", status="claimed", claimed_by="other-agent")
    result = _invoke(["task", "claim", "t-taken"])
    assert result.exit_code == 4, result.output
    assert "Traceback" not in result.output


def test_cli_build_context_seed_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["build-context", "n-nope"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_graph_seed_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["graph", "n-nope"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_project_not_found_exits_3(cfg: Config) -> None:
    result = _invoke(["project", "n-nope"])
    assert result.exit_code == 3, result.output
    assert "Traceback" not in result.output


def test_cli_note_new_invalid_type_exits_2(cfg: Config) -> None:
    result = _invoke(["note", "new", "Bad", "--type", "bogus", "--body", "x"])
    assert result.exit_code == 2, result.output
    assert "Traceback" not in result.output


def test_cli_note_list_invalid_sort_exits_2(cfg: Config) -> None:
    result = _invoke(["note", "list", "--sort", "bogus"])
    assert result.exit_code == 2, result.output
    assert "Traceback" not in result.output


def test_cli_task_list_invalid_sort_exits_2(cfg: Config) -> None:
    result = _invoke(["task", "list", "--sort", "bogus"])
    assert result.exit_code == 2, result.output
    assert "Traceback" not in result.output


def test_cli_task_claim_no_agent_identity_exits_2(
    vault: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _seed_task(vault, task_id="t-noagent", status="open", claimed_by=None)
    cfg_file = tmp_path / "noagent.toml"
    cfg_file.write_text(
        "\n".join(("[core]", f'tolaria_path = "{vault}"', "", "[tasks]", "collections = []", "")),
        encoding="utf-8",
    )
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(cfg_file))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    result = _invoke(["task", "claim", "t-noagent"])
    assert result.exit_code == 2, result.output
    assert "Traceback" not in result.output


def test_cli_note_delete_refuses_non_interactive_exits_2(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "Guarded", body="x")
    result = _invoke(["--json", "note", "delete", note.id])
    assert result.exit_code == 2, result.output
    assert "Traceback" not in result.output
    assert (note_folder("note", vault) / f"{note.id}.md").exists()


# --------------------------------------------------------------------------- #
# Verification gate — no hardcoded numeric literal survives the boundary sweep #
# --------------------------------------------------------------------------- #


def test_no_hardcoded_exit_codes_in_cli_handlers() -> None:
    """``git grep -nE "typer.Exit\\((2|3|4)\\)" src/shards/cli/`` must return nothing.

    Codes live on the domain exceptions; the one mapper (``cli_errors``) reads
    ``.code`` — a handler that hardcodes ``typer.Exit(2|3|4)`` again would be a
    regression back to the pre-core-hardening/3 state this unit fixes.
    """
    repo_root = Path(__file__).resolve().parents[2]
    result = subprocess.run(
        ["git", "grep", "-nE", r"typer\.Exit\((2|3|4)\)", "src/shards/cli/"],
        cwd=repo_root,
        capture_output=True,
        text=True,
    )
    # git grep exits 1 (no matches) on success; 0 means it *found* a violation.
    assert result.returncode == 1, f"hardcoded exit code(s) found:\n{result.stdout}"
    assert result.stdout == ""
