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
``tasks/`` — coexisting foreign files (any writer sharing the folder) included,
surfaced with ``id: None``. The index therefore holds both — id-bearing entities
(:meth:`~VaultIndex.entries`, and everything ``recent`` / ``get`` / ``len`` speak
for) and id-less foreign files — in one dict keyed by real path: one row per
file, never one row per id. Without the foreign rows a warm ``search.tag_pull``
would silently drop foreign hits the disk path returns — a result-contract
change, not an acceleration. Bodies stay off the index either way: no
list-shaped read needs them, and holding them would trade the daemon's whole
memory budget for nothing.

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
from typing import Any, TypeGuard

from shards.schemas.config import Config
from shards.storage.files import iter_md, read_post

DEFAULT_RECENT_LIMIT = 20
_ID_PREFIXES = ("n-", "t-")


def _is_shards_id(value: object) -> TypeGuard[str]:
    """Whether ``value`` is a shards id (``n-``/``t-``) — narrowing, so callers can
    keep the value without a follow-up ``assert isinstance``."""
    return isinstance(value, str) and value.startswith(_ID_PREFIXES)


def iter_vault_md(vault: Path) -> Iterator[Path]:
    """Yield every ``*.md`` under ``notes/`` and ``tasks/`` (full recursive walk).

    Recursive so typed note subfolders (``notes/decisions/`` …) and both task
    folders (``tasks/open/``, ``tasks/done/``) are covered. ``.locks/`` holds no
    ``.md`` and is naturally excluded. This is the corpus
    :func:`shards.index.tagpull.iter_corpus` walks, shared verbatim so the warm
    index and the on-disk scanners can never disagree about *which* files exist.
    Delegates to the one shared vault-walk primitive
    (:func:`shards.storage.files.iter_md`).
    """
    for sub in ("notes", "tasks"):
        yield from iter_md(vault / sub)


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
    """Thread-safe in-process frontmatter index over the vault corpus, keyed by path.

    **One row per file, keyed by real path — never by shards id.** The id is a
    *field* on the row, not its identity, because the folder cannot promise ids are
    unique: a ``cp``, a git conflicted copy or a sync client's "(conflicted copy)"
    puts two files carrying one id on disk, and the on-disk walks return both. An
    id-keyed index kept only the last-parsed of such a pair — a row silently
    missing from all four wired reads and an under-counted ``status`` — and could
    orphan a row outright when a file's id changed in place: the old id stayed in
    the index with no path left pointing at it, so ``note list`` advertised an id
    ``note get`` could never resolve, for the daemon's whole lifetime. Path keying
    removes both classes by construction: ``reparse`` replaces exactly the row for
    the path it read, ``evict`` drops exactly the row for the path that vanished.

    Shards entities (``n-``/``t-``) and coexisting foreign Markdown share the one
    dict and are told apart by :attr:`IndexEntry.id` being ``None`` (the module
    docstring covers why the corpus holds both). ``len()``, :meth:`entries` and
    :meth:`recent` speak for the shards rows only; :meth:`corpus` returns every row.
    """

    def __init__(self) -> None:
        self._rows: dict[str, IndexEntry] = {}  # realpath -> row (shards *and* foreign)
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

        A file that gains, loses or *changes* its shards id needs no bookkeeping:
        the row at this path is replaced wholesale, id field included.
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
        raw_id = meta.get("id")
        entry_id: str | None = raw_id if _is_shards_id(raw_id) else None
        entry = IndexEntry(id=entry_id, path=p, mtime=mtime, meta=meta)
        with self._lock:
            self._rows[self._rp(p)] = entry

    def evict(self, path: Path) -> None:
        """Drop the row whose path matches ``path`` — silent if none does."""
        rp = self._rp(Path(path))
        with self._lock:
            self._rows.pop(rp, None)

    def get(self, entry_id: str) -> IndexEntry | None:
        """The row carrying ``entry_id``; the lowest path when the vault holds duplicates.

        Path keying makes this a scan rather than a dict hit — deliberately. No
        wired read resolves an id (a point read already knows its file), so this is
        a convenience for a caller holding an id, with a stated tie-break instead of
        a silent last-writer-wins.
        """
        with self._lock:
            matches = [row for row in self._rows.values() if row.id == entry_id]
        return min(matches, key=lambda row: str(row.path)) if matches else None

    def entries(self) -> list[IndexEntry]:
        """Every indexed *shards* entity (id-bearing), as a snapshot list.

        The row source behind the warm ``note.list`` / ``task.list`` /
        ``vault.status`` handlers — one row per file, exactly like the walks.
        Order is unspecified: the shared selectors impose a deterministic path
        order before sorting, so the warm and on-disk row orders can never diverge.
        """
        with self._lock:
            return [row for row in self._rows.values() if row.id is not None]

    def corpus(self) -> list[IndexEntry]:
        """Every indexed vault file — shards entities *and* foreign ones.

        The row source behind the warm ``search.tag_pull`` handler, whose on-disk
        twin walks the same wider corpus (foreign files surface with ``id: None``).
        """
        with self._lock:
            return list(self._rows.values())

    def recent(self, limit: int = DEFAULT_RECENT_LIMIT) -> list[dict[str, Any]]:
        """Most-recently-modified shards rows, mtime-descending (id-ascending on ties)."""
        entries = self.entries()
        entries.sort(key=lambda e: e.id or "")  # stable secondary key
        entries.sort(key=lambda e: e.mtime, reverse=True)
        if limit >= 0:
            entries = entries[:limit]
        return [_entry_dict(e.path, e.meta, e.mtime) for e in entries]

    def clear(self) -> None:
        """Flush the whole index (called on a clean daemon stop)."""
        with self._lock:
            self._rows.clear()

    def __len__(self) -> int:
        """The number of indexed *shards* entities (foreign corpus rows excluded)."""
        return len(self.entries())


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
    for path in iter_vault_md(config.core.vault_path):
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
