"""Wikilink resolution: ``[[Title]]`` / ``[[n-id]]`` / ``[[t-id]]`` → ``related``.

A note body may reference other entities with ``[[…]]`` links. Resolution turns
those links into the note's ``related`` id list without ever touching the body:
:func:`resolve_wikilinks` returns the body **verbatim** alongside the resolved
ids, so it is a pure derivation and running it twice is a no-op.

Two link shapes:

* **Id form** — ``[[n-id]]`` / ``[[t-id]]`` pass through directly; the id is
  taken as-is, with no file lookup (an id-only body never reads disk).
* **Title form** — ``[[Some Title]]`` is resolved by an on-disk scan of
  ``notes/`` for a shards note whose ``title`` matches exactly (after stripping
  surrounding whitespace). Only shards-owned notes (id ``n-…``) are indexed,
  mirroring ``list_notes`` — a coexisting foreign file (any writer sharing the
  folder) never shadows a link nor leaks a foreign id into ``related``.

The match is an **exact** title comparison, deliberately unlike the
slug-normalized matching that ``core.notes`` uses for CLI ``<id|slug>`` args:
wikilinks name a note's title; slugs are user shorthand.

An unresolvable title is left verbatim in the body and reported by
:func:`find_dangling` for ``shards status``. Resolution needs no daemon — it is a
direct filesystem read. A note whose body links its *own* title resolves to
itself (it is already on disk); that is allowed and harmless.
"""

from __future__ import annotations

import itertools
import re
from collections.abc import Iterator
from pathlib import Path

from shards.storage.files import iter_md, read_post

# A ``[[…]]`` link: capture the inner text, excluding brackets and newlines.
_WIKILINK = re.compile(r"\[\[([^\[\]\n]+?)\]\]")
# Id-form link: a shards id prefix (``n-``/``t-``) followed by id characters.
_ID_FORM = re.compile(r"^[nt]-[0-9A-Za-z]+$")
_SHARDS_ID_PREFIX = "n-"
_SHARDS_ID_PREFIXES = ("n-", "t-")
# The task lifecycle folders, walked non-recursively — the same two folders and
# the same shape as ``core.tasks._TASK_SUBDIRS`` / ``core.tasks.in_task_scope``.
# Spelled here rather than imported because ``core.tasks`` imports *this* module
# (:func:`resolve_wikilinks` runs on every task body write); the two must be kept
# in step, and ``tests/notes/test_wikilinks.py`` pins that they select the same
# files.
_TASK_SUBDIRS: tuple[str, ...] = ("open", "done")


def _notes_root(vault_path: Path) -> Path:
    return vault_path / "notes"


def _tasks_root(vault_path: Path) -> Path:
    return vault_path / "tasks"


def _iter_note_files(vault_path: Path) -> Iterator[Path]:
    """Yield every ``*.md`` under ``notes/``, sorted.

    A thin, deterministically-ordered call onto the one shared vault walk
    (:func:`shards.storage.files.iter_md`).
    """
    yield from sorted(iter_md(_notes_root(vault_path)))


def _iter_task_files(vault_path: Path) -> Iterator[Path]:
    """Yield every ``*.md`` directly under ``tasks/open/`` then ``tasks/done/``, sorted.

    Non-recursive over exactly those two folders, mirroring
    :func:`shards.core.tasks._iter_task_files` and
    :func:`shards.core.tasks.in_task_scope`: those folders *are* the task
    lifecycle, so a file filed beside them (``tasks/archive/t-z.md``) or a level
    deeper (``tasks/open/sub/t-z.md``) is not a task any verb can resolve, get,
    claim or edit. Walking it anyway would let ``shards status`` count dangling
    links from files the tool offers no way to fix — a health signal naming a
    problem outside its own scope. Sorted per folder, so the scan order is
    deterministic and ``open`` precedes ``done`` exactly as in ``core.tasks``.
    """
    root = _tasks_root(vault_path)
    for sub in _TASK_SUBDIRS:
        yield from sorted(iter_md(root / sub, recursive=False))


def _link_targets(body: str) -> list[str]:
    """Return the stripped inner text of every ``[[…]]`` link, in body order."""
    return [match.group(1).strip() for match in _WIKILINK.finditer(body)]


def _title_index(vault_path: Path) -> dict[str, str]:
    """Map exact ``title`` → shards ``id`` for every shards note under ``notes/``.

    Only files whose ``id`` starts with ``n-`` are indexed (foreign files — any
    writer sharing the folder — are skipped, exactly as ``list_notes`` surfaces
    only shards notes). On a
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
            and note_id.startswith(_SHARDS_ID_PREFIX)
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
    directly; title links resolve via an on-disk lookup of shards notes; a title
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
    """Return the unresolvable ``[[Title]]`` link texts across the whole vault.

    Scans every shards note **and task** body (``notes/**`` recursively, plus
    ``tasks/open/`` and ``tasks/done/`` non-recursively — exactly the folder scope
    ``core.tasks`` resolves through, so every file counted here is one a verb can
    actually reach) for title-form links whose title matches no shards note,
    de-duplicated in first-seen order (sorted file scan per root, notes before
    tasks, body order within a file). Id-form links are never dangling. The title *index* itself
    stays notes-only — titles resolve to notes by contract (see module docstring)
    — only the scan for link *targets* widens to cover tasks. Consumed by
    ``shards status`` to surface broken references across the whole corpus (root
    tech.md § B6).
    """
    index = _title_index(vault_path)
    dangling: list[str] = []
    for path in itertools.chain(_iter_note_files(vault_path), _iter_task_files(vault_path)):
        post = read_post(path)
        if post is None:
            continue
        note_id = post.metadata.get("id")
        if not (isinstance(note_id, str) and note_id.startswith(_SHARDS_ID_PREFIXES)):
            continue
        for target in _link_targets(post.content):
            if _ID_FORM.match(target):
                continue
            if target not in index and target not in dangling:
                dangling.append(target)
    return dangling
