"""Note domain logic: locate, append to, and update note files.

This module owns the *amend* verbs — :func:`append_note` and :func:`update_note`
— plus the machinery they share: resolving an ``<id|slug>`` to a file, holding
the per-entity ``O_EXCL`` lock for the whole read-modify-write cycle, and writing
back atomically. Bodies are treated as inert Markdown: appends only add the
caller's text (optionally under a ``##`` section, optionally timestamped) and
never inject machinery. Unknown frontmatter keys survive untouched because we
mutate the parsed metadata dict in place rather than reserialising a model.

Every body write refreshes the derived ``related`` list via
:func:`shards.core.wikilinks.resolve_wikilinks` (an on-disk scan, daemon-free):
``related`` is a pure function of the body, recomputed and overwritten on each
append/update.
"""

from __future__ import annotations

import os
import re
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import get_args

import frontmatter
from msgspec import ValidationError

from shards.core.ids import generate_note_id
from shards.core.wikilinks import resolve_wikilinks
from shards.schemas.config import Config
from shards.schemas.note import Note, NoteType
from shards.storage.files import atomic_write, note_folder, read_post
from shards.storage.locks import hold
from shards.storage.sandbox import safe_resolve

_NOTE_TYPES: tuple[str, ...] = get_args(NoteType)
_SORT_FIELDS: tuple[str, ...] = ("updated", "created", "title")
_ID_PREFIX = "n-"
_DEFAULT_LIMIT = 20
_SLUG_NON_ALNUM = re.compile(r"[^a-z0-9]+")
# A heading of level 1 or 2 (##, #) — a section boundary; ### and deeper nest.
_TOP_HEADING = re.compile(r"^#{1,2}(?!#)\s")
# ``--since`` duration shorthand: <int> days / hours / weeks.
_DURATION = re.compile(r"^(\d+)([dhw])$")
_DURATION_UNITS = {"d": "days", "h": "hours", "w": "weeks"}


class NoteError(Exception):
    """Base class for note-resolution failures."""


class NoteNotFoundError(NoteError):
    """No note matches the given id or slug (CLI exit 3)."""


class AmbiguousSlugError(NoteError):
    """A slug matches more than one note (CLI exit 2).

    Carries the matching ids so the CLI can list them in its exit-2 message.
    """

    def __init__(self, slug: str, ids: list[str] | None = None) -> None:
        self.slug = slug
        self.ids = list(ids or [])
        detail = f": {', '.join(self.ids)}" if self.ids else ""
        super().__init__(f"ambiguous slug {slug!r}{detail}")


@dataclass(frozen=True)
class NoteView:
    """A note read off disk: validated frontmatter plus the raw body.

    ``get_note`` / ``list_notes`` return these; the CLI renders them per the
    active output flags. ``body`` is inert Markdown, never interpreted.
    """

    note: Note
    body: str
    path: Path


def _now() -> datetime:
    return datetime.now(UTC)


def _slugify(text: str) -> str:
    return _SLUG_NON_ALNUM.sub("-", text.strip().lower()).strip("-")


def _notes_root(config: Config) -> Path:
    return config.core.tolaria_path / "notes"


def _iter_note_files(config: Config) -> Iterator[Path]:
    """Yield every ``*.md`` under ``notes/`` (``.locks/`` holds no ``.md``)."""
    root = _notes_root(config)
    if not root.is_dir():
        return
    yield from root.rglob("*.md")


def _resolve_path(config: Config, id_or_slug: str) -> Path:
    """Resolve ``<id|slug>`` to a *shards* note path, sandbox-checked.

    Only files whose stem carries a shards id (``n-`` prefix) are candidates, so a
    coexisting Tolaria/foreign ``.md`` is never resolved (and thus never read,
    amended, or deleted) — mirroring the id gate ``list_notes`` applies. Id match
    (filename stem) wins; otherwise a normalized-title slug match. A slug hitting
    multiple notes raises :class:`AmbiguousSlugError`; no match raises
    :class:`NoteNotFoundError`.
    """
    vault = config.core.tolaria_path
    shards_files = [p for p in _iter_note_files(config) if p.stem.startswith(_ID_PREFIX)]
    by_id = [p for p in shards_files if p.stem == id_or_slug]
    if by_id:
        return safe_resolve(vault, by_id[0])

    target = _slugify(id_or_slug)
    by_slug: list[Path] = []
    for path in shards_files:
        post = read_post(path)
        if post is None:
            continue
        title = post.metadata.get("title")
        if isinstance(title, str) and _slugify(title) == target:
            by_slug.append(path)
    if len(by_slug) == 1:
        return safe_resolve(vault, by_slug[0])
    if len(by_slug) > 1:
        raise AmbiguousSlugError(id_or_slug, sorted(p.stem for p in by_slug))
    raise NoteNotFoundError(id_or_slug)


