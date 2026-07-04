"""Watcher, warm frontmatter index, folder reconcile, and the change-hook registry.

This module is the daemon's *freshness* engine. Three responsibilities, all
daemon-optional accelerators over the same one Markdown folder:

* **:class:`VaultIndex`** — an in-process frontmatter index keyed by brain id
  (``n-``/``t-``). ``reparse(path)`` (re)reads one file, ``evict(path)`` drops a
  (possibly already-deleted) path without error, and ``recent(limit)`` returns the
  most-recently-modified rows for ``activity.recent``. It is guarded by a lock
  because the watchdog observer thread writes it while the asyncio server thread
  reads it.

* **:func:`reconcile_path`** — the load-bearing folder healer. A file whose
  frontmatter ``status``/``type`` maps (per :mod:`brain.storage.files`) to a
  different subdirectory than it currently lives in is relocated with a
  byte-preserving ``os.replace`` — *no* frontmatter reserialization, so the
  ``updated`` field is **not** bumped and unknown keys round-trip untouched. That
  byte-identity is exactly what distinguishes a watcher move from a user edit
  (which *does* bump ``updated`` via ``core``).

* **:class:`Watcher`** — wraps a real :class:`watchdog.observers.Observer` watching
  the ``notes/`` and ``tasks/`` subtrees for the four filesystem event kinds. Each
  event drives reparse / evict / reconcile and then calls :func:`on_vault_change`,
  which fans out to a **module-level** hook registry so other subsystems (the
  search feature registers its ``indexed_client`` here) can subscribe without
  editing this file.

:func:`scan_recent` is the daemon-down fallback for ``activity.recent``: an
mtime-sorted dir scan of ``notes/`` and ``tasks/`` (brain-id files only),
returning the same JSON-serializable row shape as the warm index.
"""

from __future__ import annotations

import os
import threading
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import frontmatter
import yaml
from watchdog.events import (
    EVENT_TYPE_DELETED,
    EVENT_TYPE_MOVED,
    FileSystemEvent,
    FileSystemEventHandler,
    FileSystemMovedEvent,
)
from watchdog.observers import Observer
from watchdog.observers.api import BaseObserver

from brain.schemas.config import Config
from brain.storage.files import note_folder, task_folder
from brain.storage.sandbox import safe_resolve

DEFAULT_RECENT_LIMIT = 20
_ID_PREFIXES = ("n-", "t-")


def _is_brain_id(value: object) -> bool:
    return isinstance(value, str) and value.startswith(_ID_PREFIXES)


def _iter_vault_md(vault: Path) -> Iterator[Path]:
    """Yield every candidate ``*.md`` under ``notes/`` and ``tasks/{open,done}/``.

    ``notes/`` is scanned recursively (typed subfolders live under it);
    ``.locks/`` holds no ``.md`` and is naturally excluded.
    """
    notes = vault / "notes"
    if notes.is_dir():
        yield from notes.rglob("*.md")
    tasks = vault / "tasks"
    for sub in ("open", "done"):
        folder = tasks / sub
        if folder.is_dir():
            yield from folder.glob("*.md")


def _entry_dict(path: Path, meta: dict[str, Any], mtime: float) -> dict[str, Any]:
    """A JSON-serializable ``activity.recent`` row.

    Deliberately excludes ``created``/``updated`` (which ``frontmatter`` parses
    back into ``datetime`` objects) so the payload survives ``json.dumps`` over
    the socket unmodified.
    """
    return {
        "id": meta.get("id"),
        "type": meta.get("type"),
        "title": meta.get("title"),
        "path": str(path),
        "mtime": mtime,
    }


# --------------------------------------------------------------------------- #
# In-process frontmatter index                                                #
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class IndexEntry:
    """One indexed note/task: brain id, on-disk path, mtime, and frontmatter."""

    id: str
    path: Path
    mtime: float
    meta: dict[str, Any]


