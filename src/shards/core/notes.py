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
from collections.abc import Iterable, Iterator, Mapping
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any, get_args

import frontmatter
from msgspec import ValidationError

from shards.core.errors import ShardsError
from shards.core.ids import generate_note_id
from shards.core.wikilinks import resolve_wikilinks
from shards.schemas.config import Config
from shards.schemas.note import Note, NoteType
from shards.storage.files import atomic_write, note_folder, read_post
from shards.storage.locks import allocator_lock_path, hold
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


class NoteError(ShardsError):
    """Base class for note-resolution failures."""


class NoteNotFoundError(NoteError):
    """No note matches the given id or slug (CLI exit 3)."""

    code = 3

    def __init__(self, id_or_slug: str) -> None:
        self.id_or_slug = id_or_slug
        super().__init__(f"note not found: {id_or_slug}")


class AmbiguousSlugError(NoteError):
    """A slug matches more than one note (CLI exit 2).

    Carries the matching ids so the CLI can list them in its exit-2 message.
    """

    code = 2

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
    """Yield every ``*.md`` under ``notes/`` (``.locks/`` holds no ``.md``).

    The *canonical* note scope: :func:`_resolve_path`, :func:`_id_taken` and
    :func:`note_rows` all read through here, so a file outside it is not a note as
    far as this program is concerned. :func:`in_note_scope` is the membership form
    of this same scope — keep the two in step.
    """
    root = _notes_root(config)
    if not root.is_dir():
        return
    yield from root.rglob("*.md")


def in_note_scope(vault: Path, path: Path) -> bool:
    """Whether ``path`` lies in the folder scope :func:`_iter_note_files` walks.

    The membership form of that walk, for a caller holding a path rather than a
    directory: the daemon's warm handlers hold one vault-wide index and must
    project it down to exactly the rows the on-disk walk would have yielded.
    Without this the warm answer is a *superset* of the cold one — an ``n-`` note
    filed under ``tasks/`` is in the index but outside the note walk — and the two
    paths stop agreeing. ``tests/daemon/test_warm_reads.py`` asserts the walk and
    the predicate select the same set over a deliberately misfiled corpus.
    """
    return path.suffix == ".md" and path.is_relative_to(vault / "notes")


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


def _format_stamp(iso: str, agent: str | None) -> str:
    """Render the attribution-stamp contract: ``<iso> — <agent>`` (team-awareness/8).

    The single place that contract is spelled — :func:`_format_block` (note/task
    ``append``) and :func:`shards.core.tasks._terminate_task` (``## Outcome`` /
    ``## Cancelled``) both call through here rather than each formatting their
    own line, so the "who wrote this" prose stays one formatter, not two drifting
    copies. ``agent`` is **who is actually running the command** — resolved by
    each caller from ``config.agent`` (the only identity available at these call
    sites; none of them take a separate ``--owner`` override) — never the note's
    ``owner`` field, which only names who a task/note is accountable to and may
    be a completely different agent than the one appending right now (the bug
    this unit fixes: an editor's append was silently attributed to the creator).
    When ``agent`` is unset (no ``[core].agent``, no ``$SHARDS_AGENT``) this
    degrades to the bare ``iso`` — no trailing separator, no placeholder — so a
    misconfigured caller still gets a clean, human-readable line instead of a
    dangling ``— None`` or a crash. This is prose, not metadata: nothing indexes
    or queries the name embedded here, and no new frontmatter key is added
    (rejected by the spec — see the feature plan's "Attribution on stamps"
    entry).
    """
    return f"{iso} — {agent}" if agent else iso