def _lock_path(config: Config, note_id: str) -> Path:
    return _notes_root(config) / ".locks" / f"{note_id}.lock"


def _append_to_end(body: str, block: str) -> str:
    base = body.rstrip("\n")
    return f"{base}\n\n{block}" if base else block


def _append_under_section(body: str, block: str, section: str) -> str:
    """Append ``block`` under the ``## {section}`` heading, creating it if absent."""
    heading = f"## {section}"
    lines = body.split("\n")
    start = next((i for i, line in enumerate(lines) if line.strip() == heading), None)
    if start is None:
        return _append_to_end(body, f"{heading}\n\n{block}")

    end = len(lines)
    for j in range(start + 1, len(lines)):
        if _TOP_HEADING.match(lines[j]):
            end = j
            break
    head = "\n".join(lines[:end]).rstrip("\n")
    tail = "\n".join(lines[end:]).strip("\n")
    result = f"{head}\n\n{block}"
    return f"{result}\n\n{tail}" if tail else result


def _format_block(text: str, timestamp: bool) -> str:
    if timestamp:
        return f"{_now().strftime('%Y-%m-%dT%H:%M:%SZ')}\n{text}"
    return text


def _id_taken(config: Config, candidate: str) -> bool:
    """Whether a note file with stem ``candidate`` already exists (id collision)."""
    return any(path.stem == candidate for path in _iter_note_files(config))


def _validate_owner(config: Config, owner: str | None) -> None:
    """Reject an explicit ``owner`` outside a non-empty ``[tasks].collections``.

    Enforced here at the core write boundary so every caller — CLI, MCP, a future
    daemon write path — gets the identical rule (notes and tasks alike). A
    ``None`` owner (which defaults to the config agent) is exempt: ``collections``
    validates the supplied ``--owner`` argument, not the running identity. Raises
    ``ValueError`` (CLI exit 2), checked before any file is written.
    """
    collections = config.tasks.collections
    if owner is not None and collections and owner not in collections:
        raise ValueError(f"unknown owner: {owner!r}")


def create_note(
    config: Config,
    title: str,
    *,
    note_type: str = "note",
    tags: list[str] | None = None,
    owner: str | None = None,
    body: str = "",
) -> Note:
    """Create a new note and return its validated frontmatter (R1).

    Generates a deterministic hash ``n-`` id (extended on collision), routes the
    file into the folder matching ``note_type`` (``notes/`` for ``note``;
    ``notes/{logs,decisions,references}/`` for the typed variants), derives
    ``related`` from the body's wikilinks, and writes ``<id>.md`` atomically.
    ``created`` and ``updated`` are set to the same instant (birth). ``owner``
    defaults to the resolved config agent (``$SHARDS_AGENT`` override applied at
    load) when not given. Raises ``ValueError`` for an unknown ``note_type``.
    """
    if note_type not in _NOTE_TYPES:
        raise ValueError(f"invalid note type: {note_type!r}")
    _validate_owner(config, owner)

    vault = config.core.tolaria_path
    now = _now()
    note_id = generate_note_id(
        now.isoformat(), title, exists=lambda candidate: _id_taken(config, candidate)
    )
    _, related = resolve_wikilinks(body, vault)
    note = Note.model_validate(
        {
            "id": note_id,
            "type": note_type,
            "title": title,
            "tags": list(tags or []),
            "owner": owner if owner is not None else config.agent,
            "created": now,
            "updated": now,
            "related": related,
        }
    )

    path = safe_resolve(vault, note_folder(note_type, vault) / f"{note_id}.md")
    post = frontmatter.Post(body)
    # Serialize the frontmatter from the validated model — the schema is the one
    # on-disk contract, never a parallel hand-built dict that can drift from it.
    post.metadata = note.model_dump(mode="python")
    atomic_write(path, frontmatter.dumps(post))
    return note


