"""Per-entity advisory locks for serialized concurrent edits.

A lock is an ``O_EXCL``-created file whose contents are the owning PID. Acquiring
is a single atomic test-and-set: if the file already exists the lock is held,
*unless* it is stale. A lock is stale when either:

* its recorded PID is dead, or
* its mtime is older than :data:`LOCK_TTL_SECONDS` (300 s).

Stale locks are cleared and re-acquired automatically. A live, fresh lock raises
:class:`LockError`. A lock file that exists but is not yet populated with a PID
(the sub-millisecond window between create and write) is treated as *held* while
fresh, so a racing acquirer never steals an in-progress lock.
"""

from __future__ import annotations

import contextlib
import os
import time
from collections.abc import Iterator
from pathlib import Path

LOCK_TTL_SECONDS = 300.0
_MAX_ATTEMPTS = 3


class LockError(RuntimeError):
    """Raised when a non-stale lock is already held by another owner."""


def _pid_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True  # exists, owned by someone else
    return True


def _is_stale(lock_path: Path) -> bool:
    try:
        mtime = lock_path.stat().st_mtime
    except FileNotFoundError:
        return False  # vanished; caller can just re-create it

    if time.time() - mtime > LOCK_TTL_SECONDS:
        return True  # aged out regardless of contents

    try:
        raw = lock_path.read_text(encoding="utf-8").strip()
    except FileNotFoundError:
        return False

    if not raw:
        return False  # fresh but not yet written -> in-progress, treat as held
    try:
        pid = int(raw)
    except ValueError:
        return False  # fresh but unparseable -> don't steal a live lock
    return not _pid_alive(pid)


def _clear(lock_path: Path) -> None:
    with contextlib.suppress(FileNotFoundError):
        lock_path.unlink()


@contextlib.contextmanager
def acquire(lock_path: Path) -> Iterator[Path]:
    """Context manager holding an ``O_EXCL`` lock at ``lock_path``.

    Auto-clears and re-acquires stale locks; raises :class:`LockError` if a
    live, fresh lock is held. Releases (unlinks) on exit.
    """
    lock_path.parent.mkdir(parents=True, exist_ok=True)

    for _ in range(_MAX_ATTEMPTS):
        try:
            fd = os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        except FileExistsError:
            if _is_stale(lock_path):
                _clear(lock_path)
                continue
            raise LockError(f"lock is held: {lock_path}") from None

        try:
            os.write(fd, f"{os.getpid()}\n".encode())
        finally:
            os.close(fd)
        try:
            yield lock_path
        finally:
            _clear(lock_path)
        return

    raise LockError(f"could not acquire lock: {lock_path}")
