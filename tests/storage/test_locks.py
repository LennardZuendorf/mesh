"""core-hardening/6 — locks under the conditions the module exists for.

``storage/locks.py`` exists to answer one question: when two *independent OS
processes* (not two threads sharing a GIL and a PID) contend for the same
``O_EXCL`` lock, does exactly one win, and does a lock abandoned by a process
that has genuinely died get reclaimed safely? Every pre-existing concurrency
test in this tree (``tests/tasks/test_claim.py``'s ``ThreadPoolExecutor`` test,
``tests/storage/test_create_race.py``'s ``multiprocessing`` create race) either
races threads in one PID — where ``_pid_alive`` is always ``True`` and the
stale-reclaim path is structurally unreachable — or races processes on a
*different* code path (the allocator lock, not the reclaim CAS). This module
closes both gaps:

* a cross-process ``task claim`` race, driven through the real CLI so exit
  codes (0 winner, 4 losers) are the real contract, not an internal return
  value;
* a lock owned by a **genuinely reaped** PID (spawned, joined, gone — not a
  guessed unused number) being reclaimed;
* two real OS processes racing the ``_reclaim_if_stale`` compare-and-swap
  itself, engineered (via event handshakes, not luck) to deterministically
  exercise each of its three outcomes: the stale lock vanishes entirely before
  a reclaimer can even open it (``FileNotFoundError`` at the ``os.open``),
  two reclaimers open the *same* stale lock and the flock loser finds it gone
  by the time it stats under its own flock (``FileNotFoundError`` after
  ``flock``), and — the flagship case the CAS exists for — a reclaimer that
  paused mid-check resumes to find the stale lock already replaced by a live,
  freshly-created lock; the CAS's re-check under the flock must recognise this
  and *refuse* to steal it, where a naive unlink-then-create implementation
  would delete the live holder's lock out from under it.
"""

from __future__ import annotations

import multiprocessing
import os
from multiprocessing.synchronize import Barrier as MpBarrier
from multiprocessing.synchronize import Event as MpEvent
from pathlib import Path
from typing import Any

import frontmatter
import pytest
from typer.testing import CliRunner

import mesh.storage.locks as locks_mod
from mesh.core.tasks import create_task
from mesh.schemas.config import Config, load_config
from mesh.storage.locks import LockError, acquire

_QUEUE_TIMEOUT = 20.0
_EVENT_TIMEOUT = 30.0


@pytest.fixture
def cfg(mesh_config: Path) -> Config:
    return load_config()


def _reload(path: Path) -> frontmatter.Post:
    return frontmatter.loads(path.read_text(encoding="utf-8"))


# --------------------------------------------------------------------------- #
# 1. N separate OS processes race one ``task claim`` (real CLI exit codes)     #
# --------------------------------------------------------------------------- #


def _claim_worker(
    config_path: str,
    task_id: str,
    owner: str,
    barrier: MpBarrier,
    queue: multiprocessing.Queue[tuple[str, int]],
) -> None:
    """Run in a spawned child process: invoke the real CLI ``task claim``."""
    import os as _os

    _os.environ["MESH_CONFIG_PATH"] = config_path
    _os.environ.pop("MESH_AGENT", None)

    from mesh.cli.__main__ import app

    barrier.wait()
    result = CliRunner().invoke(app, ["--owner", owner, "task", "claim", task_id])
    queue.put((owner, result.exit_code))


