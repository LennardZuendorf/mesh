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

* **:class:`VaultEventHandler`** — the thin :class:`watchdog.events.FileSystemEventHandler`
  adapter that forwards each callback to :meth:`Watcher.handle_event`.

The heavy platform observer backend (fsevents/inotify/kqueue) is imported lazily
inside :meth:`Watcher.start` so a daemon-less CLI never pays for it;
``watchdog.events`` alone is light and needed at class-definition time. The
byte-preserving reconcile (and its exception guard) lives in
:mod:`shards.index.reconcile`; the warm index in :mod:`shards.index.warm`.
"""

from __future__ import annotations

import os
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING

from watchdog.events import (
    EVENT_TYPE_DELETED,
    EVENT_TYPE_MOVED,
    FileSystemEvent,
    FileSystemEventHandler,
    FileSystemMovedEvent,
)

from shards.index.reconcile import reconcile_path
from shards.index.warm import VaultIndex, _iter_vault_md
from shards.schemas.config import Config

if TYPE_CHECKING:
    # The platform observer backend (fsevents/inotify/kqueue) is a heavy import;
    # defer it to ``Watcher.start`` so a daemon-less CLI never pays for it.
    # ``watchdog.events`` (above) is light and is needed at class-definition time,
    # so it stays a top-level import.
    from watchdog.observers.api import BaseObserver

__all__ = ["ChangeHooks", "VaultEventHandler", "Watcher"]


# --------------------------------------------------------------------------- #
# Change-hook registry (daemon-owned, multi-consumer)                          #
# --------------------------------------------------------------------------- #


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
    """Thin watchdog adapter: forwards every file event to :class:`Watcher`."""

    def __init__(self, watcher: Watcher) -> None:
        self._watcher = watcher

    def on_created(self, event: FileSystemEvent) -> None:
        self._watcher.handle_event(event)

    def on_modified(self, event: FileSystemEvent) -> None:
        self._watcher.handle_event(event)

    def on_moved(self, event: FileSystemEvent) -> None:
        self._watcher.handle_event(event)

    def on_deleted(self, event: FileSystemEvent) -> None:
        self._watcher.handle_event(event)


class Watcher:
    """Owns a watchdog observer over ``notes/``/``tasks/`` and the warm index."""

    def __init__(self, config: Config, index: VaultIndex, hooks: ChangeHooks | None = None) -> None:
        self._config = config
        self._index = index
        self._hooks = hooks if hooks is not None else ChangeHooks()
        self._handler = VaultEventHandler(self)
        self._observer: BaseObserver | None = None
        self._watched: list[str] = []

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
        for path in _iter_vault_md(self._config.core.tolaria_path):
            self._index.reparse(path)

    def start(self) -> None:
        """Warm the index, then start watching ``notes/`` and ``tasks/`` recursively."""
        from watchdog.observers import Observer  # lazy: heavy platform backend

        self.warm()
        vault = self._config.core.tolaria_path
        observer = Observer()
        self._watched = []
        for sub in ("notes", "tasks"):
            folder = vault / sub
            folder.mkdir(parents=True, exist_ok=True)
            observer.schedule(self._handler, str(folder), recursive=True)
            self._watched.append(str(folder))
        observer.start()
        self._observer = observer

    def is_alive(self) -> bool:
        return self._observer is not None and self._observer.is_alive()

    def stop(self) -> None:
        """Stop and join the observer thread, then flush the index."""
        if self._observer is not None:
            self._observer.stop()
            self._observer.join(timeout=5)
            self._observer = None
        self._index.clear()

    # -- event handling ---------------------------------------------------- #

    def handle_event(self, event: FileSystemEvent) -> None:
        """Route one filesystem event through reparse / evict / reconcile + hook."""
        if event.is_directory:
            return
        if event.event_type == EVENT_TYPE_DELETED:
            path = Path(os.fsdecode(event.src_path))
            self._index.evict(path)
            self._hooks.fire(path)
            return
        if event.event_type == EVENT_TYPE_MOVED and isinstance(event, FileSystemMovedEvent):
            src = Path(os.fsdecode(event.src_path))
            self._index.evict(src)
            final = self._process(Path(os.fsdecode(event.dest_path)))
            self._hooks.fire(final)
            return
        final = self._process(Path(os.fsdecode(event.src_path)))
        self._hooks.fire(final)

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
