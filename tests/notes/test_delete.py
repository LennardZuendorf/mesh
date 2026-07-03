"""notes/6 — delete: guarded hard delete.

Exercises R5 (Delete): :func:`brain.core.notes.delete_note` plus the
``brain note delete`` CLI surface. Delete is a *hard* delete — the file is
removed from disk permanently (no archive, no trash) and any
``notes/.locks/<id>.lock`` residue is cleared. The CLI guards it: an interactive
TTY prompts ``Delete <id>? [y/N]`` and aborts unless the user confirms; a machine
path (``--json`` / ``--quiet`` / piped stdin) without ``--force`` refuses (exit 2)
rather than silently destroying data; ``--force`` skips the prompt entirely.

``sys.stdin.isatty()`` is always False under Typer's ``CliRunner``, so the
interactive-prompt paths monkeypatch :func:`brain.cli.note._is_tty` to simulate a
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

import brain.cli.note as note_cli
import brain.core.notes as notes_core
from brain.cli.__main__ import app
from brain.core.notes import (
    AmbiguousSlugError,
    NoteNotFoundError,
    delete_note,
)
from brain.schemas.config import Config, load_config
from brain.storage.files import note_folder
from brain.storage.locks import acquire

_STALE_AGE = 400.0  # seconds; > LOCK_TTL_SECONDS (300) so a lock ages out


def _age_out(lock: Path) -> None:
    """Backdate a lock's mtime past the TTL so it is treated as stale residue."""
    old = time.time() - _STALE_AGE
    os.utime(lock, (old, old))


_NOW = datetime(2026, 6, 1, 12, 0, 0, tzinfo=UTC)


def _seed_note(
    vault: Path,
    *,
    note_id: str = "n-seed",
    note_type: str = "note",
    title: str = "Seed Note",
    body: str = "Body line.",
) -> Path:
    """Write a brain note straight to disk in the folder matching its type."""
    meta: dict[str, object] = {
        "id": note_id,
        "type": note_type,
        "title": title,
        "tags": [],
        "owner": "seed-agent",
        "created": _NOW,
        "updated": _NOW,
        "related": [],
    }
    folder = note_folder(note_type, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{note_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


def _lock_path(vault: Path, note_id: str) -> Path:
    return vault / "notes" / ".locks" / f"{note_id}.lock"


@pytest.fixture
def cfg(brain_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str], *, input: str | None = None):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args, input=input)


# --------------------------------------------------------------------------- #
# delete_note (core) — hard delete + lock cleanup                             #
# --------------------------------------------------------------------------- #


def test_delete_note_removes_file_and_returns_id(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, note_id="n-gone")
    assert path.exists()
    returned = delete_note(cfg, "n-gone")
    assert returned == "n-gone"
    assert not path.exists()


def test_delete_note_resolves_by_slug(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, note_id="n-abcd", title="CLID Fallback")
    assert delete_note(cfg, "clid-fallback") == "n-abcd"
    assert not path.exists()


