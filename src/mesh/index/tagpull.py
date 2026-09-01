"""Frontmatter tag-pull and the shared corpus scan both recall paths walk.

The **corpus** is every ``*.md`` under ``notes/`` and ``tasks/`` — a full
recursive walk, deliberately broader than ``note list`` / ``task list`` (which
gate on a valid mesh id). Coexisting non-mesh (foreign) Markdown is included;
it surfaces with ``id: None``.

:func:`tagpull` is the ``--tags``-without-query path (product R2): it reads
*frontmatter only* — no body scan, no embedding, zero ``indexed`` cost — and
returns every file matching the (conjunctive) tag / type / owner / status
filters with a fixed ``score`` of ``1.0``. :func:`iter_corpus`, :func:`read_row`,
and the small metadata coercers live here too because the substring
:mod:`~mesh.index.fallback` reuses the exact same corpus and parsing.
"""

from __future__ import annotations

from collections.abc import Iterable, Iterator, Mapping
from dataclasses import dataclass
from datetime import date, datetime
from pathlib import Path
from typing import Any

from mesh.core.notes import _opt_int, _opt_str, _str_tuple
from mesh.index.warm import iter_vault_md
from mesh.schemas.config import Config
from mesh.schemas.search import SearchResult
from mesh.storage.files import read_post

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

    Delegates to :func:`mesh.index.warm.iter_vault_md`, which the warm index
    warms from — one walk implementation, so a file the daemon holds and a file
    the disk scan finds are by construction the same set.
    """
    yield from iter_vault_md(config.core.vault_path)


def read_row(path: Path) -> CorpusRow | None:
    """Parse one corpus file into a :class:`CorpusRow`; ``None`` if unreadable.

    Thin adapter over :func:`mesh.storage.files.read_post` — the single safe
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


def mesh_id(meta: dict[str, Any]) -> str | None:
    """The frontmatter ``id`` iff it is a mesh id (``n-``/``t-``), else ``None``."""
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
        id=mesh_id(meta),
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


@dataclass(frozen=True)
class TagPullFilter:
    """A normalized, socket-transportable tag-pull filter spec.

    The tag-pull twin of :class:`mesh.core.notes.NoteFilter`: built once at the
    caller's boundary, then either applied locally by :func:`select_tagpull` or
    shipped to the daemon's ``search.tag_pull`` handler, which rebuilds it with
    :meth:`from_params` and applies the *same* selector to warm index rows.
    """

    tags: tuple[str, ...] | None = None
    type_filter: str | None = None
    owner: str | None = None
    status: str | None = None
    limit: int = _DEFAULT_LIMIT

    @classmethod
    def build(
        cls,
        *,
        tags: list[str] | None = None,
        type_filter: str | None = None,
        owner: str | None = None,
        status: str | None = None,
        limit: int = _DEFAULT_LIMIT,
    ) -> TagPullFilter:
        """Normalize the caller-level arguments into a spec (nothing to validate)."""
        return cls(
            tags=tuple(tags) if tags else None,
            type_filter=type_filter,
            owner=owner,
            status=status,
            limit=limit,
        )

    def to_params(self) -> dict[str, Any]:
        """Render the spec as JSON-safe RPC params."""
        return {
            "tags": list(self.tags) if self.tags else None,
            "type_filter": self.type_filter,
            "owner": self.owner,
            "status": self.status,
            "limit": self.limit,
        }

    @classmethod
    def from_params(cls, params: Mapping[str, Any]) -> TagPullFilter:
        """Rebuild a spec from untrusted RPC params (see :meth:`NoteFilter.from_params`)."""
        limit = _opt_int(params.get("limit"))
        return cls(
            tags=_str_tuple(params.get("tags")),
            type_filter=_opt_str(params.get("type_filter")),
            owner=_opt_str(params.get("owner")),
            status=_opt_str(params.get("status")),
            limit=limit if limit is not None else _DEFAULT_LIMIT,
        )


def corpus_rows(config: Config) -> Iterator[tuple[Path, dict[str, Any]]]:
    """Yield ``(path, frontmatter)`` for every readable corpus file — the on-disk rows.

    Bodies are deliberately dropped: a tag pull is frontmatter-only, so this is
    exactly the projection the warm index holds.
    """
    for path in iter_corpus(config):
        row = read_row(path)
        if row is None:
            continue
        yield path, row.meta


def select_tagpull(
    rows: Iterable[tuple[Path, dict[str, Any]]], spec: TagPullFilter
) -> list[SearchResult]:
    """Apply ``spec`` to ``rows`` — the *one* tag-pull filter/sort/limit implementation.

    Called with on-disk rows by :func:`tagpull` and with warm-index corpus rows by
    the daemon's ``search.tag_pull`` handler, so the two can never drift. Every
    row whose frontmatter satisfies the (conjunctive) ``tags`` **AND** ``type`` /
    ``owner`` / ``status`` filters is returned with ``score=1.0`` and no
    ``snippet`` (zero body cost); foreign rows surface with ``id=None``. Results
    are ``updated``-descending, tie-broken by path so the order is deterministic
    and identical on both paths, capped at ``limit`` (``limit < 0`` → unbounded).
    """
    results: list[SearchResult] = []
    for path, meta in rows:
        file_tags = meta_tags(meta)
        if not matches_filters(
            meta,
            file_tags,
            tags=list(spec.tags) if spec.tags else None,
            type_filter=spec.type_filter,
            owner=spec.owner,
            status=spec.status,
        ):
            continue
        results.append(to_result(path, meta, score=1.0, snippet=None))

    results.sort(key=lambda r: r.path)  # deterministic tie order under a stable sort
    results.sort(key=updated_key, reverse=True)
    if spec.limit >= 0:
        return results[: spec.limit]
    return results


def tagpull(
    config: Config,
    tags: list[str] | None = None,
    type_filter: str | None = None,
    owner: str | None = None,
    limit: int = _DEFAULT_LIMIT,
    status: str | None = None,
) -> list[SearchResult]:
    """Frontmatter-only tag pull over the whole corpus — the on-disk path (product R2).

    A thin composition of :func:`corpus_rows` (the walk) and
    :func:`select_tagpull` (the predicate); see the latter for the filter/sort/
    limit semantics. This is also the daemon-down fallback behind
    :meth:`DaemonClient.tag_pull <mesh.daemon.client.DaemonClient.tag_pull>`.
    """
    return select_tagpull(
        corpus_rows(config),
        TagPullFilter.build(
            tags=tags,
            type_filter=type_filter,
            owner=owner,
            status=status,
            limit=limit,
        ),
    )
