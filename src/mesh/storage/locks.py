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

**Both removals are compare-and-swaps, never blind unlinks.** Reclaiming
someone else's stale lock (:func:`_reclaim_if_stale`) and releasing your own
(:func:`_release`) each hold an open descriptor on the lock they mean, take an
exclusive ``flock``, and unlink only if the file still at the path is that same
(device, inode) file. The open descriptor is what makes the inode comparison
mean something: an inode number cannot be recycled while a descriptor still
refers to it, so no successor's lock can land on an inode one of these callers
is holding. (Recycling is otherwise immediate — a plain create-unlink-create
cycle reuses the same inode number every time.) The identity check therefore
catches every swap that happens *after* the descriptor was opened; a swap that
happened *before* it is caught by the second ``_is_stale`` re-check under the
flock instead. Both guards are load-bearing, for different windows.

The release CAS exists because a holder can legitimately lose its lock while
still inside its body: once it ages past the TTL a peer may reclaim the path and
create its own live lock there. A bare ``unlink`` on the way out would delete
that peer's live lock and let a third caller straight in — two simultaneous
holders of what is advertised as an atomic test-and-set. With the CAS, the
stalled holder's release is simply a no-op.

What the CAS does *not* do is undo the TTL steal itself: a holder stalled past
300 s and the peer that reclaimed from it can still both be inside their bodies.
That is the deliberate standing trade (``Stale locks | TTL + dead PID`` in
`.spec/tech.md`) — a lock whose owner will never return must not wedge an entity
forever — and the reason the TTL is 300 s and not 5.
"""

from __future__ import annotations

import contextlib
import fcntl
import os
import time
from collections.abc import Iterator
from pathlib import Path

from mesh.core.errors import MeshError

LOCK_TTL_SECONDS = 300.0
_MAX_ATTEMPTS = 3
_LOCK_WAIT_SECONDS = 15.0
_LOCK_POLL_SECONDS = 0.01


class LockError(RuntimeError, MeshError):
    """Raised when a non-stale lock is already held by another owner (CLI exit 4).

    A contended lock is a conflict, not an infrastructure crash — the same exit
    tier as :class:`~mesh.core.tasks.ClaimConflictError`. Keeps its historical
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


def _identity(st: os.stat_result) -> tuple[int, int]:
    """The comparable identity of one lock file: (device, inode).

    Only meaningful while the comparing caller holds an **open descriptor** on
    the lock it is asking about — see :func:`_release` and
    :func:`_reclaim_if_stale` for why that is exactly when it is asked.
    """
    return (st.st_dev, st.st_ino)


def _clear(lock_path: Path) -> None:
    with contextlib.suppress(FileNotFoundError):
        lock_path.unlink()


def _release(lock_path: Path, fd: int) -> None:
    """Release the lock created on ``fd`` — a compare-and-swap, never a blind unlink.

    The holder may no longer own the path: if it stalled past
    :data:`LOCK_TTL_SECONDS`, a peer will have reclaimed it and created its own
    live lock there. Unlinking *that* would hand a third caller the lock while
    the peer is still inside its body — two simultaneous holders. So the removal
    mirrors :func:`_reclaim_if_stale`: take the ``flock`` (serializing against a
    reclaimer working on the same inode), then unlink only if the file at the
    path is still the one this acquisition created — matching device and inode.
    Anything else means the lock was taken from us, and the right move is to
    walk away quietly.

    ``fd`` — the descriptor from the original ``O_EXCL`` create — is what makes
    the inode comparison a real identity check rather than a coincidence: an
    inode number cannot be reused while any descriptor still refers to it, so
    while we hold this one open no successor's lock can ever land on our inode.
    That is also why ``acquire`` keeps it open for the whole body instead of
    closing it after the PID write. Closing it here drops the ``flock`` too.
    """
    try:
        with contextlib.suppress(OSError):
            fcntl.flock(fd, fcntl.LOCK_EX)
            try:
                current = _identity(lock_path.stat())
            except OSError:
                return  # already gone (a reclaimer, or a manual cleanup)
            if current == _identity(os.fstat(fd)):
                _clear(lock_path)
    finally:
        os.close(fd)


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
    is held. Releases on exit via :func:`_release`, which unlinks only a lock
    this acquisition still owns — the descriptor stays open for the whole body
    precisely so that identity can be re-verified against it at release time.
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
        except BaseException:
            _release(lock_path, fd)  # nothing published; drop the half-made lock
            raise
        try:
            yield lock_path
        finally:
            _release(lock_path, fd)
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
