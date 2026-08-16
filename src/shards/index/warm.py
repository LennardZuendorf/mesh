"""Warm frontmatter index + the daemon-down activity scan.

The daemon's *freshness* engine keeps a hot, in-process view of the one Markdown
folder so read-lenses do not re-parse from disk while the daemon is up. This
module owns that view and its daemon-down equivalent — no ``watchdog`` import, so
it stays cheap on the CLI path:

* **:class:`VaultIndex`** — an in-process frontmatter index of the vault corpus.
  ``reparse(path)`` (re)reads one file, ``evict(path)`` drops a (possibly
  already-deleted) path without error, ``recent(limit)`` returns the
  most-recently-modified *shards* rows for ``activity.recent``, and
  :meth:`~VaultIndex.entries` / :meth:`~VaultIndex.corpus` are the projections the
  daemon's warm read handlers filter over. It is guarded by a lock because the
  watchdog observer thread writes it while the asyncio server thread reads it.

* **:func:`scan_recent`** — the daemon-down fallback for ``activity.recent``: an
  mtime-sorted dir scan of ``notes/`` and ``tasks/`` (shards-id files only),
  returning the same JSON-serializable row shape and ordering as
  :meth:`VaultIndex.recent`.

**Shards rows vs. the corpus.** ``note list`` / ``task list`` only ever surface
files carrying a shards id (``n-``/``t-``), but ``search`` (tag-pull and the
substring fallback) deliberately covers *every* ``*.md`` under ``notes/`` and
``tasks/`` — coexisting Tolaria/foreign files included, surfaced with ``id:
None``. The index therefore holds both, split into two buckets: id-bearing
entities keyed by shards id (:meth:`~VaultIndex.entries`, and everything
``recent`` / ``get`` / ``len`` speak for) and id-less corpus files keyed by real
path. Without the second bucket a warm ``search.tag_pull`` would silently drop
foreign hits the disk path returns — a result-contract change, not an
acceleration. Bodies stay off the index in both buckets: no list-shaped read
needs them, and holding them would trade the daemon's whole memory budget for
nothing.

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

from shards.schemas.config import Config
from shards.storage.files import read_post

DEFAULT_RECENT_LIMIT = 20
_ID_PREFIXES = ("n-", "t-")


def _is_shards_id(value: object) -> bool:
    return isinstance(value, str) and value.startswith(_ID_PREFIXES)


def iter_vault_md(vault: Path) -> Iterator[Path]:
    """Yield every ``*.md`` under ``notes/`` and ``tasks/`` (full recursive walk).

    Recursive so typed note subfolders (``notes/decisions/`` …) and both task
    folders (``tasks/open/``, ``tasks/done/``) are covered. ``.locks/`` holds no
    ``.md`` and is naturally excluded. This is the corpus
    :func:`shards.index.tagpull.iter_corpus` walks, shared verbatim so the warm
    index and the on-disk scanners can never disagree about *which* files exist.
    """
    for sub in ("notes", "tasks"):
        root = vault / sub
        if root.is_dir():
            yield from root.rglob("*.md")


def _entry_dict(path: Path, meta: dict[str, Any], mtime: float) -> dict[str, Any]:
    """A JSON-serializable ``activity.recent`` row (team-awareness/6).

    Deliberately excludes ``created``/``updated`` (which ``frontmatter`` parses
    back into ``datetime`` objects) so the payload survives ``json.dumps`` over
    the socket unmodified. ``owner``/``claimed_by`` are read straight off the
    ``meta`` dict already in hand from the parse — no extra disk read — so
    identity-aware filters (``--owner``/``--mine``) can read the row instead of
    re-opening the file. ``claimed_by`` is ``None`` for notes (only tasks carry
    the key); both are already JSON-safe (``str | None``).
    """
    return {
        "id": meta.get("id"),
        "type": meta.get("type"),
        "title": meta.get("title"),
        "path": str(path),
        "mtime": mtime,
        "owner": meta.get("owner"),
        "claimed_by": meta.get("claimed_by"),
    }


# --------------------------------------------------------------------------- #
# In-process frontmatter index                                                #
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class IndexEntry:
    """One indexed vault file: its shards id (``None`` if foreign), path, mtime, meta."""

    id: str | None
    path: Path
    mtime: float
    meta: dict[str, Any]


class VaultIndex:
    """Thread-safe in-process frontmatter index over the vault corpus.

    Shards entities are keyed by their shards id; coexisting foreign Markdown is
    kept in a second bucket keyed by real path (see the module docstring for why
    both are needed). ``len()``, :meth:`get` and :meth:`recent` speak only for the
    shards bucket, exactly as before.
    """

    def __init__(self) -> None:
        self._entries: dict[str, IndexEntry] = {}
        self._by_path: dict[str, str] = {}  # realpath -> id, for path-based eviction
        self._foreign: dict[str, IndexEntry] = {}  # realpath -> row, for id-less files
        self._lock = threading.RLock()

    @staticmethod
    def _rp(path: Path) -> str:
        return os.path.realpath(path)

    def reparse(self, path: Path) -> None:
        """(Re)index ``path``; a vanished file is evicted, an unreadable one skipped.

        Reads through :func:`shards.storage.files.read_post` — the project's one
        safe reader — which collapses "vanished" and "corrupt" into a single
        ``None``. The two must stay distinguishable here (a deleted file has to
        leave the index; a momentarily unreadable or malformed one must keep its
        last good row rather than disappear from ``activity.recent``), so the
        ``None`` branch re-checks the path: no longer a file → evict, still a file
        → skip. That preserves the original ``FileNotFoundError``/``IsADirectoryError``
        → evict, other ``OSError``/``yaml.YAMLError`` → skip behaviour at the cost
        of one extra ``stat`` on the failure path only. Never raises.
        """
        p = Path(path)
        if p.suffix != ".md":
            return
        post = read_post(p)
        if post is None:
            if not p.is_file():
                self.evict(p)  # gone (or replaced by a directory) — drop the row
            return  # unreadable or malformed — keep the last good row
        try:
            mtime = p.stat().st_mtime
        except OSError:
            return
        meta = dict(post.metadata)
        entry_id = meta.get("id")
        rp = self._rp(p)
        if not _is_shards_id(entry_id):
            entry = IndexEntry(id=None, path=p, mtime=mtime, meta=meta)
            with self._lock:
                prior_id = self._by_path.pop(rp, None)  # the file lost its shards id
                if prior_id is not None:
                    self._entries.pop(prior_id, None)
                self._foreign[rp] = entry
            return
        assert isinstance(entry_id, str)
        entry = IndexEntry(id=entry_id, path=p, mtime=mtime, meta=meta)
        with self._lock:
            self._foreign.pop(rp, None)  # the file gained a shards id
            prior = self._entries.get(entry_id)
            if prior is not None:
                self._by_path.pop(self._rp(prior.path), None)
            self._entries[entry_id] = entry
            self._by_path[rp] = entry_id

    def evict(self, path: Path) -> None:
        """Drop the row whose path matches ``path`` — silent if none does."""
        rp = self._rp(Path(path))
        with self._lock:
            self._foreign.pop(rp, None)
            entry_id = self._by_path.pop(rp, None)
            if entry_id is not None:
                self._entries.pop(entry_id, None)

    def get(self, entry_id: str) -> IndexEntry | None:
        with self._lock:
            return self._entries.get(entry_id)

    def entries(self) -> list[IndexEntry]:
        """Every indexed *shards* entity (id-bearing), as a snapshot list.

        The row source behind the warm ``note.list`` / ``task.list`` /
        ``vault.status`` handlers. Order is unspecified — the shared selectors
        impose a deterministic path order before sorting, so the warm and on-disk
        row orders can never diverge.
        """
        with self._lock:
            return list(self._entries.values())

    def corpus(self) -> list[IndexEntry]:
        """Every indexed vault file — shards entities *and* foreign ones.

        The row source behind the warm ``search.tag_pull`` handler, whose on-disk
        twin walks the same wider corpus (foreign files surface with ``id: None``).
        """
        with self._lock:
            return [*self._entries.values(), *self._foreign.values()]

    def recent(self, limit: int = DEFAULT_RECENT_LIMIT) -> list[dict[str, Any]]:
        """Most-recently-modified shards rows, mtime-descending (id-ascending on ties)."""
        with self._lock:
            entries = list(self._entries.values())
        entries.sort(key=lambda e: e.id or "")  # stable secondary key
        entries.sort(key=lambda e: e.mtime, reverse=True)
        if limit >= 0:
            entries = entries[:limit]
        return [_entry_dict(e.path, e.meta, e.mtime) for e in entries]

    def clear(self) -> None:
        """Flush the whole index (called on a clean daemon stop)."""
        with self._lock:
            self._entries.clear()
            self._by_path.clear()
            self._foreign.clear()

    def __len__(self) -> int:
        """The number of indexed *shards* entities (foreign corpus rows excluded)."""
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
    for path in iter_vault_md(config.core.tolaria_path):
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
    "iter_vault_md",
    "scan_recent",
]
