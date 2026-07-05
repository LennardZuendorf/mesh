"""Wikilink resolution: ``[[Title]]`` / ``[[n-id]]`` / ``[[t-id]]`` → ``related``.

A note body may reference other entities with ``[[…]]`` links. Resolution turns
those links into the note's ``related`` id list without ever touching the body:
:func:`resolve_wikilinks` returns the body **verbatim** alongside the resolved
ids, so it is a pure derivation and running it twice is a no-op.

Two link shapes:

* **Id form** — ``[[n-id]]`` / ``[[t-id]]`` pass through directly; the id is
  taken as-is, with no file lookup (an id-only body never reads disk).
* **Title form** — ``[[Some Title]]`` is resolved by an on-disk scan of
  ``notes/`` for a brain note whose ``title`` matches exactly (after stripping
  surrounding whitespace). Only brain-owned notes (id ``n-…``) are indexed,
  mirroring ``list_notes`` — a coexisting Tolaria/foreign file never shadows a
  link nor leaks a foreign id into ``related``.

The match is an **exact** title comparison, deliberately unlike the
slug-normalized matching that ``core.notes`` uses for CLI ``<id|slug>`` args:
wikilinks name a note's title; slugs are user shorthand.

An unresolvable title is left verbatim in the body and reported by
:func:`find_dangling` for ``brain status``. Resolution needs no daemon — it is a
direct filesystem read. A note whose body links its *own* title resolves to
itself (it is already on disk); that is allowed and harmless.
"""

from __future__ import annotations

import re
from collections.abc import Iterator
from pathlib import Path

from brain.storage.files import read_post

# A ``[[…]]`` link: capture the inner text, excluding brackets and newlines.
_WIKILINK = re.compile(r"\[\[([^\[\]\n]+?)\]\]")
# Id-form link: a brain id prefix (``n-``/``t-``) followed by id characters.
_ID_FORM = re.compile(r"^[nt]-[0-9A-Za-z]+$")
_BRAIN_ID_PREFIX = "n-"


def _notes_root(vault_path: Path) -> Path:
    return vault_path / "notes"


def _iter_note_files(vault_path: Path) -> Iterator[Path]:
    """Yield every ``*.md`` under ``notes/`` (``.locks/`` holds no ``.md``)."""
    root = _notes_root(vault_path)
    if not root.is_dir():
        return
    yield from sorted(root.rglob("*.md"))


def _link_targets(body: str) -> list[str]:
    """Return the stripped inner text of every ``[[…]]`` link, in body order."""
    return [match.group(1).strip() for match in _WIKILINK.finditer(body)]


def _title_index(vault_path: Path) -> dict[str, str]:
    """Map exact ``title`` → brain ``id`` for every brain note under ``notes/``.

    Only files whose ``id`` starts with ``n-`` are indexed (foreign/Tolaria files
    are skipped, exactly as ``list_notes`` surfaces only brain notes). On a
    duplicate title the first file in sorted order wins, keeping resolution
    deterministic.
    """
    index: dict[str, str] = {}
    for path in _iter_note_files(vault_path):
        post = read_post(path)
        if post is None:
            continue
        meta = post.metadata
        note_id = meta.get("id")
        title = meta.get("title")
        if (
            isinstance(note_id, str)
            and note_id.startswith(_BRAIN_ID_PREFIX)
            and isinstance(title, str)
            and title not in index
        ):
            index[title] = note_id
    return index


def resolve_wikilinks(body: str, vault_path: Path) -> tuple[str, list[str]]:
    """Resolve ``[[…]]`` links in ``body`` to a ``related`` id list.

    Returns ``(body, related)`` where ``body`` is returned unchanged (links are
    never rewritten) and ``related`` holds the resolved ids in first-seen order
    with duplicates dropped. Id-form links (``[[n-id]]`` / ``[[t-id]]``) resolve
    directly; title links resolve via an on-disk lookup of brain notes; a title
    with no match is skipped (see :func:`find_dangling`). The title index is
    built lazily, so a body of only id-form links reads no files.
    """
    index: dict[str, str] | None = None
    related: list[str] = []
    for target in _link_targets(body):
        if _ID_FORM.match(target):
            resolved = target
        else:
            if index is None:
                index = _title_index(vault_path)
            found = index.get(target)
            if found is None:
                continue  # dangling: leave verbatim, omit from related
            resolved = found
        if resolved not in related:
            related.append(resolved)
    return body, related


def find_dangling(vault_path: Path) -> list[str]:
    """Return the unresolvable ``[[Title]]`` link texts across all brain notes.

    Scans every brain note's body for title-form links whose title matches no
    brain note, de-duplicated in first-seen order (sorted file scan, body order
    within a file). Id-form links are never dangling. Consumed by ``brain
    status`` to surface broken references.
    """
    index = _title_index(vault_path)
    dangling: list[str] = []
    for path in _iter_note_files(vault_path):
        post = read_post(path)
        if post is None:
            continue
        note_id = post.metadata.get("id")
        if not (isinstance(note_id, str) and note_id.startswith(_BRAIN_ID_PREFIX)):
            continue
        for target in _link_targets(post.content):
            if _ID_FORM.match(target):
                continue
            if target not in index and target not in dangling:
                dangling.append(target)
    return dangling