class VaultIndex:
    """Thread-safe in-process frontmatter index keyed by brain id."""

    def __init__(self) -> None:
        self._entries: dict[str, IndexEntry] = {}
        self._by_path: dict[str, str] = {}  # realpath -> id, for path-based eviction
        self._lock = threading.RLock()

    @staticmethod
    def _rp(path: Path) -> str:
        return os.path.realpath(path)

    def reparse(self, path: Path) -> None:
        """(Re)index ``path``; a vanished or non-brain file is a no-op / eviction.

        Only ``*.md`` files whose frontmatter carries a brain id are indexed;
        malformed YAML or a foreign file is skipped silently (never raised). A file
        that no longer exists is evicted instead.
        """
        p = Path(path)
        if p.suffix != ".md":
            return
        try:
            text = p.read_text(encoding="utf-8")
        except (FileNotFoundError, IsADirectoryError):
            self.evict(p)
            return
        except OSError:
            return
        try:
            meta = dict(frontmatter.loads(text).metadata)
        except yaml.YAMLError:
            return
        entry_id = meta.get("id")
        if not _is_brain_id(entry_id):
            return
        assert isinstance(entry_id, str)
        try:
            mtime = p.stat().st_mtime
        except OSError:
            return
        entry = IndexEntry(id=entry_id, path=p, mtime=mtime, meta=meta)
        rp = self._rp(p)
        with self._lock:
            prior = self._entries.get(entry_id)
            if prior is not None:
                self._by_path.pop(self._rp(prior.path), None)
            self._entries[entry_id] = entry
            self._by_path[rp] = entry_id

    def evict(self, path: Path) -> None:
        """Drop the entry whose path matches ``path`` — silent if none does."""
        rp = self._rp(Path(path))
        with self._lock:
            entry_id = self._by_path.pop(rp, None)
            if entry_id is not None:
                self._entries.pop(entry_id, None)

    def get(self, entry_id: str) -> IndexEntry | None:
        with self._lock:
            return self._entries.get(entry_id)

    def recent(self, limit: int = DEFAULT_RECENT_LIMIT) -> list[dict[str, Any]]:
        """Most-recently-modified rows, mtime-descending (id-ascending on ties)."""
        with self._lock:
            entries = list(self._entries.values())
        entries.sort(key=lambda e: e.id)  # stable secondary key
        entries.sort(key=lambda e: e.mtime, reverse=True)
        if limit >= 0:
            entries = entries[:limit]
        return [_entry_dict(e.path, e.meta, e.mtime) for e in entries]

    def clear(self) -> None:
        """Flush the whole index (called on a clean daemon stop)."""
        with self._lock:
            self._entries.clear()
            self._by_path.clear()

    def __len__(self) -> int:
        with self._lock:
            return len(self._entries)


# --------------------------------------------------------------------------- #
# Folder reconcile                                                             #
# --------------------------------------------------------------------------- #


def _correct_folder(config: Config, meta: dict[str, Any]) -> Path | None:
    """The folder ``meta`` *should* live in, or ``None`` if its type/status is unknown."""
    vault = config.core.tolaria_path
    try:
        if meta.get("type") == "task":
            return task_folder(str(meta.get("status")), vault)
        return note_folder(str(meta.get("type", "note")), vault)
    except ValueError:
        return None