def append_note(
    config: Config,
    id_or_slug: str,
    text: str,
    *,
    section: str | None = None,
    timestamp: bool = False,
) -> Note:
    """Append ``text`` to a note's body and bump ``updated``.

    With ``section`` the text lands under a ``## {section}`` heading (created at
    end-of-body if missing). With ``timestamp`` an ISO-8601 UTC line is prepended
    to the text block. The whole read-modify-write runs under the entity lock and
    the result is written atomically. The re-read inside the lock goes through
    :func:`shards.storage.files.read_post`; a file that vanishes or turns
    unreadable between resolution and the lock raises
    :class:`NoteNotFoundError`, matching :func:`get_note`.
    """
    path = _resolve_path(config, id_or_slug)
    note_id = path.stem
    block = _format_block(text, timestamp)
    with hold(_lock_path(config, note_id)):
        post = read_post(path)
        if post is None:
            raise NoteNotFoundError(id_or_slug)
        post.content = (
            _append_under_section(post.content, block, section)
            if section is not None
            else _append_to_end(post.content, block)
        )
        _, related = resolve_wikilinks(post.content, config.core.tolaria_path)
        post.metadata["related"] = related
        post.metadata["updated"] = _now()
        note = Note.model_validate(post.metadata)
        atomic_write(path, frontmatter.dumps(post))
    return note


def apply_tag_spec(existing: list[str], spec: str) -> list[str]:
    """Apply a ``--tags`` spec to ``existing``.

    ``+x,-y`` (every token prefixed) is a delta: add ``x``, remove ``y``. Any
    unprefixed token makes the whole spec a replacement of the tag list. Both
    forms dedupe while preserving order.
    """
    tokens = [t.strip() for t in spec.split(",") if t.strip()]
    is_delta = bool(tokens) and all(t[0] in "+-" for t in tokens)
    if is_delta:
        result = list(existing)
        for token in tokens:
            op, name = token[0], token[1:]
            if not name:
                continue
            if op == "+" and name not in result:
                result.append(name)
            elif op == "-" and name in result:
                result.remove(name)
        return result

    result = []
    for token in tokens:
        if token not in result:
            result.append(token)
    return result


def update_note(
    config: Config,
    id_or_slug: str,
    *,
    tags: str | None = None,
    new_type: str | None = None,
) -> Note:
    """Update a note's fields and bump ``updated``.

    ``tags`` mutates the tag list (delta or replace, see :func:`apply_tag_spec`).
    ``new_type`` rewrites the ``type`` field and moves the file into the matching
    folder via ``os.replace`` (atomic rename); the old path stops existing. Runs
    under the entity lock; writes are atomic. The re-read inside the lock goes
    through :func:`shards.storage.files.read_post`; a file that vanishes or turns
    unreadable between resolution and the lock raises :class:`NoteNotFoundError`,
    matching :func:`get_note`.
    """
    if new_type is not None and new_type not in _NOTE_TYPES:
        raise ValueError(f"invalid note type: {new_type!r}")

    vault = config.core.tolaria_path
    path = _resolve_path(config, id_or_slug)
    note_id = path.stem
    with hold(_lock_path(config, note_id)):
        post = read_post(path)
        if post is None:
            raise NoteNotFoundError(id_or_slug)
        if tags is not None:
            current = post.metadata.get("tags") or []
            existing = [str(t) for t in current] if isinstance(current, list) else []
            post.metadata["tags"] = apply_tag_spec(existing, tags)
        if new_type is not None:
            post.metadata["type"] = new_type
        _, related = resolve_wikilinks(post.content, config.core.tolaria_path)
        post.metadata["related"] = related
        post.metadata["updated"] = _now()
        note = Note.model_validate(post.metadata)

        atomic_write(path, frontmatter.dumps(post))
        if new_type is not None:
            dest = safe_resolve(vault, note_folder(new_type, vault) / path.name)
            if dest != path:
                dest.parent.mkdir(parents=True, exist_ok=True)
                os.replace(path, dest)
    return note


def delete_note(config: Config, id_or_slug: str) -> str:
    """Hard-delete a note under the entity lock; return the deleted id.

    Resolves ``<id|slug>`` to a sandbox-checked path (raising
    :class:`NoteNotFoundError` / :class:`AmbiguousSlugError` before touching the
    filesystem), then removes the file permanently — no archive, no trash —
    *inside* the per-entity ``O_EXCL`` lock. Holding the lock serializes the
    delete against a concurrent ``append``/``update`` so a racing writer can never
    resurrect the note or have its in-flight lock stolen. :func:`shards.storage.locks.hold`
    clears only *stale* locks (dead PID or aged out) on acquire and releases the
    lock on exit, so residue is cleaned without unconditionally destroying a live
    lock.
    """
    path = _resolve_path(config, id_or_slug)
    note_id = path.stem
    with hold(_lock_path(config, note_id)):
        path.unlink()
    return note_id


