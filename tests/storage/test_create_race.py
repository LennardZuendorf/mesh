"""storage/create-race — race-safe creates: per-kind allocator lock (core-hardening/2).

Verifies :func:`mesh.core.notes.create_note` and :func:`mesh.core.tasks.create_task`
hold a per-kind allocator lock (:func:`mesh.storage.locks.allocator_lock_path`) across
id allocation *and* the write, closing the ``_id_taken`` -> ``atomic_write`` TOCTOU.

Uses real :mod:`multiprocessing` (separate OS processes, spawned fresh — not threads in
one PID): the bug is a race between independent processes writing the same vault on disk,
which a thread-only concurrency test (see ``tests/tasks/test_claim.py``'s
``ThreadPoolExecutor`` test) cannot reproduce, because threads in one process share a GIL
and never truly interleave two ``os.replace`` calls the way two OS processes can.

Determinism, not luck:

* Id generation is stubbed to a fixed digest (:func:`_install_fixed_digest`), so every
  worker's *candidate* id, before extension, is identical — the test does not depend on
  a real SHA-256 happening to collide.
* ``_id_taken`` is wrapped with a short sleep between its scan and its return
  (:func:`_install_toctou_delay`), widening the real check -> write window the bug lives
  in. This is harmless under the fix: the allocator lock already serializes every caller
  through that window one at a time, so the extra sleep only slows the serialized loop —
  it never changes which id wins or whether content survives.
* A :class:`multiprocessing.Barrier` releases every worker at the same instant, so the
  race is forced rather than hoped for.
"""

from __future__ import annotations

import multiprocessing
import time
from collections.abc import Callable
from multiprocessing.synchronize import Barrier as MpBarrier
from pathlib import Path
from types import ModuleType

import frontmatter

_FIXED_DIGEST = "B" * 12  # room to extend id length 4..4+N_RACERS-1 without exhausting
_TOCTOU_DELAY = 0.05  # seconds; widens the _id_taken check -> write window
_QUEUE_TIMEOUT = 20.0  # generous vs. hold()'s own 15s wait budget
N_RACERS = 6


def _install_fixed_digest() -> None:
    """Force every id candidate (pre-extension) onto the same collision path."""
    import mesh.core.ids as ids_mod

    def _fixed(created_iso: str, title: str) -> str:
        return _FIXED_DIGEST

    ids_mod._digest_b32 = _fixed  # ty: ignore[invalid-assignment]


def _install_toctou_delay(module: ModuleType) -> None:
    """Wrap ``module._id_taken`` to sleep between its scan and its return.

    Widens the real ``_id_taken`` -> ``atomic_write`` TOCTOU window so a real race
    between OS processes is reliably forced rather than merely possible. Harmless
    once the fix holds the allocator lock across allocation+write: callers are
    already serialized through this window one at a time, so the sleep only slows
    the serialized loop down — it never changes which id wins.
    """
    original = module._id_taken

    def _slow(config: object, candidate: str) -> bool:
        result = original(config, candidate)
        time.sleep(_TOCTOU_DELAY)
        return result

    module._id_taken = _slow  # ty: ignore[unresolved-attribute]


def _note_worker(
    config_path: str,
    index: int,
    barrier: MpBarrier,
    queue: multiprocessing.Queue[tuple[str, str, str]],
) -> None:
    """Run in a spawned child process: create one racing note."""
    import os

    os.environ["MESH_CONFIG_PATH"] = config_path
    os.environ.pop("MESH_AGENT", None)

    import mesh.core.notes as notes_mod
    from mesh.schemas.config import load_config

    _install_fixed_digest()
    _install_toctou_delay(notes_mod)

    cfg = load_config()
    body = f"racer-{index}-body"
    barrier.wait()
    try:
        note = notes_mod.create_note(cfg, f"Racer {index}", body=body)
        queue.put(("ok", note.id, body))
    except Exception as exc:  # pragma: no cover - surfaced via assertion on RED
        queue.put(("error", repr(exc), body))


def _task_worker(
    config_path: str,
    index: int,
    barrier: MpBarrier,
    queue: multiprocessing.Queue[tuple[str, str, str]],
) -> None:
    """Run in a spawned child process: create one racing task."""
    import os

    os.environ["MESH_CONFIG_PATH"] = config_path
    os.environ.pop("MESH_AGENT", None)

    import mesh.core.tasks as tasks_mod
    from mesh.schemas.config import load_config

    _install_fixed_digest()
    _install_toctou_delay(tasks_mod)

    cfg = load_config()
    body = f"racer-{index}-body"
    barrier.wait()
    try:
        task = tasks_mod.create_task(cfg, f"Racer {index}", body=body)
        queue.put(("ok", task.id, body))
    except Exception as exc:  # pragma: no cover - surfaced via assertion on RED
        queue.put(("error", repr(exc), body))


