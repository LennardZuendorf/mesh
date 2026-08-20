"""daemon/2 — watcher + hook: warm index, folder reconcile, change hooks.

Coverage maps 1:1 to the unit's acceptance criteria:

* **Index** — a ``VaultIndex`` keyed by shards id: ``reparse`` reads frontmatter for
  a path, ``evict`` drops a (possibly already-gone) path without error, ``recent``
  returns mtime-sorted rows.
* **Reconcile** — a file whose frontmatter ``status``/``type`` maps to a different
  subdir (per ``storage/files.py``) is relocated to the correct folder; crucially
  the on-disk ``updated`` field is **not** bumped (byte-identical move), which is
  what distinguishes a watcher move from a user edit.
* **Handler** — the four watchdog event kinds (created/modified/moved/deleted)
  drive reparse / evict / reconcile, and every cycle ends by firing the watcher's
  ``ChangeHooks`` registry (so search's ``indexed_client`` can subscribe without
  editing the watcher).
* **activity.recent** — served from the warm in-process index over the real
  socket (JSON-serializable), and falling back to an mtime-sorted dir scan when
  the daemon is down.
* **Wiring** — the daemon warms the index before the socket accepts connections,
  and a clean stop joins the observer thread and flushes the index.

Reconcile/hook behaviour is exercised by driving ``handle_event`` /
``reconcile_path`` directly (deterministic); one tolerant test starts a real
``Observer`` on a correctly-placed file. Sockets live under a short ``/tmp`` dir
because ``AF_UNIX`` paths are length-capped.
"""

from __future__ import annotations

import asyncio
import os
import time
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest
from watchdog.events import (
    FileCreatedEvent,
    FileDeletedEvent,
    FileModifiedEvent,
    FileMovedEvent,
)

from shards.daemon.client import DaemonClient, DaemonError
from shards.daemon.server import DaemonServer
from shards.index.reconcile import reconcile_path
from shards.index.warm import DEFAULT_RECENT_LIMIT, VaultIndex, scan_recent
from shards.index.watcher import ChangeHooks, Watcher
from shards.schemas.config import Config, load_config
from shards.storage.sandbox import safe_resolve
from tests.daemon.conftest import running_daemon

# --------------------------------------------------------------------------- #
# Fixtures & helpers                                                          #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _write_note(
    vault: Path,
    *,
    note_id: str,
    note_type: str = "note",
    title: str = "A Note",
    folder: str | None = None,
    extra: dict[str, object] | None = None,
    mtime: float | None = None,
    body: str = "Body line.",
) -> Path:
    """Write a note ``.md`` into ``folder`` (default: the type's correct folder)."""
    when = datetime.now(UTC)
    meta: dict[str, object] = {
        "id": note_id,
        "type": note_type,
        "title": title,
        "tags": [],
        "owner": "seed-agent",
        "created": when,
        "updated": when,
        "related": [],
    }
    if extra:
        meta.update(extra)
    sub = folder if folder is not None else _note_sub(note_type)
    dest_dir = vault / sub
    dest_dir.mkdir(parents=True, exist_ok=True)
    path = dest_dir / f"{note_id}.md"
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    if mtime is not None:
        os.utime(path, (mtime, mtime))
    return path


def _note_sub(note_type: str) -> str:
    return {
        "note": "notes",
        "log": "notes/logs",
        "decision": "notes/decisions",
        "reference": "notes/references",
    }[note_type]


def _write_task(
    vault: Path,
    *,
    task_id: str,
    status: str = "open",
    title: str = "Seed Task",
    folder: str | None = None,
    extra: dict[str, object] | None = None,
    mtime: float | None = None,
    body: str = "Task body.",
) -> Path:
    """Write a task ``.md`` into ``folder`` (default: the status's correct folder)."""
    when = datetime.now(UTC)
    meta: dict[str, object] = {
        "id": task_id,
        "type": "task",
        "title": title,
        "tags": [],
        "owner": "seed-agent",
        "created": when,
        "updated": when,
        "related": [],
        "status": status,
        "priority": None,
        "claimed_by": None,
        "blocks": [],
        "blocked_by": [],
    }
    if extra:
        meta.update(extra)
    sub = (
        folder
        if folder is not None
        else ("tasks/done" if status in {"done", "cancelled"} else "tasks/open")
    )
    dest_dir = vault / sub
    dest_dir.mkdir(parents=True, exist_ok=True)
    path = dest_dir / f"{task_id}.md"
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    if mtime is not None:
        os.utime(path, (mtime, mtime))
    return path