# --------------------------------------------------------------------------- #
# Read verbs — get / list (notes/4). Direct on-disk reads, daemon-independent. #
# --------------------------------------------------------------------------- #


def resolve_slug(config: Config, id_or_slug: str) -> str:
    """Resolve ``<id|slug>`` to a single note id.

    Thin public wrapper over the shared resolver: an id (filename stem) wins,
    else a normalized-title slug match. Raises :class:`NoteNotFoundError` on no
    match and :class:`AmbiguousSlugError` (carrying the matching ids) on a slug
    that hits more than one note.
    """
    return _resolve_path(config, id_or_slug).stem


def get_note(config: Config, id_or_slug: str) -> NoteView:
    """Read a single note by ``<id|slug>`` into a :class:`NoteView`.

    Resolution mirrors :func:`resolve_slug`; the frontmatter is validated into a
    :class:`Note` and the raw body is returned verbatim (the CLI truncates for
    previews). Not-found / ambiguous surface as the shared exceptions.
    """
    path = _resolve_path(config, id_or_slug)
    post = read_post(path)
    if post is None:
        raise NoteNotFoundError(id_or_slug)
    note = Note.model_validate(post.metadata)
    return NoteView(note=note, body=post.content, path=path)


def _parse_since(value: str) -> datetime:
    """Parse a ``--since`` value into a UTC cutoff datetime.

    Accepts duration shorthand (``7d``, ``12h``, ``2w``) relative to now, or an
    ISO-8601 date / datetime string. A naive ISO value is read as UTC. Raises
    ``ValueError`` on anything else (mapped to CLI exit 2).
    """
    text = value.strip()
    match = _DURATION.match(text)
    if match:
        amount, unit = int(match.group(1)), match.group(2)
        return _now() - timedelta(**{_DURATION_UNITS[unit]: amount})
    parsed = datetime.fromisoformat(text)
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=UTC)
    return parsed


def _matches_tags(note_tags: list[str], want: list[str], any_tag: bool) -> bool:
    have = set(note_tags)
    return not have.isdisjoint(want) if any_tag else set(want).issubset(have)


def list_notes(
    config: Config,
    *,
    tags: list[str] | None = None,
    any_tag: bool = False,
    owner: str | None = None,
    note_type: str | None = None,
    since: str | None = None,
    sort: str = "updated",
    limit: int | None = _DEFAULT_LIMIT,
) -> list[NoteView]:
    """List shards notes under ``notes/``, filtered and sorted.

    Only files whose frontmatter carries a valid shards id (``n-`` prefix) and
    validates against :class:`Note` are surfaced; Tolaria/foreign files are
    skipped silently. Filters (all conjunctive): ``tags`` (AND, or OR with
    ``any_tag``), exact ``owner``, exact ``note_type``, and ``since`` recency on
    ``updated``. ``sort`` is ``updated``/``created`` (descending) or ``title``
    (ascending); ``limit`` caps the result (``None`` for unbounded).
    """
    if sort not in _SORT_FIELDS:
        raise ValueError(f"invalid sort field: {sort!r} (use {', '.join(_SORT_FIELDS)})")
    cutoff = _parse_since(since) if since else None

    views: list[NoteView] = []
    for path in _iter_note_files(config):
        post = read_post(path)
        if post is None:
            continue
        note_id = post.metadata.get("id")
        if not isinstance(note_id, str) or not note_id.startswith(_ID_PREFIX):
            continue
        try:
            note = Note.model_validate(post.metadata)
        except ValidationError:
            continue
        if note_type is not None and note.type != note_type:
            continue
        if owner is not None and note.owner != owner:
            continue
        if tags and not _matches_tags(note.tags, tags, any_tag):
            continue
        if cutoff is not None and note.updated < cutoff:
            continue
        views.append(NoteView(note=note, body=post.content, path=path))

    if sort == "title":
        views.sort(key=lambda v: v.note.title.lower())
    else:
        views.sort(key=lambda v: getattr(v.note, sort), reverse=True)

    if limit is not None and limit >= 0:
        return views[:limit]
    return views
