"""Substring recall — the daemon-independent degradation path (product R4).

When ``indexed`` is unavailable (daemon down, no engine, or ``hybrid=false``),
``shards search "<q>"`` falls back to a deterministic in-process substring scan
over the same corpus :mod:`~shards.index.tagpull` walks. Each file is scored by
the highest matching tier of a fixed matrix and results below ``threshold`` are
dropped:

| Tier            | Score |
|-----------------|-------|
| title exact     | 1.0   |
| title substring | 0.8   |
| tag contains    | 0.6   |
| body substring  | 0.4   |

Results sort by ``score`` descending, ties broken by ``updated`` descending. The
fallback emits **exactly one** stderr notice per call so a human knows recall is
degraded (suppressed under ``--quiet``); the JSON payload itself never carries
infrastructure text.
"""

from __future__ import annotations

import sys

from shards.index.tagpull import (
    _str_or_none,
    iter_corpus,
    matches_filters,
    meta_tags,
    read_row,
    to_result,
    updated_key,
)
from shards.schemas.config import Config
from shards.schemas.search import SearchResult

_DEFAULT_LIMIT = 10
_DEFAULT_THRESHOLD = 0.65
_SNIPPET_CHARS = 200

FALLBACK_NOTICE = "search: using substring fallback (indexed unavailable)"

# Highest matching tier wins.
_TITLE_EXACT = 1.0
_TITLE_SUBSTRING = 0.8
_TAG_CONTAINS = 0.6
_BODY_SUBSTRING = 0.4


def _emit_notice(quiet: bool) -> None:
    """Emit the single degradation notice on stderr (silent under ``quiet``)."""
    if not quiet:
        print(FALLBACK_NOTICE, file=sys.stderr)


def _score(query_lower: str, title: str | None, file_tags: list[str], body: str) -> float:
    """The highest matching matrix tier for ``query_lower`` (``0.0`` = no match)."""
    title_lower = (title or "").lower()
    if title_lower == query_lower:
        return _TITLE_EXACT
    if query_lower in title_lower:
        return _TITLE_SUBSTRING
    if any(query_lower in tag.lower() for tag in file_tags):
        return _TAG_CONTAINS
    if query_lower in body.lower():
        return _BODY_SUBSTRING
    return 0.0


def _snippet(body: str) -> str | None:
    """A short body excerpt for the hit (``None`` for an empty body)."""
    text = body.strip()
    if not text:
        return None
    return text[:_SNIPPET_CHARS]


def search_fallback(
    config: Config,
    query: str,
    type_filter: str | None = None,
    tags: list[str] | None = None,
    owner: str | None = None,
    limit: int = _DEFAULT_LIMIT,
    threshold: float = _DEFAULT_THRESHOLD,
    status: str | None = None,
    quiet: bool = False,
) -> list[SearchResult]:
    """Substring-scan the corpus for ``query``, scored by the fallback matrix.

    Walks every ``*.md`` under ``notes/`` + ``tasks/`` (foreign files included,
    surfacing with ``id=None``), applies the conjunctive ``tags`` **AND** ``type``
    / ``owner`` / ``status`` filters, scores by the highest matching tier, and
    drops anything below ``threshold``. Emits exactly one stderr degradation
    notice (unless ``quiet``). Sorted ``score`` descending, ``updated``
    descending on ties, capped at ``limit`` (``limit < 0`` → unbounded).
    """
    _emit_notice(quiet)
    query_lower = query.lower()

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
        score = _score(query_lower, _str_or_none(row.meta.get("title")), file_tags, row.body)
        if score < threshold:
            continue
        results.append(to_result(path, row.meta, score=score, snippet=_snippet(row.body)))

    # Stable two-phase sort: updated desc first, then score desc — final order is
    # score desc with updated desc breaking ties.
    results.sort(key=updated_key, reverse=True)
    results.sort(key=lambda r: r.score, reverse=True)
    if limit >= 0:
        return results[:limit]
    return results
