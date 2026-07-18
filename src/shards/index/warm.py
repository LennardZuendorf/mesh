"""Warm frontmatter index + the daemon-down activity scan.

The daemon's *freshness* engine keeps a hot, in-process view of the one Markdown
folder so read-lenses do not re-parse from disk while the daemon is up. This
module owns that view and its daemon-down equivalent — no ``watchdog`` import, so
it stays cheap on the CLI path:

* **:class:`VaultIndex`** — an in-process frontmatter index keyed by shards id
  (``n-``/``t-``). ``reparse(path)`` (re)reads one file, ``evict(path)`` drops a
  (possibly already-deleted) path without error, and ``recent(limit)`` returns the
  most-recently-modified rows for ``activity.recent``. It is guarded by a lock
  because the watchdog observer thread writes it while the asyncio server thread
  reads it.

* **:func:`scan_recent`** — the daemon-down fallback for ``activity.recent``: an
  mtime-sorted dir scan of ``notes/`` and ``tasks/`` (shards-id files only),
  returning the same JSON-serializable row shape and ordering as
  :meth:`VaultIndex.recent`.

The watcher that drives ``reparse``/``evict`` on filesystem events lives in
:mod:`shards.index.watcher`; folder reconcile lives in
:mod:`shards.index.reconcile`. Both build on the small shared helpers below.
"""

from __future__ import annotations

import os
import threading
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import frontmatter
import yaml

from shards.schemas.config import Config
from shards.storage.files import read_post

DEFAULT_RECENT_LIMIT = 20
_ID_PREFIXES = ("n-", "t-")


def _is_shards_id(value: object) -> bool:
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
    """One indexed note/task: shards id, on-disk path, mtime, and frontmatter."""

    id: str
    path: Path
    mtime: float
    meta: dict[str, Any]


class VaultIndex:
    """Thread-safe in-process frontmatter index keyed by shards id."""

    def __init__(self) -> None:
        self._entries: dict[str, IndexEntry] = {}
        self._by_path: dict[str, str] = {}  # realpath -> id, for path-based eviction
        self._lock = threading.RLock()

    @staticmethod
    def _rp(path: Path) -> str:
        return os.path.realpath(path)

    def reparse(self, path: Path) -> None:
        """(Re)index ``path``; a vanished or non-shards file is a no-op / eviction.

        Only ``*.md`` files whose frontmatter carries a shards id are indexed;
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
        if not _is_shards_id(entry_id):
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
# Daemon-down fallback for activity.recent                                     #
# --------------------------------------------------------------------------- #


def scan_recent(config: Config, limit: int = DEFAULT_RECENT_LIMIT) -> list[dict[str, Any]]:
    """Mtime-sorted dir scan of ``notes/``/``tasks/`` (shards-id files only).

    The daemon-down fallback for ``activity.recent``: same JSON-serializable row
    shape and ordering (mtime-descending, id-ascending on ties) as
    :meth:`VaultIndex.recent`, computed by a fresh on-disk scan.
    """
    rows: list[tuple[float, str, dict[str, Any], Path]] = []
    for path in _iter_vault_md(config.core.tolaria_path):
        post = read_post(path)
        if post is None:
            continue
        meta = dict(post.metadata)
        entry_id = meta.get("id")
        if not _is_shards_id(entry_id):
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
    "VaultIndex",
    "scan_recent",
]