def _format_block(text: str, timestamp: bool, agent: str | None = None) -> str:
    if timestamp:
        iso = _now().strftime("%Y-%m-%dT%H:%M:%SZ")
        return f"{_format_stamp(iso, agent)}\n{text}"
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

    Id allocation (``_id_taken`` scan + the ``generate_note_id`` extension loop)
    and the write both run under the per-kind allocator lock at
    ``notes/.locks/_create.lock`` (see :func:`shards.storage.locks.allocator_lock_path`),
    so two concurrent creates that resolve the same candidate id can no longer
    both pass the check and race ``os.replace`` — the second waits, rescans, and
    extends past the collision instead of destroying the first file. A per-entity
    lock cannot serve here (the id does not exist yet to name one), hence the
    coarser per-kind lock; contention is bounded because a create is one scan
    plus one write.
    """
    if note_type not in _NOTE_TYPES:
        raise ValueError(f"invalid note type: {note_type!r}")
    _validate_owner(config, owner)

    vault = config.core.tolaria_path
    with hold(allocator_lock_path(_notes_root(config))):
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
        # Serialize the frontmatter from the validated model — the schema is the
        # one on-disk contract, never a parallel hand-built dict that can drift.
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
    to the text block, naming the acting agent (``<iso> — <agent>``, team-awareness/8
    — see :func:`_format_stamp`) resolved from ``config.agent``: the identity of
    *this* call, not the note's ``owner``, so an editor's append is attributed to
    the editor even when it lands on a note it does not own — the observed gap
    this unit closes. The whole read-modify-write runs under the entity lock and
    the result is written atomically. The re-read inside the lock goes through
    :func:`shards.storage.files.read_post`; a file that vanishes or turns
    unreadable between resolution and the lock raises
    :class:`NoteNotFoundError`, matching :func:`get_note`.
    """
    path = _resolve_path(config, id_or_slug)
    note_id = path.stem
    block = _format_block(text, timestamp, config.agent)
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


# --------------------------------------------------------------------------- #
# RPC param coercers — shared by every filter spec that crosses the socket      #
# --------------------------------------------------------------------------- #
#
# Filter specs are rebuilt daemon-side from JSON that arrived over a socket, so
# every field is untrusted: a wrong-typed value degrades to ``None``/the default
# rather than raising, which would turn a garbled param into a 500 the caller
# cannot fall back from. Defined once here and reused by the task and tag-pull
# specs (``core.tasks``, ``index.tagpull``) — the same import direction those
# modules already use for ``_matches_tags`` / ``_parse_since``.


def _opt_str(value: object) -> str | None:
    return value if isinstance(value, str) else None


def _opt_int(value: object) -> int | None:
    """An ``int`` or ``None``; ``bool`` is excluded (a stray ``true`` must not cap to 1)."""
    if isinstance(value, bool) or not isinstance(value, int):
        return None
    return value


def _str_tuple(value: object) -> tuple[str, ...] | None:
    """A non-empty tuple of strings, or ``None`` (absent / empty / wrong type)."""
    if not isinstance(value, list):
        return None
    items = tuple(item for item in value if isinstance(item, str))
    return items or None


def _opt_datetime(value: object) -> datetime | None:
    """An ISO-8601 string parsed to an aware UTC datetime, or ``None``."""
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.fromisoformat(value)
    except ValueError:
        return None
    return parsed.replace(tzinfo=UTC) if parsed.tzinfo is None else parsed


# --------------------------------------------------------------------------- #
# The one note list predicate — shared by the disk walk and the warm index      #
# --------------------------------------------------------------------------- #

# One candidate row: a path and its parsed frontmatter. Deliberately **no body**:
# the daemon's warm index holds frontmatter only, so a body-bearing row shape
# could never be filled warm — and a list result whose ``body`` silently emptied
# whenever the daemon came up would be worse than one that never carries it. The
# views a list produces therefore always have ``body=""``; a caller that needs a
# body reads it per id (``get_note``/``get_task``, or ``storage.files.read_body``).
MetaRow = tuple[Path, Mapping[str, Any]]


