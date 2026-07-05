"""notes/1 — storage primitives: atomic writes, folder routing, sandbox, locks.

These are the hardest-tested primitives in the unit: every later write path
depends on atomicity, containment, and safe concurrent-edit serialization.
"""

from __future__ import annotations

import os
import time
from pathlib import Path

import pytest

from shards.storage.files import atomic_write, note_folder
from shards.storage.locks import LockError, acquire
from shards.storage.sandbox import safe_resolve

# --------------------------------------------------------------------------- #
# atomic_write                                                                  #
# --------------------------------------------------------------------------- #


def test_atomic_write_creates_file(tmp_path: Path) -> None:
    target = tmp_path / "notes" / "n-abcd.md"
    atomic_write(target, "hello world")
    assert target.read_text(encoding="utf-8") == "hello world"


def test_atomic_write_overwrites_existing(tmp_path: Path) -> None:
    target = tmp_path / "n-abcd.md"
    atomic_write(target, "first")
    atomic_write(target, "second")
    assert target.read_text(encoding="utf-8") == "second"


def test_atomic_write_leaves_no_temp_siblings(tmp_path: Path) -> None:
    target = tmp_path / "n-abcd.md"
    atomic_write(target, "content")
    siblings = list(tmp_path.iterdir())
    assert siblings == [target]


def test_atomic_write_never_partial_on_replace_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = tmp_path / "n-abcd.md"
    atomic_write(target, "ORIGINAL")  # pre-seed the destination

    def boom(src: str, dst: str) -> None:  # noqa: ARG001
        raise OSError("simulated crash between temp-write and rename")

    monkeypatch.setattr(os, "replace", boom)

    with pytest.raises(OSError):
        atomic_write(target, "NEW-DATA")

    # Destination is untouched (old content intact, not truncated/partial)...
    assert target.read_text(encoding="utf-8") == "ORIGINAL"
    # ...and the aborted temp file was cleaned up, leaving only the target.
    assert list(tmp_path.iterdir()) == [target]


# --------------------------------------------------------------------------- #
# note_folder routing                                                           #
# --------------------------------------------------------------------------- #


def test_note_folder_routes_by_type(tmp_path: Path) -> None:
    base = tmp_path / "vault"
    assert note_folder("note", base) == base / "notes"
    assert note_folder("log", base) == base / "notes" / "logs"
    assert note_folder("decision", base) == base / "notes" / "decisions"
    assert note_folder("reference", base) == base / "notes" / "references"


def test_note_folder_unknown_type_raises(tmp_path: Path) -> None:
    with pytest.raises(ValueError):
        note_folder("task", tmp_path)


# --------------------------------------------------------------------------- #
# sandbox                                                                       #
# --------------------------------------------------------------------------- #


def test_safe_resolve_accepts_in_sandbox_path(vault: Path) -> None:
    candidate = vault / "notes" / "n-abcd.md"
    resolved = safe_resolve(vault, candidate)
    # Realpath of both sides is compared, so the returned path stays in-sandbox.
    assert resolved.is_relative_to(Path(os.path.realpath(vault)))


def test_safe_resolve_accepts_base_itself(vault: Path) -> None:
    resolved = safe_resolve(vault, vault)
    assert resolved == Path(os.path.realpath(vault))


def test_safe_resolve_rejects_traversal(vault: Path) -> None:
    with pytest.raises(ValueError):
        safe_resolve(vault, vault / ".." / ".." / "etc" / "passwd")


def test_safe_resolve_rejects_symlink_escape(vault: Path, tmp_path: Path) -> None:
    outside = tmp_path / "outside"
    outside.mkdir()
    escape = vault / "notes" / "escape.md"
    escape.symlink_to(outside / "secret.md")  # target need not exist
    with pytest.raises(ValueError):
        safe_resolve(vault, escape)


# --------------------------------------------------------------------------- #
# locks                                                                         #
# --------------------------------------------------------------------------- #


def test_acquire_creates_and_releases(vault: Path) -> None:
    lock = vault / "notes" / ".locks" / "n-abcd.lock"
    with acquire(lock):
        assert lock.exists()
    assert not lock.exists()


def test_acquire_writes_owning_pid(vault: Path) -> None:
    lock = vault / "notes" / ".locks" / "n-abcd.lock"
    with acquire(lock):
        assert lock.read_text().strip() == str(os.getpid())


def test_held_lock_by_live_pid_raises(vault: Path) -> None:
    lock = vault / "notes" / ".locks" / "n-abcd.lock"
    # A fresh lock owned by *this* (alive) process is not stale.
    lock.write_text(f"{os.getpid()}\n")
    with pytest.raises(LockError), acquire(lock):
        pass


def test_stale_lock_dead_pid_is_reacquired(vault: Path) -> None:
    lock = vault / "notes" / ".locks" / "n-abcd.lock"
    dead_pid = _find_dead_pid()
    lock.write_text(f"{dead_pid}\n")
    with acquire(lock):
        # Reacquired: the file now records our pid.
        assert lock.read_text().strip() == str(os.getpid())
    assert not lock.exists()


def test_stale_lock_old_mtime_is_reacquired(vault: Path) -> None:
    lock = vault / "notes" / ".locks" / "n-abcd.lock"
    # Owned by a live pid (us) but far past the 300s TTL -> stale by age.
    lock.write_text(f"{os.getpid()}\n")
    old = time.time() - 600
    os.utime(lock, (old, old))
    with acquire(lock):
        assert lock.exists()
    assert not lock.exists()


def _find_dead_pid() -> int:
    """Return a PID that is not currently alive."""
    for candidate in range(999_999, 900_000, -1):
        try:
            os.kill(candidate, 0)
        except ProcessLookupError:
            return candidate
        except PermissionError:  # pragma: no cover - alive but not ours
            continue
    raise RuntimeError("could not find a dead pid")  # pragma: no cover
