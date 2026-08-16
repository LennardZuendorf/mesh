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
import fcntl
import os
import time
from collections.abc import Iterator
from pathlib import Path

from shards.core.errors import ShardsError

LOCK_TTL_SECONDS = 300.0
_MAX_ATTEMPTS = 3
_LOCK_WAIT_SECONDS = 15.0
_LOCK_POLL_SECONDS = 0.01


class LockError(RuntimeError, ShardsError):
    """Raised when a non-stale lock is already held by another owner (CLI exit 4).

    A contended lock is a conflict, not an infrastructure crash — the same exit
    tier as :class:`~shards.core.tasks.ClaimConflictError`. Keeps its historical
    ``RuntimeError`` ancestry (nothing in the tree relies on catching it as a
    plain ``RuntimeError``, but there is no reason to drop it) alongside the new
    ``code``-bearing base the CLI/MCP boundary mappers read.
    """

    code = 4


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


def _reclaim_if_stale(lock_path: Path) -> bool:
    """Remove ``lock_path`` iff it is a stale lock, atomically against other
    reclaimers; report whether the caller should retry the ``O_EXCL`` create.

    A blind ``unlink``-then-create races: two acquirers can both judge a
    dead-PID lock stale and the second's unlink removes the *fresh* lock the
    first just took, yielding two holders. So the removal is a compare-and-swap:
    take an exclusive ``flock`` on the existing lock, then unlink only when the
    file at the path is still the same (inode-matched) stale lock. A peer that
    already re-created a fresh lock fails the re-check and is never unlinked.
    """
    if not _is_stale(lock_path):
        return False
    try:
        fd = os.open(lock_path, os.O_RDONLY)
    except FileNotFoundError:
        return True  # a peer already cleared it — just retry the create
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        try:
            still_same = lock_path.stat().st_ino == os.fstat(fd).st_ino
        except FileNotFoundError:
            return True  # a peer reclaimed it under our flock — retry
        if still_same and _is_stale(lock_path):
            _clear(lock_path)
        return True
    finally:
        os.close(fd)  # releases the flock


def allocator_lock_path(kind_root: Path) -> Path:
    """Return the per-kind allocator lock path under a ``notes/`` or ``tasks/`` root.

    A create has no id yet, so the per-entity ``<id>.lock`` scheme (see callers of
    :func:`hold` in ``core.notes``/``core.tasks``) does not apply — one coarse lock
    per kind (``notes/.locks/_create.lock``, ``tasks/.locks/_create.lock``) is held
    across id allocation and the write instead, closing the ``_id_taken`` ->
    ``atomic_write`` TOCTOU. No new lock semantics: this is the same ``O_EXCL``
    file used by every other lock in this module, just a fixed, id-less name.
    """
    return kind_root / ".locks" / "_create.lock"


@contextlib.contextmanager
def acquire(lock_path: Path) -> Iterator[Path]:
    """Context manager holding an ``O_EXCL`` lock at ``lock_path``.

    Auto-reclaims and re-acquires stale locks (race-safely, see
    :func:`_reclaim_if_stale`); raises :class:`LockError` if a live, fresh lock
    is held. Releases (unlinks) on exit.
    """
    lock_path.parent.mkdir(parents=True, exist_ok=True)

    for _ in range(_MAX_ATTEMPTS):
        try:
            fd = os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        except FileExistsError:
            if _reclaim_if_stale(lock_path):
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


@contextlib.contextmanager
def hold(lock_path: Path) -> Iterator[Path]:
    """Hold the entity ``O_EXCL`` lock, waiting out a live holder.

    :func:`acquire` is a non-blocking test-and-set: it raises :class:`LockError`
    when a live, fresh lock is held. This wrapper adds the bounded wait-and-retry
    policy so concurrent edits serialize instead of failing. Acquisition is
    retried; the protected body is not. Shared by ``core.notes`` and
    ``core.tasks`` — the one home for lock-wait policy.
    """
    deadline = time.monotonic() + _LOCK_WAIT_SECONDS
    while True:
        cm = acquire(lock_path)
        try:
            cm.__enter__()
        except LockError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(_LOCK_POLL_SECONDS)
            continue
        try:
            yield lock_path
        finally:
            cm.__exit__(None, None, None)
        return