def test_concurrent_task_claim_cross_process_exactly_one_winner(
    cfg: Config, vault: Path, config_path: Path
) -> None:
    """N distinct OS processes race the same ``task claim``: one exits 0, rest exit 4.

    Real ``multiprocessing.Process`` workers (spawn context) — not threads —
    each carrying a distinct identity, so a loser's rejection is a genuine
    ``ClaimConflictError``/``LockError`` (both CLI exit 4), never a same-agent
    idempotent no-op that would also exit 0 and falsify "exactly one winner".
    """
    n = 6
    task = create_task(cfg, "Cross-Process Race Task", body="race body")
    owners = [f"racer-{i}" for i in range(n)]

    ctx = multiprocessing.get_context("spawn")
    barrier = ctx.Barrier(n)
    queue: multiprocessing.Queue = ctx.Queue()
    procs = [
        ctx.Process(target=_claim_worker, args=(str(config_path), task.id, owner, barrier, queue))
        for owner in owners
    ]
    for proc in procs:
        proc.start()
    results = [queue.get(timeout=_QUEUE_TIMEOUT) for _ in range(n)]
    for proc in procs:
        proc.join(timeout=_QUEUE_TIMEOUT)
        assert proc.exitcode == 0, f"worker process crashed: exitcode={proc.exitcode}"

    winners = [owner for owner, code in results if code == 0]
    losers = [(owner, code) for owner, code in results if code != 0]
    assert len(winners) == 1, results  # exactly one process claimed
    assert all(code == 4 for _owner, code in losers), (
        results
    )  # every loser is a conflict, not a crash
    assert len(losers) == n - 1, results

    on_disk = _reload(vault / "tasks" / "open" / f"{task.id}.md")
    assert on_disk.metadata["claimed_by"] == winners[0]  # the winner's claim is durable


# --------------------------------------------------------------------------- #
# 2. A genuinely reaped PID — not a guessed unused number — is reclaimed      #
# --------------------------------------------------------------------------- #


def _noop_child() -> None:
    return None


def _spawn_and_reap() -> int:
    """Start a real child process and join it: returns a PID that is truly dead.

    Distinct from the pre-existing ``_find_dead_pid()`` helper elsewhere in the
    suite (``tests/notes/test_storage.py``), which *guesses* a high, probably-
    unused PID number. This one is a real process this test spawned and waited
    on — genuinely reaped, satisfying the unit's binding constraint that
    ``_pid_alive`` return ``False`` for a real dead PID, not an assumed one.
    """
    ctx = multiprocessing.get_context("spawn")
    proc = ctx.Process(target=_noop_child)
    proc.start()
    proc.join(timeout=10)
    assert proc.exitcode == 0
    assert proc.pid is not None
    return proc.pid


def test_stale_lock_with_genuinely_reaped_pid_is_reclaimed(vault: Path) -> None:
    lock = vault / "tasks" / ".locks" / "t-real-dead.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    dead_pid = _spawn_and_reap()

    # Sanity: this PID is genuinely gone, not merely assumed unused.
    with pytest.raises(ProcessLookupError):
        os.kill(dead_pid, 0)
    assert locks_mod._pid_alive(dead_pid) is False  # the crux this unit exists to prove

    lock.write_text(f"{dead_pid}\n", encoding="utf-8")
    with acquire(lock):
        assert lock.read_text(encoding="utf-8").strip() == str(os.getpid())
    assert not lock.exists()


# --------------------------------------------------------------------------- #
# 3. Two real OS processes race ``_reclaim_if_stale`` — three deterministic    #
#    interleavings of the CAS, each engineered via event handshakes           #
# --------------------------------------------------------------------------- #


def _seed_dead_lock(lock_path: Path) -> int:
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    dead_pid = _spawn_and_reap()
    lock_path.write_text(f"{dead_pid}\n", encoding="utf-8")
    return dead_pid


# -- 3a. locks.py:101-102 — os.open(O_RDONLY) itself raises FileNotFoundError -- #
# The stale lock vanishes *entirely* (a peer both reclaimed and fully released
# it) between this process judging it stale and trying to open it for the CAS.


