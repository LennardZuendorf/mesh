"""Frontmatter tag-pull and the shared corpus scan both recall paths walk.

The **corpus** is every ``*.md`` under ``notes/`` and ``tasks/`` — a full
recursive walk, deliberately broader than ``note list`` / ``task list`` (which
gate on a valid brain id). Coexisting non-brain (foreign) Markdown is included;
it surfaces with ``id: None``.

:func:`tagpull` is the ``--tags``-without-query path (product R2): it reads
*frontmatter only* — no body scan, no embedding, zero ``indexed`` cost — and
returns every file matching the (conjunctive) tag / type / owner / status
filters with a fixed ``score`` of ``1.0``. :func:`iter_corpus`, :func:`read_row`,
and the small metadata coercers live here too because the substring
:mod:`~brain.index.fallback` reuses the exact same corpus and parsing.
"""

from __future__ import annotations

from collections.abc import Iterator
from dataclasses import dataclass
from datetime import date, datetime
from pathlib import Path
from typing import Any

from brain.schemas.config import Config
from brain.schemas.search import SearchResult
from brain.storage.files import read_post

_ID_PREFIXES = ("n-", "t-")
_DEFAULT_LIMIT = 10


# --------------------------------------------------------------------------- #
# Shared corpus scan + parsing (reused by fallback)                            #
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class CorpusRow:
    """One parsed corpus file: its path, frontmatter metadata, and Markdown body."""

    path: Path
    meta: dict[str, Any]
    body: str


def iter_corpus(config: Config) -> Iterator[Path]:
    """Yield every ``*.md`` under ``notes/`` and ``tasks/`` (full recursive walk).

    Recursive so typed note subfolders (``notes/decisions/`` …) and both task
    folders (``tasks/open/``, ``tasks/done/``) are covered. ``.locks/`` holds no
    ``.md`` and is naturally excluded.
    """
    vault = config.core.tolaria_path
    for sub in ("notes", "tasks"):
        root = vault / sub
        if root.is_dir():
            yield from root.rglob("*.md")


def read_row(path: Path) -> CorpusRow | None:
    """Parse one corpus file into a :class:`CorpusRow`; ``None`` if unreadable.

    Thin adapter over :func:`brain.storage.files.read_post` — the single safe
    reader that skips a vanished file (``OSError``) or a malformed frontmatter
    block (``yaml.YAMLError``) silently, so foreign and corrupt files never crash
    a scan.
    """
    post = read_post(path)
    if post is None:
        return None
    return CorpusRow(path=path, meta=dict(post.metadata), body=post.content)


def _str_or_none(value: object) -> str | None:
    return value if isinstance(value, str) else None


def brain_id(meta: dict[str, Any]) -> str | None:
    """The frontmatter ``id`` iff it is a brain id (``n-``/``t-``), else ``None``."""
    raw = meta.get("id")
    if isinstance(raw, str) and raw.startswith(_ID_PREFIXES):
        return raw
    return None


def meta_tags(meta: dict[str, Any]) -> list[str]:
    """The frontmatter ``tags`` normalised to a ``list[str]`` (empty if absent)."""
    raw = meta.get("tags")
    if isinstance(raw, list):
        return [str(t) for t in raw]
    return []


def coerce_updated(value: object) -> datetime | None:
    """Normalise a frontmatter ``updated`` value to ``datetime`` or ``None``.

    PyYAML may hand back a ``datetime``, a bare ``date``, an ISO string, or
    nothing; a strict ``datetime | None`` field (and the sort key) needs one
    clean type. ``date`` (checked after ``datetime``, since it is a supertype)
    is promoted to midnight; an unparseable string yields ``None``.
    """
    if isinstance(value, datetime):
        return value
    if isinstance(value, date):
        return datetime(value.year, value.month, value.day)
    if isinstance(value, str):
        try:
            return datetime.fromisoformat(value)
        except ValueError:
            return None
    return None


def to_result(
    path: Path,
    meta: dict[str, Any],
    *,
    score: float,
    snippet: str | None,
) -> SearchResult:
    """Build a :class:`SearchResult` from parsed metadata (``id=None`` if foreign)."""
    tags = meta_tags(meta)
    return SearchResult(
        id=brain_id(meta),
        type=_str_or_none(meta.get("type")),
        title=_str_or_none(meta.get("title")),
        score=score,
        tags=tags or None,
        owner=_str_or_none(meta.get("owner")),
        updated=coerce_updated(meta.get("updated")),
        snippet=snippet,
        path=str(path),
    )


def matches_filters(
    meta: dict[str, Any],
    file_tags: list[str],
    *,
    tags: list[str] | None,
    type_filter: str | None,
    owner: str | None,
    status: str | None,
) -> bool:
    """Whether ``meta`` passes every provided filter (all conjunctive).

    ``tags`` is **AND** semantics: the file must carry *all* requested tags.
    ``type`` / ``owner`` / ``status`` are exact frontmatter equality.
    """
    if tags and not set(tags).issubset(set(file_tags)):
        return False
    if type_filter is not None and _str_or_none(meta.get("type")) != type_filter:
        return False
    if owner is not None and _str_or_none(meta.get("owner")) != owner:
        return False
    if status is not None and _str_or_none(meta.get("status")) != status:  # noqa: SIM103
        return False
    return True


def updated_key(result: SearchResult) -> float:
    """Sort key: ``updated`` as a POSIX timestamp; missing dates sort last (desc)."""
    if result.updated is None:
        return float("-inf")
    return result.updated.timestamp()


# --------------------------------------------------------------------------- #
# Tag pull                                                                     #
# --------------------------------------------------------------------------- #


def tagpull(
    config: Config,
    tags: list[str] | None = None,
    type_filter: str | None = None,
    owner: str | None = None,
    limit: int = _DEFAULT_LIMIT,
    status: str | None = None,
) -> list[SearchResult]:
    """Frontmatter-only tag pull over the whole corpus (product R2).

    Every ``*.md`` under ``notes/`` + ``tasks/`` whose frontmatter satisfies the
    (conjunctive) ``tags`` **AND** ``type`` / ``owner`` / ``status`` filters is
    returned with ``score=1.0`` and no ``snippet`` (zero body cost). Foreign
    files surface with ``id=None``. Results are ``updated``-descending, capped at
    ``limit`` (``limit < 0`` → unbounded).
    """
    results: list[SearchResult] = []
    for path in iter_corpus(config):
        row = read_row(path)
        if row is None:
            continue
        file_tags = meta_tags(row.meta)
        if not matches_filters(
            row.meta,
            file_tags,
            tags=tags,
            type_filter=type_filter,
            owner=owner,
            status=status,
        ):
            continue
        results.append(to_result(path, row.meta, score=1.0, snippet=None))

    results.sort(key=updated_key, reverse=True)
    if limit >= 0:
        return results[:limit]
    return results