@dataclass(frozen=True)
class NoteFilter:
    """A normalized, socket-transportable ``note list`` filter/sort/limit spec.

    Built once at the caller's boundary by :meth:`build` (which is where the
    caller-facing strings — ``sort``, ``--since`` — are *validated*, so a bad
    value fails the same way whether the daemon is up or down), then either
    applied locally by :func:`select_notes` or shipped over the socket via
    :meth:`to_params` and rebuilt daemon-side by :meth:`from_params`.
    """

    tags: tuple[str, ...] | None = None
    any_tag: bool = False
    owner: str | None = None
    note_type: str | None = None
    cutoff: datetime | None = None
    sort: str = "updated"
    limit: int | None = _DEFAULT_LIMIT

    @classmethod
    def build(
        cls,
        *,
        tags: list[str] | None = None,
        any_tag: bool = False,
        owner: str | None = None,
        note_type: str | None = None,
        since: str | None = None,
        sort: str = "updated",
        limit: int | None = _DEFAULT_LIMIT,
    ) -> NoteFilter:
        """Validate and normalize the caller-level arguments into a spec.

        Raises ``ValueError`` for an unknown ``sort`` field or an unparseable
        ``since`` (the boundary mappers turn both into exit 2) — *before* any
        socket call, so validation never depends on the daemon being up.
        """
        if sort not in _SORT_FIELDS:
            raise ValueError(f"invalid sort field: {sort!r} (use {', '.join(_SORT_FIELDS)})")
        return cls(
            tags=tuple(tags) if tags else None,
            any_tag=any_tag,
            owner=owner,
            note_type=note_type,
            cutoff=_parse_since(since) if since else None,
            sort=sort,
            limit=limit,
        )

    def to_params(self) -> dict[str, Any]:
        """Render the spec as JSON-safe RPC params."""
        return {
            "tags": list(self.tags) if self.tags else None,
            "any_tag": self.any_tag,
            "owner": self.owner,
            "note_type": self.note_type,
            "cutoff": self.cutoff.isoformat() if self.cutoff is not None else None,
            "sort": self.sort,
            "limit": self.limit,
        }

    @classmethod
    def from_params(cls, params: Mapping[str, Any]) -> NoteFilter:
        """Rebuild a spec from RPC params, coercing defensively.

        Params arrive off a socket, so every field is treated as untrusted: a
        wrong-typed value falls back to its default rather than raising, and an
        unknown ``sort`` collapses to ``updated``. The wire is not a validation
        boundary — :meth:`build` already ran on the caller's side.
        """
        sort = params.get("sort")
        return cls(
            tags=_str_tuple(params.get("tags")),
            any_tag=bool(params.get("any_tag", False)),
            owner=_opt_str(params.get("owner")),
            note_type=_opt_str(params.get("note_type")),
            cutoff=_opt_datetime(params.get("cutoff")),
            sort=sort if sort in _SORT_FIELDS else "updated",
            limit=_opt_int(params.get("limit")),
        )


def note_rows(config: Config) -> Iterator[MetaRow]:
    """Yield one :data:`MetaRow` per readable ``*.md`` under ``notes/``.

    Unreadable and malformed files are skipped by
    :func:`shards.storage.files.read_post`, so a foreign or corrupt sibling never
    crashes the walk.
    """
    for path in _iter_note_files(config):
        post = read_post(path)
        if post is None:
            continue
        yield path, post.metadata


def _title_collision(rows: Iterable[MetaRow], title: str, id_prefix: str) -> str | None:
    """Return the ``id`` of the first row whose id carries ``id_prefix`` and whose
    ``title`` slug-collides with ``title``; ``None`` if nothing matches (R9).

    "Slug-collides" means :func:`_slugify` of the two titles is equal — the
    identical normalization :func:`_resolve_path` applies for CLI ``<id|slug>``
    lookups, deliberately mirrored here (see :func:`find_duplicate_title` for
    the reasoning). A single pass, one target slug computed once and compared
    against each row — no second scan and no index built on top of the row
    iterator already being walked.

    The shared engine behind :func:`find_duplicate_title` (here) and
    :func:`shards.core.tasks.find_duplicate_title`: each passes its own row
    iterator (``note_rows``/``task_rows``) and id prefix, so a note and a task
    that happen to share a title never collide with each other — same-kind only,
    matching the product decision (R9: warn on same-kind duplicates).
    """
    target = _slugify(title)
    for _, meta in rows:
        candidate_id = meta.get("id")
        candidate_title = meta.get("title")
        if (
            isinstance(candidate_id, str)
            and candidate_id.startswith(id_prefix)
            and isinstance(candidate_title, str)
            and _slugify(candidate_title) == target
        ):
            return candidate_id
    return None


