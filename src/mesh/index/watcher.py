"""Watchdog event adapter + the daemon-owned change-hook registry.

This module wires the one Markdown folder's filesystem events to the warm index
and folder reconcile, and fans each settled change out to subscribers:

* **:class:`ChangeHooks`** — a small fan-out registry of ``Callable[[Path], None]``
  subscribers. It replaces the former module-level mutable global: the daemon
  server owns one instance and hands it to the :class:`Watcher`, so there is no
  hidden cross-module coupling and a restart never stacks a stale hook.

* **:class:`Watcher`** — wraps a real :class:`watchdog.observers.Observer` watching
  the ``notes/`` and ``tasks/`` subtrees for the four filesystem event kinds. Each
  event drives reparse / evict / reconcile and then fires the watcher's
  :class:`ChangeHooks`, so other subsystems (search registers its
  ``indexed_client`` re-index here) subscribe without editing this file.
  Directory events get their own branch: a subtree rename is reported as *one*
  event with no per-file events behind it, so the folder is walked (see
  :meth:`Watcher._handle_directory_event`).

* **:class:`VaultEventHandler`** — the thin :class:`watchdog.events.FileSystemEventHandler`
  adapter that forwards each callback to :meth:`Watcher.handle_event`.

The heavy platform observer backend (fsevents/inotify/kqueue) is imported lazily
inside :meth:`Watcher.start` so a daemon-less CLI never pays for it;
``watchdog.events`` alone is light and needed at class-definition time. The
byte-preserving reconcile (and its exception guard) lives in
:mod:`mesh.index.reconcile`; the warm index in :mod:`mesh.index.warm`.
"""

from __future__ import annotations

import contextlib
import os
import queue
import threading
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING

from watchdog.events import (
    EVENT_TYPE_CREATED,
    EVENT_TYPE_DELETED,
    EVENT_TYPE_MOVED,
    FileSystemEvent,
    FileSystemEventHandler,
    FileSystemMovedEvent,
)

from mesh.index.reconcile import reconcile_path
from mesh.index.warm import VaultIndex, iter_vault_md
from mesh.schemas.config import Config
from mesh.storage.files import iter_md

if TYPE_CHECKING:
    # The platform observer backend (fsevents/inotify/kqueue) is a heavy import;
    # defer it to ``Watcher.start`` so a daemon-less CLI never pays for it.
    # ``watchdog.events`` (above) is light and is needed at class-definition time,
    # so it stays a top-level import.
    from watchdog.observers.api import BaseObserver

__all__ = ["ChangeHooks", "VaultEventHandler", "Watcher"]

#: The vault subtrees the observer watches — the only place a row can come from.
_WATCHED_SUBS = ("notes", "tasks")


# --------------------------------------------------------------------------- #
# Change-hook registry (daemon-owned, multi-consumer)                          #
# --------------------------------------------------------------------------- #


# Bounded backlog of paths awaiting hook delivery. Deep enough to absorb a
# burst (a git pull, a scripted import); shallow enough that a wedged hook
# cannot turn the queue into a memory leak.
_HOOK_QUEUE_MAX = 1024


class ChangeHooks:
    """A fan-out registry of vault-change subscribers, owned by the daemon server.

    The daemon creates one instance, hands it to the :class:`Watcher`, and
    registers its own re-index hook on it; a clean stop clears it. Because the
    registry is an owned object (not a module-level global), a restart can never
    stack a stale hook, and no other module can reach in and mutate it implicitly.
    """

    def __init__(self) -> None:
        self._hooks: list[Callable[[Path], None]] = []

    def register(self, hook: Callable[[Path], None]) -> None:
        """Subscribe ``hook`` to vault-change notifications (search registers here)."""
        self._hooks.append(hook)

    def clear(self) -> None:
        """Remove every registered hook (daemon stop; test isolation)."""
        self._hooks.clear()

    def fire(self, path: Path) -> None:
        """Fan a vault change out to every registered hook — called after each cycle."""
        for hook in list(self._hooks):
            hook(path)


# --------------------------------------------------------------------------- #
# Watcher + watchdog event adapter                                            #
# --------------------------------------------------------------------------- #


