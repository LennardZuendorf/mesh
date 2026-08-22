"""Release is a compare-and-swap, not a blind unlink.

``acquire`` used to end its body with a bare ``_clear(lock_path)`` — an
unconditional ``unlink`` of whatever file sat at the path. That is safe only
while the holder is guaranteed to still own the lock, and it is not: once a
holder ages past :data:`~shards.storage.locks.LOCK_TTL_SECONDS` a peer
legitimately reclaims the path and creates *its own* live lock there. The
original holder's release then deleted the peer's live lock, and the next
acquirer walked straight in — two simultaneous holders of what the module
advertises as an atomic test-and-set.

The reclaim path (:func:`~shards.storage.locks._reclaim_if_stale`) has always
been a compare-and-swap for exactly this reason; these tests pin the same
property on the release path — and pin the one filesystem fact the comparison
rests on (``test_open_descriptor_pins_the_inode_against_reuse``), because
without an open descriptor an inode number is recycled immediately and the
comparison would compare nothing.
"""

from __future__ import annotations

import os
from pathlib import Path

import pytest

import shards.storage.locks as locks_mod
from shards.storage.locks import acquire


@pytest.fixture
def lock_path(vault: Path) -> Path:
    path = vault / "tasks" / ".locks" / "t-release-cas.lock"
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def _expire_locks_instantly(monkeypatch: pytest.MonkeyPatch) -> None:
    """Make every lock TTL-stale the instant it is written.

    A negative TTL is deterministic where ``0.0`` is not: ``_is_stale`` compares
    ``time.time() - mtime > LOCK_TTL_SECONDS``, and a lock written microseconds
    ago can legitimately produce a difference of exactly ``0.0`` on a coarse
    clock. This is the 300-second production path, fast-forwarded.
    """
    monkeypatch.setattr(locks_mod, "LOCK_TTL_SECONDS", -1.0)


def test_release_does_not_unlink_a_successors_live_lock(
    lock_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A stalled holder's release must not delete the lock a peer reclaimed.

    The production sequence with the 300 s TTL fast-forwarded: A acquires; A
    stalls long enough to age out; B finds A's lock stale, reclaims it and
    creates its own live lock; A finally returns and releases. A's release must
    be a no-op, because the file at the path is B's, not A's.

    Same-process on purpose: the successor's lock then carries the *same* PID,
    so PID cannot be the discriminator and the (device, inode) identity check is
    genuinely what is under test. With the bare ``unlink`` this fails on the
    first assertion — the file is simply gone.
    """
    _expire_locks_instantly(monkeypatch)

    a = acquire(lock_path)
    a.__enter__()
    a_ino = lock_path.stat().st_ino

    b = acquire(lock_path)
    b.__enter__()  # B reclaims A's aged-out lock and creates its own
    b_ino = lock_path.stat().st_ino
    assert b_ino != a_ino, "B must have created a new file, not adopted A's"

    a.__exit__(None, None, None)  # A returns from its stall and releases

    assert lock_path.exists(), "A's release destroyed B's live lock"
    assert lock_path.stat().st_ino == b_ino  # still B's lock, untouched

    b.__exit__(None, None, None)
    assert not lock_path.exists()  # B's own release does clear it


def test_release_clears_the_lock_it_still_owns(lock_path: Path) -> None:
    """The ordinary path is unchanged: a holder that still owns the lock frees it."""
    with acquire(lock_path):
        assert lock_path.exists()
    assert not lock_path.exists()


def test_release_tolerates_a_lock_already_gone(lock_path: Path) -> None:
    """A lock removed under the holder (manual cleanup, a reclaimer) is not an error."""
    with acquire(lock_path):
        lock_path.unlink()
    assert not lock_path.exists()


def test_release_does_not_unlink_an_unrelated_file_at_the_path(lock_path: Path) -> None:
    """Even a *non*-lock file that replaced ours is left alone, not deleted.

    The CAS compares identity, not "does something exist here" — so a file
    written at the lock path by anything else (an operator, another tool) is
    never collateral damage of our release.
    """
    with acquire(lock_path):
        lock_path.unlink()
        lock_path.write_text("someone else's file\n", encoding="utf-8")
    assert lock_path.read_text(encoding="utf-8") == "someone else's file\n"
    lock_path.unlink()


def test_release_closes_the_descriptor_it_held(
    lock_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The descriptor held across the body is always closed — no fd leak per lock.

    ``acquire`` now keeps its ``O_EXCL`` descriptor open for the whole protected
    body (that is what pins the inode for the CAS), so the release path owns the
    close. A leak here would be invisible until a long-lived daemon ran out of
    descriptors, so it is pinned directly.
    """
    closed: list[int] = []
    real_close = os.close
    monkeypatch.setattr(locks_mod.os, "close", lambda fd: (closed.append(fd), real_close(fd))[1])

    with acquire(lock_path):
        pass
    assert len(closed) == 1  # exactly the acquire descriptor

    # And on the failure path: a body that raises still closes it.
    closed.clear()
    with pytest.raises(RuntimeError), acquire(lock_path):
        raise RuntimeError("boom")
    assert len(closed) == 1


def test_open_descriptor_pins_the_inode_against_reuse(lock_path: Path) -> None:
    """The filesystem fact the CAS rests on — and the one finding 4 turned on.

    A create/unlink cycle with the descriptor *closed* each time recycles the
    same inode number immediately, which is why an inode comparison across such
    a cycle proves nothing. Holding the descriptor open changes that: the inode
    cannot be deallocated while a descriptor refers to it, so it cannot be
    handed to a successor's ``O_EXCL`` create. That is precisely the situation
    both ``_release`` and ``_reclaim_if_stale`` are in when they compare.

    If this ever fails, the (device, inode) identity check in ``_release`` is no
    longer sufficient on this filesystem and needs a generation stamp written
    into the lock file instead.
    """
    recycled = []
    for _ in range(5):
        fd = os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        recycled.append(os.fstat(fd).st_ino)
        os.close(fd)
        lock_path.unlink()
    # Recorded, not required: on every filesystem this tree runs on, five
    # closed-descriptor cycles return one inode number. A filesystem that does
    # not recycle only makes the check below redundant, never wrong — so it must
    # not fail the suite.
    assert len(recycled) == 5

    held = os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    try:
        mine = os.fstat(held).st_ino
        successors = []
        for _ in range(5):
            lock_path.unlink()
            fd = os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            successors.append(os.fstat(fd).st_ino)
            os.close(fd)
        assert mine not in successors  # our inode is unreachable while we hold it
    finally:
        os.close(held)
        lock_path.unlink()