def find_duplicate_title(config: Config, title: str) -> str | None:
    """Return the id of an existing note whose title slug-collides with ``title`` (R9).

    Mirrors the **slug-normalized** rule :func:`_resolve_path` uses for CLI
    ``<id|slug>`` lookups (``_slugify`` — lower-cased, non-alphanumeric runs
    collapsed to a single ``-``, trimmed) — *not* the exact-match rule
    :func:`shards.core.wikilinks._title_index` uses for wikilink title
    resolution. That is the point: a duplicate that only ``_resolve_path``
    would consider the same is exactly the duplicate that later poisons the
    slug resolver forever (``AmbiguousSlugError``, no way back short of
    renaming/deleting) — so the warning has to use the resolver's own rule, or
    it warns about the wrong set of collisions. A title differing only by case
    or surrounding/internal whitespace **does** warn here, because it *would*
    already collide once slugified — see
    ``tests/notes/test_new.py::test_cli_new_case_whitespace_duplicate_warns_and_is_genuinely_ambiguous_slug``,
    which ties the warning to the real harm (a subsequent ambiguous-slug
    lookup), not to a string comparison.

    Same-kind only — scans ``notes/`` and never sees a task, even a
    title-identical one (see :func:`shards.core.tasks.find_duplicate_title` for
    the task-side twin).

    Reads every note's frontmatter (an ``O(vault-notes)`` scan via
    :func:`note_rows`) — the same order of cost ``create_note``'s own id-collision
    scan and the wikilink title index already pay. Call this *before* acquiring
    the create lock: it is a plain, best-effort read (a concurrent creator can
    still race past it unseen — this is advisory, not a guarantee) and must never
    extend how long the per-kind allocator lock is held.
    """
    return _title_collision(note_rows(config), title, _ID_PREFIX)


def select_notes(rows: Iterable[MetaRow], spec: NoteFilter) -> list[NoteView]:
    """Apply ``spec`` to ``rows`` — the *one* note filter/sort/limit implementation.

    Called with on-disk rows by :func:`list_notes` and with warm-index rows by the
    daemon's ``note.list`` handler, so the two paths can never drift. Only rows
    whose frontmatter carries a valid shards id (``n-`` prefix) and validates
    against :class:`Note` are surfaced; Tolaria/foreign rows are skipped silently.
    Filters (all conjunctive): ``tags`` (AND, or OR with ``any_tag``), exact
    ``owner``, exact ``note_type``, and the ``cutoff`` recency bound on
    ``updated``. ``sort`` is ``updated``/``created`` (descending) or ``title``
    (ascending), tie-broken by path so the order is deterministic and identical
    on both paths; ``limit`` caps the result (``None`` for unbounded).

    The returned views carry ``body=""`` — see :data:`MetaRow`.
    """
    views: list[NoteView] = []
    for path, meta in rows:
        note_id = meta.get("id")
        if not isinstance(note_id, str) or not note_id.startswith(_ID_PREFIX):
            continue
        try:
            note = Note.model_validate(meta)
        except ValidationError:
            continue
        if spec.note_type is not None and note.type != spec.note_type:
            continue
        if spec.owner is not None and note.owner != spec.owner:
            continue
        if spec.tags and not _matches_tags(note.tags, list(spec.tags), spec.any_tag):
            continue
        if spec.cutoff is not None and note.updated < spec.cutoff:
            continue
        views.append(NoteView(note=note, body="", path=path))

    views.sort(key=lambda v: str(v.path))  # deterministic tie order under a stable sort
    if spec.sort == "title":
        views.sort(key=lambda v: v.note.title.lower())
    else:
        views.sort(key=lambda v: getattr(v.note, spec.sort), reverse=True)

    if spec.limit is not None and spec.limit >= 0:
        return views[: spec.limit]
    return views


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
    """List shards notes under ``notes/``, filtered and sorted — the on-disk path.

    A thin composition of :func:`note_rows` (the walk) and :func:`select_notes`
    (the predicate); see the latter for the filter/sort/limit semantics, including
    why the views carry no body. This is also the daemon-down fallback behind
    :meth:`DaemonClient.note_list <shards.daemon.client.DaemonClient.note_list>`.
    """
    return select_notes(
        note_rows(config),
        NoteFilter.build(
            tags=tags,
            any_tag=any_tag,
            owner=owner,
            note_type=note_type,
            since=since,
            sort=sort,
            limit=limit,
        ),
    )
