"""Folder reconcile — the load-bearing folder healer.

A file whose frontmatter ``status``/``type`` maps (per :mod:`mesh.storage.files`)
to a different subdirectory than it currently lives in is relocated with a
byte-preserving :func:`os.replace` — *no* frontmatter reserialization, so the
``updated`` field is **not** bumped and unknown keys round-trip untouched. That
byte-identity is exactly what distinguishes a watcher move from a user edit (which
*does* bump ``updated`` via ``core``).

The rename is wrapped in a ``try/except OSError`` guard on purpose: this runs on
the watchdog observer thread (via :mod:`mesh.index.watcher`), and an escaping
exception on a raced source-vanish would kill that thread and freeze freshness for
the daemon's whole lifetime (see ``lessons.md`` § "An exception in a watcher thread
silently kills freshness"). Never remove that guard.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

from mesh.index.warm import _is_mesh_id
from mesh.schemas.config import Config
from mesh.storage.files import note_folder, read_post, task_folder
from mesh.storage.locks import LockError, acquire
from mesh.storage.sandbox import safe_resolve

__all__ = ["reconcile_path"]


def _correct_folder(config: Config, meta: dict[str, Any]) -> Path | None:
    """The folder ``meta`` *should* live in, or ``None`` if its type/status is unknown."""
    vault = config.core.vault_path
    try:
        if meta.get("type") == "task":
            return task_folder(str(meta.get("status")), vault)
        return note_folder(str(meta.get("type", "note")), vault)
    except ValueError:
        return None


def _entity_lock_path(src: Path, entity_id: str) -> Path:
    """The lock file ``core`` uses for this entity, derived from its live folder.

    ``core.notes``/``core.tasks`` lock at ``<notes|tasks>/.locks/<id>.lock``; the
    watcher must name the *same* file or the lock protects nothing. ``src`` is the
    file's current location, so its ``notes/``-or-``tasks/`` root is the one the
    writer is locking under.
    """
    root = src.parent
    while root.name not in ("notes", "tasks") and root.parent != root:
        root = root.parent
    return root / ".locks" / f"{entity_id}.lock"


def reconcile_path(config: Config, path: Path) -> Path:
    """Move ``path`` into the folder its frontmatter dictates; return the final path.

    A file whose ``status``/``type`` maps to a different subdirectory is relocated
    with a byte-preserving :func:`os.replace` — no frontmatter is reserialized, so
    ``updated`` is left untouched and unknown keys round-trip. Correctly placed,
    foreign (no mesh id), malformed, or unknown-type files are left in place and
    the **caller's own path is returned unchanged** — never a realpath.

    That last clause is load-bearing. The warm index is populated by a walk of the
    configured vault (:meth:`mesh.index.watcher.Watcher.warm`) and every scope
    predicate (:func:`mesh.core.notes.in_note_scope`,
    :func:`mesh.core.tasks.in_task_scope`) compares against that same configured
    vault, so a row must stay in the path space it was walked in. Returning
    ``safe_resolve``'s canonical path on the *no-move* branch silently moved every
    edited file into a second path space: with the vault reached through a symlink
    the row then matched no scope, and ``note list`` / ``task list`` went empty one
    edit after daemon start while the on-disk path stayed correct. Only a real move
    changes the answer, and only there is the sandbox-checked destination returned.
    """
    p = Path(path)
    if p.suffix != ".md":
        return p
    post = read_post(p)
    if post is None:
        return p
    meta = post.metadata
    if not _is_mesh_id(meta.get("id")):
        return p
    folder = _correct_folder(config, meta)
    if folder is None:
        return p
    vault = config.core.vault_path
    try:
        src = safe_resolve(vault, p)
        dest = safe_resolve(vault, folder / p.name)
    except ValueError:
        return p
    if src == dest:
        return p  # nothing moved — hand back the caller's path space, not a realpath
    entity_id = str(meta.get("id"))
    try:
        dest.parent.mkdir(parents=True, exist_ok=True)
        # Take the same per-entity lock every writer in ``core`` holds. Without it
        # the watcher can move a file out from under a locked writer *between* that
        # writer's resolve and its write, and the write then recreates the old path
        # — two files carrying one id, which is exactly the corpus state the warm
        # index cannot represent. Non-blocking on purpose: reconcile is opportunistic
        # folder healing, so a contended entity is simply left for the next event
        # rather than stalling the observer behind another process's edit.
        with acquire(_entity_lock_path(src, entity_id)):
            if not src.exists():
                return p  # the writer finished and moved it already
            os.replace(src, dest)  # atomic rename; content (and `updated`) preserved
    except LockError:
        return p  # a writer holds this entity — heal it on a later event
    except OSError:
        # The source raced away (concurrent delete/move) between the checks above
        # and the rename. Swallow it and leave the file where it is — a later event
        # will reconcile. Letting it escape would kill the watchdog observer thread
        # and freeze freshness for the daemon's whole lifetime. Nothing moved, so
        # the caller's path is returned here too.
        return p
    return dest