# --------------------------------------------------------------------------- #
# VaultIndex — reparse / evict / recent                                       #
# --------------------------------------------------------------------------- #


def test_reparse_indexes_note_by_id(vault: Path) -> None:
    path = _write_note(vault, note_id="n-idx", title="Indexed Note")
    index = VaultIndex()
    index.reparse(path)
    entry = index.get("n-idx")
    assert entry is not None
    assert entry.id == "n-idx"
    assert entry.mtime > 0
    assert entry.meta["title"] == "Indexed Note"


def test_reparse_skips_file_without_shards_id(vault: Path) -> None:
    foreign = vault / "notes" / "tolaria.md"
    foreign.write_text(frontmatter.dumps(frontmatter.Post("x", title="Tolaria")), encoding="utf-8")
    index = VaultIndex()
    index.reparse(foreign)
    assert len(index) == 0


def test_evict_removes_entry(vault: Path) -> None:
    path = _write_note(vault, note_id="n-ev", title="Evict Me")
    index = VaultIndex()
    index.reparse(path)
    assert index.get("n-ev") is not None
    index.evict(path)
    assert index.get("n-ev") is None


def test_evict_missing_path_does_not_error(vault: Path) -> None:
    index = VaultIndex()
    # A path that was never indexed (and does not exist) must evict silently.
    index.evict(vault / "notes" / "n-ghost.md")
    assert len(index) == 0


def test_reparse_after_delete_evicts(vault: Path) -> None:
    path = _write_note(vault, note_id="n-gone", title="Gone")
    index = VaultIndex()
    index.reparse(path)
    path.unlink()
    index.reparse(path)  # file vanished mid-reparse — treated as an eviction
    assert index.get("n-gone") is None


# --------------------------------------------------------------------------- #
# reconcile_path — folder mismatch move, no `updated` bump                     #
# --------------------------------------------------------------------------- #


def test_reconcile_moves_task_status_mismatch(cfg: Config, vault: Path) -> None:
    # A done task wrongly sitting in tasks/open/ must move to tasks/done/.
    path = _write_task(vault, task_id="t-mv", status="done", folder="tasks/open")
    final = reconcile_path(cfg, path)
    assert final == (vault / "tasks" / "done" / "t-mv.md").resolve()
    assert final.exists()
    assert not path.exists()


def test_reconcile_moves_note_type_mismatch(cfg: Config, vault: Path) -> None:
    # A decision note wrongly in notes/ root must move to notes/decisions/.
    path = _write_note(vault, note_id="n-dec", note_type="decision", folder="notes")
    final = reconcile_path(cfg, path)
    assert final == (vault / "notes" / "decisions" / "n-dec.md").resolve()
    assert not path.exists()


def test_reconcile_does_not_bump_updated_and_roundtrips_unknown_keys(
    cfg: Config, vault: Path
) -> None:
    path = _write_task(
        vault,
        task_id="t-keep",
        status="done",
        folder="tasks/open",
        extra={"reviewer": "alice"},
    )
    original = path.read_text(encoding="utf-8")
    final = reconcile_path(cfg, path)
    moved = final.read_text(encoding="utf-8")
    # Byte-identical move: `updated` is untouched and the unknown `reviewer` key
    # survives — the load-bearing distinction from a user edit.
    assert moved == original
    assert "reviewer: alice" in moved


def test_reconcile_leaves_correctly_placed_file(cfg: Config, vault: Path) -> None:
    path = _write_note(vault, note_id="n-ok", note_type="note")  # correct folder
    final = reconcile_path(cfg, path)
    assert final == path.resolve()
    assert path.exists()


def test_reconcile_ignores_foreign_file(cfg: Config, vault: Path) -> None:
    foreign = vault / "tasks" / "open" / "tolaria.md"
    foreign.parent.mkdir(parents=True, exist_ok=True)
    foreign.write_text(
        frontmatter.dumps(frontmatter.Post("x", title="Foreign", status="done")),
        encoding="utf-8",
    )
    final = reconcile_path(cfg, foreign)
    assert final == foreign.resolve()
    assert foreign.exists()  # no shards id → never moved