def _slow_paused_reclaimer(
    lock_path_str: str,
    ready_evt: MpEvent,
    go_evt: MpEvent,
    queue: multiprocessing.Queue[tuple[str, Any]],
) -> None:
    import mesh.storage.locks as _locks

    target = Path(lock_path_str)
    real_is_stale = _locks._is_stale
    paused = {"done": False}

    def paused_is_stale(lock_path: Path) -> bool:
        result = real_is_stale(lock_path)
        if lock_path == target and result and not paused["done"]:
            paused["done"] = True
            ready_evt.set()
            go_evt.wait(timeout=_EVENT_TIMEOUT)
        return result

    _locks._is_stale = paused_is_stale  # ty: ignore[invalid-assignment]
    try:
        with _locks.acquire(target):
            queue.put(("acquired", os.getpid()))
    except _locks.LockError:
        queue.put(("refused", os.getpid()))
    except Exception as exc:  # pragma: no cover - surfaced via assertion on RED
        queue.put(("error", repr(exc)))


def _fast_full_cycle(
    lock_path_str: str,
    ready_evt: MpEvent,
    go_evt: MpEvent,
    queue: multiprocessing.Queue[tuple[str, Any]],
) -> None:
    import mesh.storage.locks as _locks

    lock_path = Path(lock_path_str)
    ready_evt.wait(timeout=_EVENT_TIMEOUT)
    try:
        with _locks.acquire(lock_path):
            pass  # reclaim the stale lock, create a fresh one, immediately release it
        queue.put(("cleared", os.getpid()))
    except Exception as exc:  # pragma: no cover - surfaced via assertion on RED
        queue.put(("error", repr(exc)))
    finally:
        go_evt.set()


def test_cas_race_stale_lock_vanishes_before_reopen(vault: Path) -> None:
    """A peer fully reclaims *and releases* between our stale-check and our open.

    Forces locks.py:101-102: the CAS's ``os.open(O_RDONLY)`` itself raises
    ``FileNotFoundError`` because the lock is not merely replaced but entirely
    gone by the time we try to open it — the peer's whole reclaim-create-release
    cycle happened inside our pause window.
    """
    lock = vault / "tasks" / ".locks" / "t-vanish.lock"
    _seed_dead_lock(lock)

    ctx = multiprocessing.get_context("spawn")
    ready_evt = ctx.Event()
    go_evt = ctx.Event()
    queue: multiprocessing.Queue = ctx.Queue()

    slow = ctx.Process(target=_slow_paused_reclaimer, args=(str(lock), ready_evt, go_evt, queue))
    fast = ctx.Process(target=_fast_full_cycle, args=(str(lock), ready_evt, go_evt, queue))
    slow.start()
    fast.start()
    results = dict(sorted(queue.get(timeout=_QUEUE_TIMEOUT) for _ in range(2)))
    slow.join(timeout=_QUEUE_TIMEOUT)
    fast.join(timeout=_QUEUE_TIMEOUT)
    assert slow.exitcode == 0 and fast.exitcode == 0, (slow.exitcode, fast.exitcode)

    assert "error" not in results, results
    assert results.get("cleared") is not None  # fast completed its full cycle
    assert results.get("acquired") is not None  # slow retried past the FileNotFoundError and won
    assert not lock.exists()  # slow released too — nothing left behind


# -- 3b. locks.py:107-108 — two racers open the *same* stale inode; the flock  #
#    loser's stat finds it gone once it finally gets the flock                #


def _barrier_synced_reclaimer(
    lock_path_str: str,
    barrier: MpBarrier,
    queue: multiprocessing.Queue[tuple[Any, ...]],
    tag: str,
) -> None:
    import os as _os

    lock_path = Path(lock_path_str)
    real_open = _os.open
    fired = {"done": False}

    def patched_open(path: str | bytes | os.PathLike[str], flags: int, *a: Any, **kw: Any) -> int:
        fd = real_open(path, flags, *a, **kw)
        if path == str(lock_path) and not (flags & _os.O_CREAT) and not fired["done"]:
            fired["done"] = True
            barrier.wait(timeout=_EVENT_TIMEOUT)
        return fd

    _os.open = patched_open  # ty: ignore[invalid-assignment]
    import mesh.storage.locks as _locks

    try:
        # hold() (bounded wait-and-retry), not the bare 3-attempt acquire(): the
        # loser of the flock race may find a *live* fresh lock immediately after
        # (the winner's own retry-create), which a single-shot acquire() would
        # correctly refuse — hold() is what production callers actually use to
        # ride out exactly that, so it is what proves "both eventually get a
        # turn" without racing the winner's release on raw scheduler luck.
        with _locks.hold(lock_path):
            queue.put((tag, "acquired", os.getpid()))
    except Exception as exc:  # pragma: no cover - surfaced via assertion on RED
        queue.put((tag, "error", repr(exc)))


