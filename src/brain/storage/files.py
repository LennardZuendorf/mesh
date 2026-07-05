"""Atomic file writes and note folder routing.

Every vault write goes through :func:`atomic_write`: content is written to a
sibling temp file (same directory, so ``os.replace`` is a same-filesystem atomic
rename) and only then swapped onto the target. If anything fails before the
rename, the destination keeps its previous contents and the temp file is
removed — the destination is never observed half-written.
"""

from __future__ import annotations

import contextlib
import os
import tempfile
from pathlib import Path

import frontmatter
import yaml

# type -> path relative to the vault root.
_NOTE_SUBDIRS: dict[str, tuple[str, ...]] = {
    "note": ("notes",),
    "log": ("notes", "logs"),
    "decision": ("notes", "decisions"),
    "reference": ("notes", "references"),
}

# task status -> path relative to the vault root. Live work (open/claimed) sits
# in ``tasks/open/``; terminal work (done/cancelled) moves to ``tasks/done/``.
_TASK_SUBDIRS: dict[str, tuple[str, ...]] = {
    "open": ("tasks", "open"),
    "claimed": ("tasks", "open"),
    "done": ("tasks", "done"),
    "cancelled": ("tasks", "done"),
}


def atomic_write(path: Path, content: str) -> None:
    """Write ``content`` to ``path`` atomically (temp file + ``os.replace``)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(
        dir=path.parent,
        prefix=f".{path.name}.",
        suffix=".tmp",
    )
    tmp_path = Path(tmp_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(content)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp_path, path)
        _fsync_dir(path.parent)
    except BaseException:
        with contextlib.suppress(FileNotFoundError):
            tmp_path.unlink()
        raise


def _fsync_dir(directory: Path) -> None:
    """Best-effort fsync of ``directory`` so a rename survives power loss.

    The rename itself is atomic; this makes the *directory entry* durable. Guarded
    because some filesystems reject an ``fsync`` on a directory fd — durability is
    a best effort, never a correctness dependency."""
    with contextlib.suppress(OSError):
        dir_fd = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)


def read_post(path: Path) -> frontmatter.Post | None:
    """Read and parse a Markdown file's frontmatter; ``None`` if unreadable.

    A vanished/unreadable file (``OSError``) or a malformed YAML frontmatter
    block (``yaml.YAMLError``) yields ``None`` — foreign and corrupt files must
    skip silently, never crash a scan or a lookup. The single safe reader every
    scanner and lookup routes through (``.metadata`` / ``.content`` off the
    returned :class:`frontmatter.Post`)."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return None
    try:
        return frontmatter.loads(text)
    except yaml.YAMLError:
        return None


def note_folder(note_type: str, tolaria_path: Path) -> Path:
    """Return the vault folder for ``note_type`` (raises ``ValueError`` if unknown)."""
    try:
        parts = _NOTE_SUBDIRS[note_type]
    except KeyError:
        raise ValueError(f"unknown note type: {note_type!r}") from None
    return tolaria_path.joinpath(*parts)


def task_folder(status: str, tolaria_path: Path) -> Path:
    """Return the vault folder for a task ``status`` (raises ``ValueError`` if unknown).

    ``open``/``claimed`` route to ``tasks/open/``; ``done``/``cancelled`` route to
    ``tasks/done/``.
    """
    try:
        parts = _TASK_SUBDIRS[status]
    except KeyError:
        raise ValueError(f"unknown task status: {status!r}") from None
    return tolaria_path.joinpath(*parts)