def test_reconcile_returns_unmoved_when_source_races_away(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """FIX 2: a source that vanishes at rename time must not raise out of reconcile.

    An unguarded ``os.replace`` would let ``FileNotFoundError`` escape the watchdog
    event-handler thread and silently kill the observer for the daemon's lifetime.
    """
    path = _write_task(vault, task_id="t-race", status="done", folder="tasks/open")

    def _vanish(_src: object, _dst: object) -> None:
        raise FileNotFoundError

    monkeypatch.setattr("shards.index.reconcile.os.replace", _vanish)
    result = reconcile_path(cfg, path)  # must not raise
    # Left in place (move failed); the resolved original path is returned so a
    # later event can reconcile it.
    assert result == safe_resolve(cfg.core.vault_path, path)
    assert path.exists()


# --------------------------------------------------------------------------- #
# Watcher.handle_event — routing for the four event kinds                      #
# --------------------------------------------------------------------------- #


def test_handle_created_event_reparses(cfg: Config, vault: Path) -> None:
    path = _write_note(vault, note_id="n-cre", title="Created")
    watcher = Watcher(cfg, VaultIndex())
    watcher.handle_event(FileCreatedEvent(str(path)))
    assert watcher.index.get("n-cre") is not None


def test_handle_modified_event_reparses(cfg: Config, vault: Path) -> None:
    path = _write_note(vault, note_id="n-mod", title="Modified")
    watcher = Watcher(cfg, VaultIndex())
    watcher.handle_event(FileModifiedEvent(str(path)))
    assert watcher.index.get("n-mod") is not None


def test_handle_deleted_event_evicts(cfg: Config, vault: Path) -> None:
    path = _write_note(vault, note_id="n-del", title="Delete")
    index = VaultIndex()
    index.reparse(path)
    path.unlink()
    watcher = Watcher(cfg, index)
    watcher.handle_event(FileDeletedEvent(str(path)))
    assert index.get("n-del") is None


def test_handle_moved_event_evicts_src_reparses_dest(cfg: Config, vault: Path) -> None:
    src = vault / "notes" / "n-mvd.md"
    dest = _write_note(vault, note_id="n-mvd", title="Moved")
    index = VaultIndex()
    watcher = Watcher(cfg, index)
    watcher.handle_event(FileMovedEvent(str(src), str(dest)))
    assert index.get("n-mvd") is not None


def test_handle_event_reconciles_folder_mismatch(cfg: Config, vault: Path) -> None:
    # A modified event on a mis-filed task triggers a reconcile move.
    path = _write_task(vault, task_id="t-rec", status="done", folder="tasks/open")
    index = VaultIndex()
    watcher = Watcher(cfg, index)
    watcher.handle_event(FileModifiedEvent(str(path)))
    entry = index.get("t-rec")
    assert entry is not None
    assert entry.path.resolve() == (vault / "tasks" / "done" / "t-rec.md").resolve()
    assert (vault / "tasks" / "done" / "t-rec.md").exists()
    assert not path.exists()


def test_event_handler_routes_created(cfg: Config, vault: Path) -> None:
    """The FileSystemEventHandler adapter forwards events to the watcher."""
    path = _write_note(vault, note_id="n-route", title="Route")
    watcher = Watcher(cfg, VaultIndex())
    watcher.handler.on_created(FileCreatedEvent(str(path)))
    assert watcher.index.get("n-route") is not None


# --------------------------------------------------------------------------- #
# ChangeHooks — the watcher's owned, multi-consumer hook registry              #
# --------------------------------------------------------------------------- #


def test_watcher_fires_registered_hook(cfg: Config, vault: Path) -> None:
    seen: list[Path] = []
    watcher = Watcher(cfg, VaultIndex())
    watcher.hooks.register(seen.append)
    path = _write_note(vault, note_id="n-hook", title="Hook")
    watcher.handle_event(FileModifiedEvent(str(path)))
    assert seen and seen[-1].resolve() == path.resolve()


def test_multiple_hooks_all_fire(cfg: Config, vault: Path) -> None:
    a: list[Path] = []
    b: list[Path] = []
    watcher = Watcher(cfg, VaultIndex())
    watcher.hooks.register(a.append)
    watcher.hooks.register(b.append)
    path = _write_note(vault, note_id="n-multi", title="Multi")
    watcher.handle_event(FileCreatedEvent(str(path)))
    assert a and b


def test_change_hooks_fire_all_registered(tmp_path: Path) -> None:
    """A consumer subscribes via ``ChangeHooks.register`` and ``fire`` fans out."""
    seen: list[Path] = []
    hooks = ChangeHooks()
    hooks.register(seen.append)
    target = tmp_path / "anything.md"
    hooks.fire(target)
    assert seen == [target]


def test_change_hooks_clear_drops_subscribers(tmp_path: Path) -> None:
    """``clear`` empties the registry so a later ``fire`` reaches no one."""
    seen: list[Path] = []
    hooks = ChangeHooks()
    hooks.register(seen.append)
    hooks.clear()
    hooks.fire(tmp_path / "anything.md")
    assert seen == []


def test_hook_fires_on_delete(cfg: Config, vault: Path) -> None:
    seen: list[Path] = []
    path = _write_note(vault, note_id="n-dh", title="DelHook")
    index = VaultIndex()
    index.reparse(path)
    path.unlink()
    watcher = Watcher(cfg, index)
    watcher.hooks.register(seen.append)
    watcher.handle_event(FileDeletedEvent(str(path)))
    assert seen and seen[-1].resolve() == path.resolve()


# --------------------------------------------------------------------------- #
# activity.recent — index-served + dir-scan fallback                          #
# --------------------------------------------------------------------------- #


def test_index_recent_sorted_by_mtime_desc_and_limited(vault: Path) -> None:
    p1 = _write_note(vault, note_id="n-1", title="One", mtime=1000.0)
    p2 = _write_note(vault, note_id="n-2", title="Two", mtime=3000.0)
    p3 = _write_note(vault, note_id="n-3", title="Three", mtime=2000.0)
    index = VaultIndex()
    for p in (p1, p2, p3):
        index.reparse(p)
    rows = index.recent(2)
    assert [r["id"] for r in rows] == ["n-2", "n-3"]  # most-recent first, capped at 2


def test_scan_recent_fallback_mtime_sorted_and_shards_ids_only(cfg: Config, vault: Path) -> None:
    _write_note(vault, note_id="n-old", title="Old", mtime=1000.0)
    _write_task(vault, task_id="t-new", status="open", mtime=5000.0)
    # A foreign Tolaria file (no shards id) must be excluded.
    foreign = vault / "notes" / "tolaria.md"
    foreign.write_text(frontmatter.dumps(frontmatter.Post("x", title="Tolaria")), encoding="utf-8")
    rows = scan_recent(cfg, limit=10)
    ids = [r["id"] for r in rows]
    assert ids == ["t-new", "n-old"]  # mtime desc; foreign excluded
    # JSON-serializable payload: no datetime leaks.
    assert all(isinstance(r["mtime"], (int, float)) for r in rows)


def test_scan_recent_respects_limit(cfg: Config, vault: Path) -> None:
    for i, m in enumerate((1000.0, 2000.0, 3000.0)):
        _write_note(vault, note_id=f"n-{i}", title=f"N{i}", mtime=m)
    rows = scan_recent(cfg, limit=1)
    assert len(rows) == 1
    assert rows[0]["id"] == "n-2"


def test_activity_recent_over_socket_serves_warm_index(
    cfg: Config, vault: Path, socket_path: Path
) -> None:
    _write_note(vault, note_id="n-warm-a", title="A", mtime=1000.0)
    _write_note(vault, note_id="n-warm-b", title="B", mtime=2000.0)
    _write_task(vault, task_id="t-warm", status="open", mtime=3000.0)
    with running_daemon(socket_path, config=cfg):
        client = DaemonClient(socket_path=socket_path)
        result = client.activity_recent(cfg, limit=10)
    ids = [r["id"] for r in result["entries"]]
    assert ids == ["t-warm", "n-warm-b", "n-warm-a"]


def test_activity_recent_falls_back_when_daemon_down(
    cfg: Config, vault: Path, missing_socket: Path
) -> None:
    _write_note(vault, note_id="n-fb", title="FB", mtime=1000.0)
    _write_task(vault, task_id="t-fb", status="open", mtime=2000.0)
    client = DaemonClient(socket_path=missing_socket)
    result = client.activity_recent(cfg, limit=10)
    assert [r["id"] for r in result["entries"]] == ["t-fb", "n-fb"]


def test_activity_recent_is_unknown_without_config(cfg: Config, socket_path: Path) -> None:
    """A daemon started without a vault config registers no activity.recent handler.

    core-hardening/5 culled the ``503`` stub table: a handler is either wired (at
    config-ful startup, over the warm index) or the method is simply unknown. The
    ``activity.recent`` verb deliberately passes an *empty* fallback-code set, so
    that ``404`` propagates as a server-state signal — the ``recent-activity``
    lens catches it and scans the folder itself.
    """
    with running_daemon(socket_path, config=None):
        client = DaemonClient(socket_path=socket_path)
        with pytest.raises(DaemonError) as excinfo:
            client.activity_recent(cfg, limit=5)
    assert excinfo.value.code == 404


def test_default_recent_limit_is_sane() -> None:
    assert isinstance(DEFAULT_RECENT_LIMIT, int) and DEFAULT_RECENT_LIMIT > 0


# --------------------------------------------------------------------------- #
# Daemon wiring — warm before accept, clean stop joins watcher + flushes index #
# --------------------------------------------------------------------------- #


def test_watcher_schedules_notes_and_tasks_subtrees(cfg: Config, vault: Path) -> None:
    watcher = Watcher(cfg, VaultIndex())
    watcher.start()
    try:
        watched = {os.path.realpath(p) for p in watcher.watched_paths}
        assert watched == {
            os.path.realpath(vault / "notes"),
            os.path.realpath(vault / "tasks"),
        }
        assert watcher.is_alive()
    finally:
        watcher.stop()


def test_daemon_start_warms_index_before_serving(
    cfg: Config, vault: Path, socket_path: Path
) -> None:
    _write_note(vault, note_id="n-pre", title="Pre-warmed")
    server = DaemonServer(socket_path, config=cfg)
    loop = asyncio.new_event_loop()
    try:
        loop.run_until_complete(server.start())
        # The socket now accepts connections; the index is already warm.
        assert server._index is not None
        assert server._index.get("n-pre") is not None
        assert server._watcher is not None and server._watcher.is_alive()
    finally:
        loop.run_until_complete(server.stop())
        loop.close()


def test_daemon_stop_joins_watcher_and_flushes_index(
    cfg: Config, vault: Path, socket_path: Path
) -> None:
    _write_note(vault, note_id="n-flush", title="Flush")
    server = DaemonServer(socket_path, config=cfg)
    loop = asyncio.new_event_loop()
    loop.run_until_complete(server.start())
    watcher = server._watcher
    index = server._index
    assert watcher is not None and index is not None
    assert index.get("n-flush") is not None
    loop.run_until_complete(server.stop())
    loop.close()
    assert not watcher.is_alive()  # observer thread joined
    assert len(index) == 0  # index flushed


@pytest.mark.parametrize("_run", [0])
def test_real_observer_picks_up_new_file(cfg: Config, vault: Path, _run: int) -> None:
    """Tolerant end-to-end: a real Observer indexes a newly created note."""
    watcher = Watcher(cfg, VaultIndex())
    watcher.start()
    try:
        assert watcher.is_alive()
        _write_note(vault, note_id="n-live", title="Live")
        deadline = time.monotonic() + 8.0
        while time.monotonic() < deadline:
            if watcher.index.get("n-live") is not None:
                break
            time.sleep(0.05)
        assert watcher.index.get("n-live") is not None
    finally:
        watcher.stop()


# --------------------------------------------------------------------------- #
# Review regressions — hook stacking, is_up seam, lazy observer import         #
# --------------------------------------------------------------------------- #


def test_daemon_owns_change_hooks_and_stop_clears(
    cfg: Config, vault: Path, socket_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The daemon owns its ``ChangeHooks``: a fresh registry per start can't stack a
    stale hook, so each vault change re-indexes exactly once — and ``stop`` clears the
    registry so it can't outlive the index it fed."""
    calls: list[Path] = []
    # The daemon's own hook is ``lambda p: incremental_update(config, p)``; patch the
    # target it resolves so a fired hook is observable.
    monkeypatch.setattr(
        "shards.daemon.server.incremental_update", lambda config, path: calls.append(path)
    )

    with running_daemon(socket_path, config=cfg) as server:
        hooks = server._hooks
        assert hooks is not None
        hooks.fire(Path("x.md"))
        assert len(calls) == 1  # only the daemon's own hook fired

    calls.clear()
    hooks.fire(Path("y.md"))
    assert calls == []  # stop() cleared the registry — nothing fires after shutdown


def test_daemon_client_is_up_reflects_liveness(
    cfg: Config, socket_path: Path, missing_socket: Path
) -> None:
    """``is_up`` is the shared liveness seam: False with no socket, True when served."""
    assert DaemonClient(socket_path=missing_socket).is_up() is False
    with running_daemon(socket_path, config=cfg):
        assert DaemonClient(socket_path=socket_path).is_up() is True


def test_watcher_module_does_not_eagerly_bind_observer_backend() -> None:
    """The heavy platform observer backend is imported lazily inside ``Watcher.start``.

    The warm index (which a daemon-less CLI imports for ``scan_recent``) never touches
    ``watchdog`` at all, and the watcher module defers the platform backend, so neither
    binds ``Observer`` at import time."""
    import shards.index.warm as warm_mod
    import shards.index.watcher as watcher_mod

    assert not hasattr(warm_mod, "Observer")
    assert not hasattr(watcher_mod, "Observer")
    assert not hasattr(watcher_mod, "BaseObserver")