def test_cas_race_two_reclaimers_open_same_stale_inode(vault: Path) -> None:
    """Both racers open the *same* original stale lock before either unlinks it.

    Forces locks.py:107-108: whichever process loses the ``flock`` race finds,
    once it finally acquires the flock, that its flock-winning peer already
    unlinked the file out from under it — ``lock_path.stat()`` raises
    ``FileNotFoundError`` — and the CAS must treat that as "a peer already
    reclaimed it" (retry) rather than crashing.
    """
    lock = vault / "tasks" / ".locks" / "t-same-inode.lock"
    _seed_dead_lock(lock)

    ctx = multiprocessing.get_context("spawn")
    barrier = ctx.Barrier(2)
    queue: multiprocessing.Queue = ctx.Queue()
    procs = [
        ctx.Process(target=_barrier_synced_reclaimer, args=(str(lock), barrier, queue, "a")),
        ctx.Process(target=_barrier_synced_reclaimer, args=(str(lock), barrier, queue, "b")),
    ]
    for proc in procs:
        proc.start()
    results = [queue.get(timeout=_QUEUE_TIMEOUT) for _ in range(2)]
    for proc in procs:
        proc.join(timeout=_QUEUE_TIMEOUT)
        assert proc.exitcode == 0, proc.exitcode

    statuses = {tag: status for tag, status, _pid in results}
    assert statuses == {"a": "acquired", "b": "acquired"}, results  # both eventually got a turn
    assert not lock.exists()  # each released after its own turn


# -- 3c. locks.py:109->111 (false) — the flagship: a stale lock replaced by a   #
#    live one must NOT be stolen — the defect the CAS exists to prevent        #


def _slow_paused_reclaimer_vs_live(
    lock_path_str: str,
    checked_evt: MpEvent,
    go_evt: MpEvent,
    done_evt: MpEvent,
    queue: multiprocessing.Queue[tuple[str, Any]],
) -> None:
    import mesh.storage.locks as _locks

    target = Path(lock_path_str)
    real_is_stale = _locks._is_stale
    paused = {"done": False}

    def paused_is_stale(lock_path: Path) -> bool:
        result = real_is_stale(lock_path)
        if lock_path == target and result and not paused["done"]:
            paused["done"] = True
            # Announce "I have judged the *original* lock stale" before pausing,
            # so fast is only ever released to run after this happened — never
            # racing ahead of it (which would let fast win outright and make
            # slow's later refusal trivial/uninformative instead of proving the
            # re-check).
            checked_evt.set()
            go_evt.wait(timeout=_EVENT_TIMEOUT)
        return result

    _locks._is_stale = paused_is_stale  # ty: ignore[invalid-assignment]
    try:
        with _locks.acquire(target):
            queue.put(("stole-it", os.getpid()))  # only a BROKEN CAS reaches this
    except _locks.LockError:
        queue.put(("correctly-refused", os.getpid()))
    except Exception as exc:  # pragma: no cover - surfaced via assertion on RED
        queue.put(("error", repr(exc)))
    finally:
        done_evt.set()


