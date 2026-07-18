"""Folder reconcile — the load-bearing folder healer.

A file whose frontmatter ``status``/``type`` maps (per :mod:`shards.storage.files`)
to a different subdirectory than it currently lives in is relocated with a
byte-preserving :func:`os.replace` — *no* frontmatter reserialization, so the
``updated`` field is **not** bumped and unknown keys round-trip untouched. That
byte-identity is exactly what distinguishes a watcher move from a user edit (which
*does* bump ``updated`` via ``core``).

The rename is wrapped in a ``try/except OSError`` guard on purpose: this runs on
the watchdog observer thread (via :mod:`shards.index.watcher`), and an escaping
exception on a raced source-vanish would kill that thread and freeze freshness for
the daemon's whole lifetime (see ``lessons.md`` § "An exception in a watcher thread
silently kills freshness"). Never remove that guard.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import frontmatter
import yaml

from shards.index.warm import _is_shards_id
from shards.schemas.config import Config
from shards.storage.files import note_folder, task_folder
from shards.storage.sandbox import safe_resolve

__all__ = ["reconcile_path"]


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
    foreign (no shards id), malformed, or unknown-type files are left in place and
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
    if not _is_shards_id(meta.get("id")):
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