def _run_race(worker: Callable[..., None], config_path: Path, n: int) -> list[tuple[str, str, str]]:
    """Spawn ``n`` separate OS processes running ``worker`` and collect their results."""
    ctx = multiprocessing.get_context("spawn")
    barrier = ctx.Barrier(n)
    queue: multiprocessing.Queue[tuple[str, str, str]] = ctx.Queue()
    procs = [
        ctx.Process(target=worker, args=(str(config_path), i, barrier, queue)) for i in range(n)
    ]
    for proc in procs:
        proc.start()
    results = [queue.get(timeout=_QUEUE_TIMEOUT) for _ in range(n)]
    for proc in procs:
        proc.join(timeout=_QUEUE_TIMEOUT)
        assert proc.exitcode == 0, f"worker process crashed: exitcode={proc.exitcode}"
    return results


# --------------------------------------------------------------------------- #
# The race: N distinct processes, N distinct ids, N distinct bodies -- no loss #
# --------------------------------------------------------------------------- #


def test_concurrent_note_creates_no_lost_content(
    mesh_config: Path, vault: Path, config_path: Path
) -> None:
    """N separate processes race a colliding note id candidate; all N survive intact."""
    results = _run_race(_note_worker, config_path, N_RACERS)

    errors = [r for r in results if r[0] == "error"]
    assert not errors, errors

    ids = [r[1] for r in results]
    assert len(set(ids)) == N_RACERS, ids  # N distinct ids -- no id collision survived

    # The extension loop was exercised: exactly one winner per length, starting at
    # MIN_LENGTH (4) -- i.e. the second (and third, ...) creator's id is longer.
    id_lengths = sorted(len(i) - len("n-") for i in ids)
    assert id_lengths == list(range(4, 4 + N_RACERS)), ids

    on_disk = sorted((vault / "notes").glob("n-*.md"))
    assert len(on_disk) == N_RACERS, on_disk  # N distinct files -- no os.replace clobber

    bodies_by_id = {r[1]: r[2] for r in results}
    for path in on_disk:
        note_id = path.stem
        post = frontmatter.loads(path.read_text(encoding="utf-8"))
        assert post.metadata["id"] == note_id
        assert post.content == bodies_by_id[note_id]  # exact body survived, untouched


def test_concurrent_task_creates_no_lost_content(
    mesh_config: Path, vault: Path, config_path: Path
) -> None:
    """N separate processes race a colliding task id candidate; all N survive intact."""
    results = _run_race(_task_worker, config_path, N_RACERS)

    errors = [r for r in results if r[0] == "error"]
    assert not errors, errors

    ids = [r[1] for r in results]
    assert len(set(ids)) == N_RACERS, ids

    id_lengths = sorted(len(i) - len("t-") for i in ids)
    assert id_lengths == list(range(4, 4 + N_RACERS)), ids

    on_disk = sorted((vault / "tasks" / "open").glob("t-*.md"))
    assert len(on_disk) == N_RACERS, on_disk

    bodies_by_id = {r[1]: r[2] for r in results}
    for path in on_disk:
        task_id = path.stem
        post = frontmatter.loads(path.read_text(encoding="utf-8"))
        assert post.metadata["id"] == task_id
        assert post.content == bodies_by_id[task_id]


# --------------------------------------------------------------------------- #
# Create still works with the daemon down -- the allocator lock adds no dep    #
# --------------------------------------------------------------------------- #


def test_create_note_succeeds_with_daemon_down(mesh_config: Path, vault: Path) -> None:
    """``create_note`` never talks to a daemon -- it must not gate on one being up."""
    import mesh.core.notes as notes_mod
    from mesh.schemas.config import load_config

    assert "daemon" not in notes_mod.__dict__  # no daemon import, no daemon dependency
    cfg = load_config()
    note = notes_mod.create_note(cfg, "Solo note", body="solo body")
    assert (vault / "notes" / f"{note.id}.md").exists()


def test_create_task_succeeds_with_daemon_down(mesh_config: Path, vault: Path) -> None:
    """``create_task`` never talks to a daemon -- it must not gate on one being up."""
    import mesh.core.tasks as tasks_mod
    from mesh.schemas.config import load_config

    assert "daemon" not in tasks_mod.__dict__  # no daemon import, no daemon dependency
    cfg = load_config()
    task = tasks_mod.create_task(cfg, "Solo task", body="solo body")
    assert (vault / "tasks" / "open" / f"{task.id}.md").exists()