def _fast_holds_a_live_lock(
    lock_path_str: str,
    checked_evt: MpEvent,
    ready_evt: MpEvent,
    done_evt: MpEvent,
    queue: multiprocessing.Queue[tuple[str, Any]],
) -> None:
    import mesh.storage.locks as _locks

    lock_path = Path(lock_path_str)
    # Do not even attempt the reclaim until slow has genuinely observed the
    # *original* dead-PID lock as stale — otherwise fast could win the race to
    # replace it before slow ever looked, and slow's later refusal would prove
    # nothing about the re-check under test.
    checked_evt.wait(timeout=_EVENT_TIMEOUT)
    try:
        with _locks.acquire(lock_path):
            queue.put(("holding", os.getpid()))
            ready_evt.set()
            done_evt.wait(timeout=_EVENT_TIMEOUT)
        queue.put(("released", os.getpid()))
    except Exception as exc:  # pragma: no cover - surfaced via assertion on RED
        queue.put(("error", repr(exc)))


def test_cas_race_refuses_to_steal_a_lock_replaced_while_it_paused(vault: Path) -> None:
    """The flagship race: locks.py's own rationale for the CAS, reproduced live.

    Slow judges the *original* dead-PID lock stale, then pauses (event-gated,
    not luck) before opening it for the CAS. Fast is held back (via
    ``checked_evt``) until slow has genuinely made that observation, so it
    cannot win the underlying race outright and short-circuit what this test
    is proving. Only then does fast reclaim that same original lock for real
    and create its own fresh, live lock — and *hold it open*. Slow then
    resumes into the CAS: its ``os.open`` succeeds (a lock now exists, just
    not the one it judged), its ``flock`` succeeds (fast never takes one), and
    the inode it opened still matches the path — so without the second
    ``_is_stale`` re-check under the flock, a naive implementation would
    unlink fast's *live* lock and steal it, producing two simultaneous
    holders. The re-check sees fast's real, alive PID and skips the clear
    (locks.py:109's false branch) instead.

    Load-bearing: replace ``_reclaim_if_stale`` with a naive
    ``_is_stale(...) and _clear(...) or True`` (no flock, no re-check) and this
    test fails — slow reports ``"stole-it"`` instead of
    ``"correctly-refused"``. Demonstrated in the unit report.
    """
    lock = vault / "tasks" / ".locks" / "t-live-replace.lock"
    _seed_dead_lock(lock)

    ctx = multiprocessing.get_context("spawn")
    checked_evt = ctx.Event()  # slow: "I judged the original lock stale"
    ready_evt = ctx.Event()  # fast: "I now hold a fresh, live lock"
    go_evt = ctx.Event()  # slow: released after fast is holding, to resume its pause
    done_evt = ctx.Event()  # slow: "my acquire attempt is finished" -> fast may release
    queue: multiprocessing.Queue = ctx.Queue()

    slow = ctx.Process(
        target=_slow_paused_reclaimer_vs_live,
        args=(str(lock), checked_evt, go_evt, done_evt, queue),
    )
    fast = ctx.Process(
        target=_fast_holds_a_live_lock, args=(str(lock), checked_evt, ready_evt, done_evt, queue)
    )
    slow.start()
    fast.start()

    # Bridge ready_evt -> go_evt from the test process: fast signals ready_evt
    # once it holds the fresh lock; only then do we release slow's pause.
    assert ready_evt.wait(timeout=_EVENT_TIMEOUT)
    go_evt.set()

    # Collect exactly 3 messages: fast's "holding", slow's outcome, fast's "released".
    messages = [queue.get(timeout=_QUEUE_TIMEOUT) for _ in range(3)]
    slow.join(timeout=_QUEUE_TIMEOUT)
    fast.join(timeout=_QUEUE_TIMEOUT)
    assert slow.exitcode == 0 and fast.exitcode == 0, (slow.exitcode, fast.exitcode)

    by_status = {status: pid for status, pid in messages}
    assert "error" not in by_status, messages
    assert "holding" in by_status
    assert "released" in by_status
    assert by_status.get("correctly-refused") is not None, (
        "slow must be refused, not steal fast's live lock",
        messages,
    )
    assert not lock.exists()  # fast released cleanly; nothing left dangling