def reconcile_path(config: Config, path: Path) -> Path:
    """Move ``path`` into the folder its frontmatter dictates; return the final path.

    A file whose ``status``/``type`` maps to a different subdirectory is relocated
    with a byte-preserving :func:`os.replace` — no frontmatter is reserialized, so
    ``updated`` is left untouched and unknown keys round-trip. Correctly placed,
    foreign (no brain id), malformed, or unknown-type files are left in place and
    their (resolved) path returned.
    """
    p = Path(path)
    if p.suffix != ".md":
        return p
    try:
        text = p.read_text(encoding="utf-8")
    except OSError:
        return p
    try:
        meta = frontmatter.loads(text).metadata
    except yaml.YAMLError:
        return p
    if not _is_brain_id(meta.get("id")):
        return p
    folder = _correct_folder(config, meta)
    if folder is None:
        return p
    vault = config.core.tolaria_path
    try:
        src = safe_resolve(vault, p)
        dest = safe_resolve(vault, folder / p.name)
    except ValueError:
        return p
    if src == dest:
        return src
    try:
        dest.parent.mkdir(parents=True, exist_ok=True)
        os.replace(src, dest)  # atomic rename; content (and `updated`) preserved verbatim
    except OSError:
        # The source raced away (concurrent delete/move) between the checks above
        # and the rename. Swallow it and leave the file where it is — a later event
        # will reconcile. Letting it escape would kill the watchdog observer thread
        # and freeze freshness for the daemon's whole lifetime.
        return src
    return dest


# --------------------------------------------------------------------------- #
# Change-hook registry (module-level, multi-consumer)                         #
# --------------------------------------------------------------------------- #

_change_hooks: list[Callable[[Path], None]] = []


def register_change_hook(hook: Callable[[Path], None]) -> None:
    """Subscribe ``hook`` to vault-change notifications (search registers here)."""
    _change_hooks.append(hook)


def clear_change_hooks() -> None:
    """Remove every registered hook (used to isolate tests)."""
    _change_hooks.clear()


def on_vault_change(path: Path) -> None:
    """Fan a vault change out to every registered hook — called after each cycle."""
    for hook in list(_change_hooks):
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

    def __init__(self, config: Config, index: VaultIndex) -> None:
        self._config = config
        self._index = index
        self._handler = VaultEventHandler(self)
        self._observer: BaseObserver | None = None
        self._watched: list[str] = []

    @property
    def index(self) -> VaultIndex:
        return self._index

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
            on_vault_change(path)
            return
        if event.event_type == EVENT_TYPE_MOVED and isinstance(event, FileSystemMovedEvent):
            src = Path(os.fsdecode(event.src_path))
            self._index.evict(src)
            final = self._process(Path(os.fsdecode(event.dest_path)))
            on_vault_change(final)
            return
        final = self._process(Path(os.fsdecode(event.src_path)))
        on_vault_change(final)

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


# --------------------------------------------------------------------------- #
# Daemon-down fallback for activity.recent                                     #
# --------------------------------------------------------------------------- #


def scan_recent(config: Config, limit: int = DEFAULT_RECENT_LIMIT) -> list[dict[str, Any]]:
    """Mtime-sorted dir scan of ``notes/``/``tasks/`` (brain-id files only).

    The daemon-down fallback for ``activity.recent``: same JSON-serializable row
    shape and ordering (mtime-descending, id-ascending on ties) as
    :meth:`VaultIndex.recent`, computed by a fresh on-disk scan.
    """
    rows: list[tuple[float, str, dict[str, Any], Path]] = []
    for path in _iter_vault_md(config.core.tolaria_path):
        try:
            text = path.read_text(encoding="utf-8")
        except OSError:
            continue
        try:
            meta = dict(frontmatter.loads(text).metadata)
        except yaml.YAMLError:
            continue
        entry_id = meta.get("id")
        if not _is_brain_id(entry_id):
            continue
        assert isinstance(entry_id, str)
        try:
            mtime = path.stat().st_mtime
        except OSError:
            continue
        rows.append((mtime, entry_id, meta, path))
    rows.sort(key=lambda r: r[1])  # id asc
    rows.sort(key=lambda r: r[0], reverse=True)  # mtime desc, stable
    if limit >= 0:
        rows = rows[:limit]
    return [_entry_dict(path, meta, mtime) for (mtime, _id, meta, path) in rows]


__all__ = [
    "DEFAULT_RECENT_LIMIT",
    "IndexEntry",
    "VaultEventHandler",
    "VaultIndex",
    "Watcher",
    "clear_change_hooks",
    "on_vault_change",
    "reconcile_path",
    "register_change_hook",
    "scan_recent",
]