class VaultEventHandler(FileSystemEventHandler):
    """Thin watchdog adapter: forwards every file event to :class:`Watcher`.

    Every callback routes through :meth:`_guarded`, because watchdog does not
    isolate handler failures: one exception escaping here kills the observer
    thread, and the warm index then silently stops updating for the daemon's whole
    life with nothing surfaced anywhere. That is the failure mode recorded in
    `.spec/lessons.md` § "An exception in a watcher thread silently kills
    freshness" — a transient race must degrade one event, never the watcher.
    """

    def __init__(self, watcher: Watcher) -> None:
        self._watcher = watcher

    def _guarded(self, event: FileSystemEvent) -> None:
        try:
            self._watcher.handle_event(event)
        except Exception:  # noqa: BLE001 - a bad event must not kill the observer
            return

    def on_created(self, event: FileSystemEvent) -> None:
        self._guarded(event)

    def on_modified(self, event: FileSystemEvent) -> None:
        self._guarded(event)

    def on_moved(self, event: FileSystemEvent) -> None:
        self._guarded(event)

    def on_deleted(self, event: FileSystemEvent) -> None:
        self._guarded(event)


class Watcher:
    """Owns a watchdog observer over ``notes/``/``tasks/`` and the warm index."""

    def __init__(self, config: Config, index: VaultIndex, hooks: ChangeHooks | None = None) -> None:
        self._config = config
        self._index = index
        self._hooks = hooks if hooks is not None else ChangeHooks()
        self._handler = VaultEventHandler(self)
        self._observer: BaseObserver | None = None
        self._watched: list[str] = []
        # Hook delivery runs on its own thread. The registered hook shells out to
        # ``indexed index update``, a blocking subprocess: fired inline it made
        # every filesystem event wait for search re-indexing before the next event
        # could be reparsed, so a burst of writes pushed the warm index seconds
        # behind disk while reads kept answering confidently from stale RAM. The
        # queue is bounded so an unbounded backlog cannot grow into memory
        # pressure either; when it is full the oldest pending path is dropped,
        # because search freshness is best-effort and index freshness is not.
        self._hook_queue: queue.Queue[Path | None] = queue.Queue(maxsize=_HOOK_QUEUE_MAX)
        self._hook_worker: threading.Thread | None = None

    @property
    def index(self) -> VaultIndex:
        return self._index

    @property
    def hooks(self) -> ChangeHooks:
        """The change-hook registry this watcher fires after each settled event."""
        return self._hooks

    @property
    def handler(self) -> VaultEventHandler:
        return self._handler

    @property
    def watched_paths(self) -> set[str]:
        """The subtree paths scheduled with the observer (``notes/``, ``tasks/``)."""
        return set(self._watched)

    # -- lifecycle --------------------------------------------------------- #

    def warm(self) -> None:
        """Populate the index from an initial full scan of the vault."""
        for path in iter_vault_md(self._config.core.vault_path):
            self._index.reparse(path)

    def start(self) -> None:
        """Warm the index, then start watching ``notes/`` and ``tasks/`` recursively."""
        from watchdog.observers import Observer  # lazy: heavy platform backend

        self.warm()
        vault = self._config.core.vault_path
        observer = Observer()
        self._watched = []
        for sub in _WATCHED_SUBS:
            folder = vault / sub
            folder.mkdir(parents=True, exist_ok=True)
            observer.schedule(self._handler, str(folder), recursive=True)
            self._watched.append(str(folder))
        observer.start()
        self._observer = observer
        self._start_hook_worker()

    def is_alive(self) -> bool:
        return self._observer is not None and self._observer.is_alive()

    def stop(self) -> None:
        """Stop and join the observer thread, then flush the index."""
        if self._observer is not None:
            self._observer.stop()
            self._observer.join(timeout=5)
            self._observer = None
        self._stop_hook_worker()
        self._index.clear()

    def _start_hook_worker(self) -> None:
        """Spawn the thread that delivers change hooks off the observer thread."""
        worker = threading.Thread(target=self._drain_hooks, name="mesh-hooks", daemon=True)
        self._hook_worker = worker
        worker.start()

    def _stop_hook_worker(self) -> None:
        """Signal the hook worker to finish and join it."""
        worker = self._hook_worker
        if worker is None:
            return
        self._hook_worker = None
        with contextlib.suppress(queue.Full):
            self._hook_queue.put_nowait(None)  # sentinel: drain and exit
        worker.join(timeout=5)

    def _drain_hooks(self) -> None:
        """Deliver queued paths to the hooks until the sentinel arrives."""
        while True:
            path = self._hook_queue.get()
            if path is None:
                return
            try:
                self._hooks.fire(path)
            except Exception:  # noqa: BLE001 - a failing hook must not end delivery
                continue

    def _queue_hook(self, path: Path) -> None:
        """Hand ``path`` to the hook worker, never blocking the caller.

        Falls back to firing inline when no worker is running (a directly-driven
        Watcher in tests, or an event arriving after ``stop``), so hook delivery
        stays observable without a started observer.
        """
        if self._hook_worker is None:
            self._hooks.fire(path)
            return
        try:
            self._hook_queue.put_nowait(path)
        except queue.Full:
            with contextlib.suppress(queue.Empty):
                self._hook_queue.get_nowait()  # drop the oldest, keep the newest
            with contextlib.suppress(queue.Full):
                self._hook_queue.put_nowait(path)

    # -- event handling ---------------------------------------------------- #

    def handle_event(self, event: FileSystemEvent) -> None:
        """Route one filesystem event through reparse / evict / reconcile + hook."""
        if event.is_directory:
            self._handle_directory_event(event)
            return
        if event.event_type == EVENT_TYPE_DELETED:
            path = Path(os.fsdecode(event.src_path))
            self._index.evict(path)
            self._queue_hook(path)
            return
        if event.event_type == EVENT_TYPE_MOVED and isinstance(event, FileSystemMovedEvent):
            src = Path(os.fsdecode(event.src_path))
            self._index.evict(src)
            final = self._process(Path(os.fsdecode(event.dest_path)))
            self._queue_hook(final)
            return
        final = self._process(Path(os.fsdecode(event.src_path)))
        self._queue_hook(final)

    def _handle_directory_event(self, event: FileSystemEvent) -> None:
        """Apply a whole-subtree event — the one shape with no per-file events behind it.

        Watchdog reports ``mv notes/decisions ../archive`` as a *single*
        ``DirMovedEvent``: no file event ever announces the rows that just left (or
        arrived), so dropping directory events left phantom rows served for the
        daemon's whole lifetime and never indexed a folder moved *in*. The subtree
        is therefore walked here — evict everything under the source, re-index
        everything under a destination that lands inside the watched tree.

        ``shutil.rmtree`` and a plain ``mkdir`` need nothing from this branch (the
        first emits a delete per file, the second arrives empty), and directory
        *modified* events — one per file write, by far the most common kind — are
        deliberately ignored: walking on each would make every write O(folder).
        """
        src = Path(os.fsdecode(event.src_path))
        if event.event_type == EVENT_TYPE_DELETED:
            self._evict_subtree(src)
            return
        if event.event_type == EVENT_TYPE_MOVED and isinstance(event, FileSystemMovedEvent):
            self._evict_subtree(src)
            self._index_subtree(Path(os.fsdecode(event.dest_path)))
            return
        if event.event_type == EVENT_TYPE_CREATED:
            # A folder moved in from outside the watch can surface as a creation
            # (an unpaired ``IN_MOVED_TO``), with its files never announced.
            self._index_subtree(src)

    def _evict_subtree(self, root: Path) -> None:
        """Drop every indexed row under ``root`` and tell the hooks about each one."""
        for path in self._index.paths_under(root):
            self._index.evict(path)
            self._hooks.fire(path)

    def _index_subtree(self, root: Path) -> None:
        """Reconcile + index every ``*.md`` under ``root``, if it is inside the watch."""
        if not self._inside_watched_tree(root):
            return
        for path in iter_md(root):
            self._hooks.fire(self._process(path))

    def _inside_watched_tree(self, path: Path) -> bool:
        """Whether ``path`` is inside ``notes/`` or ``tasks/`` — the only row sources."""
        vault = self._config.core.vault_path
        return any(path.is_relative_to(vault / sub) for sub in _WATCHED_SUBS)

    def _process(self, path: Path) -> Path:
        """Reconcile then reparse ``path``; returns its final (possibly moved) path."""
        if path.suffix != ".md":
            return path
        if not path.exists():
            self._index.evict(path)
            return path
        final = reconcile_path(self._config, path)
        if final != path:
            self._index.evict(path)
        self._index.reparse(final)
        return final