# --------------------------------------------------------------------------- #
# 3d. In-process companions to 3a/3b/3c, purely for line coverage              #
# --------------------------------------------------------------------------- #
#
# ``coverage.py`` does not by default trace code that runs inside a spawned
# ``multiprocessing.Process`` (this project sets no
# ``[tool.coverage.run] concurrency`` — matching the pre-existing
# ``tests/storage/test_create_race.py``, whose target lines in
# ``core/notes.py``/``core/tasks.py`` sit at 98%, not 100%, for the identical
# reason). The three races above are the genuine, real-process proof the
# binding constraints require; these three are single-process, deterministic
# unit tests isolating the exact same branches so the coverage gate is met
# without claiming a multi-process test proves something coverage.py cannot
# actually see.


def test_reclaim_if_stale_retries_when_lock_vanishes_before_open(
    vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """locks.py:101-102 — ``os.open`` itself raises ``FileNotFoundError``.

    In-process companion to :func:`test_cas_race_stale_lock_vanishes_before_reopen`.
    """
    lock = vault / "tasks" / ".locks" / "t-ghost.lock"
    monkeypatch.setattr(locks_mod, "_is_stale", lambda _p: True)
    assert locks_mod._reclaim_if_stale(lock) is True  # a peer already cleared it -> retry


def test_reclaim_if_stale_retries_when_stat_vanishes_under_flock(
    vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """locks.py:107-108 — ``lock_path.stat()`` raises ``FileNotFoundError`` once
    the flock is held, because a peer reclaimed it in between.

    In-process companion to :func:`test_cas_race_two_reclaimers_open_same_stale_inode`.
    """
    lock = vault / "tasks" / ".locks" / "t-vanish-flock.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    lock.write_text(f"{os.getpid()}\n", encoding="utf-8")  # must exist for os.open to succeed
    monkeypatch.setattr(locks_mod, "_is_stale", lambda _p: True)

    real_stat = Path.stat

    def vanished_stat(self: Path, *a: Any, **kw: Any) -> os.stat_result:
        if self == lock:
            raise FileNotFoundError
        return real_stat(self, *a, **kw)

    monkeypatch.setattr(Path, "stat", vanished_stat)
    assert locks_mod._reclaim_if_stale(lock) is True  # peer reclaimed under our flock -> retry


def test_reclaim_if_stale_skips_clear_when_rechecked_as_live(
    vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """locks.py:109 false branch — ``still_same`` holds but the re-check under
    the flock finds the lock no longer stale, so the clear is skipped.

    In-process companion to
    :func:`test_cas_race_refuses_to_steal_a_lock_replaced_while_it_paused` —
    the exact defensive branch that test's flagship scenario depends on.
    """
    lock = vault / "tasks" / ".locks" / "t-recheck-live.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    lock.write_text(f"{os.getpid()}\n", encoding="utf-8")

    calls = {"n": 0}

    def flip(_path: Path) -> bool:
        calls["n"] += 1
        return calls["n"] == 1  # outer check: stale; inner re-check under flock: not stale

    monkeypatch.setattr(locks_mod, "_is_stale", flip)
    assert locks_mod._reclaim_if_stale(lock) is True  # still True (retry) ...
    assert lock.exists()  # ... but the live-looking lock was never cleared


# --------------------------------------------------------------------------- #
# 4. Direct branch coverage — the remaining small, deterministic edges         #
# --------------------------------------------------------------------------- #


def test_is_stale_absent_lock_is_not_stale(vault: Path) -> None:
    """locks.py:61-62 — no lock file at all is not "stale"; the caller just creates it."""
    lock = vault / "tasks" / ".locks" / "t-absent.lock"
    assert locks_mod._is_stale(lock) is False


def test_pid_alive_rejects_non_positive_pid() -> None:
    assert locks_mod._pid_alive(0) is False
    assert locks_mod._pid_alive(-1) is False


def test_pid_alive_permission_error_means_alive(monkeypatch: pytest.MonkeyPatch) -> None:
    """A PID that exists but is owned by another user raises PermissionError.

    This sandbox runs as root, which can signal (probe) any process regardless
    of owner, so a genuine ``PermissionError`` cannot be produced by process
    ownership here (same root-cause note as ``tests/cli/test_exit_codes.py``'s
    read-only-vault case) — ``os.kill`` is monkeypatched to raise it directly,
    reaching the identical branch a non-root caller would hit for real.
    """

    def boom(pid: int, sig: int) -> None:  # noqa: ARG001
        raise PermissionError

    monkeypatch.setattr(locks_mod.os, "kill", boom)
    assert locks_mod._pid_alive(12345) is True  # exists, just not ours to signal


def test_is_stale_treats_unparseable_pid_as_live(vault: Path) -> None:
    """A lock file whose content is not an integer is never stolen (fresh, garbled)."""
    lock = vault / "tasks" / ".locks" / "t-garbled.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    lock.write_text("not-a-pid\n", encoding="utf-8")
    assert locks_mod._is_stale(lock) is False
    with pytest.raises(LockError), acquire(lock):
        pass  # never reached — the point is that acquire() raises before yielding


def test_is_stale_tolerates_vanishing_between_stat_and_read(
    vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A lock present at ``stat()`` time but gone by ``read_text()`` time -> not stale.

    A true instance of this TOCTOU window is nanoseconds wide; reproducing it
    with two real processes would need the same event-choreography as the CAS
    races above for a branch the module itself documents as "vanished; caller
    can just re-create it" — a tolerance branch, not a safety-critical one — so
    it is isolated directly: the file genuinely exists for ``stat()``, and
    ``Path.read_text`` is monkeypatched to raise ``FileNotFoundError`` for this
    one call, exactly the exception a real vanish-between-syscalls would raise.
    """
    lock = vault / "tasks" / ".locks" / "t-toctou.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    lock.write_text(f"{os.getpid()}\n", encoding="utf-8")

    real_read_text = Path.read_text

    def flaky_read_text(self: Path, *a: Any, **kw: Any) -> str:
        if self == lock:
            raise FileNotFoundError
        return real_read_text(self, *a, **kw)

    monkeypatch.setattr(Path, "read_text", flaky_read_text)
    assert locks_mod._is_stale(lock) is False


def test_acquire_exhausts_retry_budget_raises_lock_error(
    vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Every one of the fixed ``_MAX_ATTEMPTS`` create attempts loses the race.

    ``_reclaim_if_stale`` is forced to always report "retry" while the create
    itself is forced to always lose — the finite retry budget, not the CAS
    logic, is under test: after exhausting it, ``acquire`` must raise
    ``LockError`` rather than looping forever.
    """
    lock = vault / "tasks" / ".locks" / "t-exhausted.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    lock.write_text(f"{os.getpid()}\n", encoding="utf-8")

    monkeypatch.setattr(locks_mod, "_reclaim_if_stale", lambda _p: True)

    real_open = os.open

    def always_exists(
        path: str | Path, flags: int, mode: int = 0o777, *, dir_fd: int | None = None
    ) -> int:
        # ``locks_mod.os`` *is* the real ``os`` module, so this patch is
        # process-wide, not scoped to lock code — pass through anything that
        # isn't our own lock path (e.g. the autouse ``isolate_daemon_socket``
        # fixture's own teardown cleanup) to the real ``os.open``.
        if path not in (lock, str(lock)):
            return real_open(path, flags, mode, dir_fd=dir_fd)
        if flags & os.O_CREAT:
            raise FileExistsError
        raise AssertionError("should not need a real open: _reclaim_if_stale is mocked")

    monkeypatch.setattr(locks_mod.os, "open", always_exists)

    with pytest.raises(LockError, match="could not acquire"), locks_mod.acquire(lock):
        pass  # never reached — the point is that acquire() raises before yielding