def test_delete_note_missing_raises_not_found(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-seed")
    with pytest.raises(NoteNotFoundError):
        delete_note(cfg, "n-missing")


def test_delete_note_ambiguous_slug_raises(cfg: Config, vault: Path) -> None:
    a = _seed_note(vault, note_id="n-aaaa", title="Same Title")
    b = _seed_note(vault, note_id="n-bbbb", title="Same Title", note_type="decision")
    with pytest.raises(AmbiguousSlugError):
        delete_note(cfg, "same-title")
    # Neither is deleted on an ambiguous resolution.
    assert a.exists()
    assert b.exists()


def test_delete_note_clears_stale_lock_residue(cfg: Config, vault: Path) -> None:
    """A *stale* lock (aged past the TTL) is cleared as delete acquires the lock.

    (Previously this seeded a live-PID lock and asserted unconditional removal;
    finding #2 corrected the contract — a *live* lock is now respected, only stale
    residue is cleared.)
    """
    _seed_note(vault, note_id="n-lock")
    lock = _lock_path(vault, "n-lock")
    lock.parent.mkdir(parents=True, exist_ok=True)
    lock.write_text(f"{os.getpid()}\n", encoding="utf-8")
    _age_out(lock)  # aged out -> stale, so _hold_lock clears and re-acquires it
    assert lock.exists()

    delete_note(cfg, "n-lock")

    assert not lock.exists()


def test_delete_note_without_lock_present_is_ok(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, note_id="n-nolock")
    assert not _lock_path(vault, "n-nolock").exists()
    delete_note(cfg, "n-nolock")  # no lock to remove — must not raise
    assert not path.exists()


def test_delete_note_is_hard_no_archive_or_trash(cfg: Config, vault: Path) -> None:
    """Hard delete: nothing named <id>.md survives anywhere; no trash/archive dir."""
    _seed_note(vault, note_id="n-hard")
    delete_note(cfg, "n-hard")

    survivors = list(vault.rglob("n-hard.md"))
    assert survivors == []
    assert not (vault / "notes" / ".trash").exists()
    assert not (vault / "notes" / ".archive").exists()
    assert not _lock_path(vault, "n-hard").exists()


# --------------------------------------------------------------------------- #
# Finding #1 — foreign (non-brain) files are never resolved / deleted          #
# --------------------------------------------------------------------------- #


def _seed_foreign(vault: Path, name: str, title: str) -> Path:
    """Write a coexisting Tolaria file with no brain ``n-`` id (non-``n-`` stem)."""
    path = vault / "notes" / f"{name}.md"
    post = frontmatter.Post("Foreign content.", title=title, tags=["x"])
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def test_delete_note_refuses_foreign_by_stem(cfg: Config, vault: Path) -> None:
    """A foreign file addressed by its filename stem is not-found, not deleted."""
    foreign = _seed_foreign(vault, "tolaria-foo", "Tolaria Foo")
    with pytest.raises(NoteNotFoundError):
        delete_note(cfg, "tolaria-foo")
    assert foreign.exists()  # data loss averted


def test_delete_note_refuses_foreign_by_slug(cfg: Config, vault: Path) -> None:
    """A foreign file addressed by its title slug is not-found, not deleted."""
    foreign = _seed_foreign(vault, "tolaria-bar", "Tolaria Bar")
    with pytest.raises(NoteNotFoundError):
        delete_note(cfg, "tolaria-bar")  # == _slugify("Tolaria Bar")
    assert foreign.exists()


def test_cli_delete_foreign_exits_3_keeps_file(cfg: Config, vault: Path) -> None:
    foreign = _seed_foreign(vault, "tolaria-cli", "Tolaria Cli")
    result = _invoke(["note", "delete", "tolaria-cli", "--force"])
    assert result.exit_code == 3, result.output
    assert foreign.exists()


# --------------------------------------------------------------------------- #
# Finding #2 — delete acquires the entity lock (no race with a concurrent edit) #
# --------------------------------------------------------------------------- #


def test_delete_note_acquires_entity_lock(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """delete_note must take the per-entity lock, not blindly unlink it."""
    seen: list[Path] = []
    real = notes_core.acquire

    def spy(lock_path: Path):  # type: ignore[no-untyped-def]
        seen.append(lock_path)
        return real(lock_path)

    _seed_note(vault, note_id="n-lockacq")
    monkeypatch.setattr(notes_core, "acquire", spy)
    delete_note(cfg, "n-lockacq")
    assert seen == [_lock_path(vault, "n-lockacq")]


def test_delete_serializes_behind_a_held_lock(cfg: Config, vault: Path) -> None:
    """A live lock blocks the delete until released — the note survives meanwhile."""
    path = _seed_note(vault, note_id="n-ser")
    lock = _lock_path(vault, "n-ser")
    done = threading.Event()

    def _delete() -> None:
        delete_note(cfg, "n-ser")
        done.set()

    with acquire(lock):  # hold the entity lock live (simulates a concurrent edit)
        worker = threading.Thread(target=_delete)
        worker.start()
        time.sleep(0.1)
        assert path.exists()  # delete is blocked; note not yet removed
        assert not done.is_set()
    worker.join(timeout=20)
    assert not worker.is_alive()
    assert done.is_set()
    assert not path.exists()  # delete completed once the lock was released


# --------------------------------------------------------------------------- #
# CLI — --force skips the prompt                                              #
# --------------------------------------------------------------------------- #


def test_cli_force_deletes_and_exits_0(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, note_id="n-force")
    result = _invoke(["note", "delete", "n-force", "--force"])
    assert result.exit_code == 0, result.output
    assert not path.exists()
    assert "n-force" in result.output


def test_cli_force_skips_prompt_even_on_tty(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """On a TTY, --force must NOT prompt: with no stdin a prompt would EOF→Abort."""
    monkeypatch.setattr(note_cli, "_is_tty", lambda: True)
    path = _seed_note(vault, note_id="n-noprompt")
    result = _invoke(["note", "delete", "n-noprompt", "--force"], input="")
    assert result.exit_code == 0, result.output
    assert not path.exists()
    assert "Delete n-noprompt?" not in result.output


def test_cli_force_quiet_prints_id_only(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, note_id="n-quiet")
    result = _invoke(["--quiet", "note", "delete", "n-quiet", "--force"])
    assert result.exit_code == 0, result.output
    assert not path.exists()
    assert result.output.strip() == "n-quiet"


def test_cli_force_json_emits_object(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, note_id="n-json")
    result = _invoke(["--json", "note", "delete", "n-json", "--force"])
    assert result.exit_code == 0, result.output
    assert not path.exists()
    obj = json.loads(result.output)
    assert obj["id"] == "n-json"


def test_cli_force_clears_stale_lock_residue(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-flock")
    lock = _lock_path(vault, "n-flock")
    lock.parent.mkdir(parents=True, exist_ok=True)
    lock.write_text(f"{os.getpid()}\n", encoding="utf-8")
    _age_out(lock)  # stale residue -> cleared on acquire
    result = _invoke(["note", "delete", "n-flock", "--force"])
    assert result.exit_code == 0, result.output
    assert not lock.exists()


# --------------------------------------------------------------------------- #
# CLI — machine path without --force refuses (exit 2), file survives          #
# --------------------------------------------------------------------------- #


def test_cli_non_tty_without_force_exits_2_keeps_file(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, note_id="n-keep")
    # CliRunner stdin is not a TTY -> machine path.
    result = _invoke(["note", "delete", "n-keep"])
    assert result.exit_code == 2, result.output
    # A real error message, not just a bare usage/exit code.
    assert "--force" in result.output
    assert path.exists()  # nothing destroyed


def test_cli_json_without_force_exits_2_keeps_file(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, note_id="n-jkeep")
    result = _invoke(["--json", "note", "delete", "n-jkeep"])
    assert result.exit_code == 2, result.output
    assert "--force" in result.output
    assert path.exists()


# --------------------------------------------------------------------------- #
# CLI — not found                                                             #
# --------------------------------------------------------------------------- #


def test_cli_missing_exits_3(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-seed")
    result = _invoke(["note", "delete", "n-missing", "--force"])
    assert result.exit_code == 3, result.output
    assert "n-missing" in result.output


# --------------------------------------------------------------------------- #
# CLI — interactive TTY prompt                                                #
# --------------------------------------------------------------------------- #


def test_cli_tty_confirm_yes_deletes(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(note_cli, "_is_tty", lambda: True)
    path = _seed_note(vault, note_id="n-yes")
    result = _invoke(["note", "delete", "n-yes"], input="y\n")
    assert result.exit_code == 0, result.output
    assert "Delete n-yes? [y/N]" in result.output
    assert not path.exists()


def test_cli_tty_abort_no_keeps_file(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(note_cli, "_is_tty", lambda: True)
    path = _seed_note(vault, note_id="n-no")
    result = _invoke(["note", "delete", "n-no"], input="n\n")
    assert result.exit_code != 0
    assert "Delete n-no? [y/N]" in result.output
    assert path.exists()  # declined -> nothing destroyed
