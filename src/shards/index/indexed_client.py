"""Thin wrapper over the first-party ``indexed`` CLI — the hybrid recall engine.

Shards owns the *interface* to search, not the ranking: this module shells the
``indexed`` binary and maps its hits back onto the uniform :class:`SearchResult`
shape. Three responsibilities, all daemon-optional:

* **:func:`search`** — run ``indexed index search "<q>" --collection <c> --json
  --limit N``, parse the NDJSON hits (``{path, score, snippet}``), and rehydrate
  each into a :class:`SearchResult` by reading the frontmatter at ``path``. Foreign
  (non-shards) files surface with ``id=None``; paths that escape the vault sandbox
  or no longer exist are dropped. Results sort by ``score`` descending with a
  recency tiebreak: two hits whose scores fall within :data:`_TIEBREAK_EPSILON`
  order by ``updated`` descending. When ``[search].collection`` is unset the engine
  is disabled and the call degrades to the in-process substring
  :func:`~shards.index.fallback.search_fallback`.

* **:func:`incremental_update` / :func:`full_rebuild` / :func:`reindex`** — keep the
  ``indexed`` collection fresh: update one path (``indexed index update``) or
  rebuild the whole vault (``indexed index create``). Both are silent no-ops when no
  collection is configured. :func:`incremental_update` runs on the watcher thread,
  so it additionally **swallows** every subprocess failure (a dead ``indexed`` must
  never crash the observer); :func:`full_rebuild` / :func:`reindex` run on the main
  thread and let failures propagate. ``reindex`` is the delegate ``shards reindex``
  calls.

* **:func:`register_hook`** — subscribe :func:`incremental_update` to a daemon-owned
  :class:`~shards.index.watcher.ChangeHooks` registry so a vault edit re-indexes
  just that file. The mechanism lives here; the daemon owns the registry and wires
  it up.

The subprocess calls live behind the small ``_run_indexed_*`` seams so the real
``indexed`` binary is never invoked in unit tests (they monkeypatch the seam). For
:func:`search` (and :func:`full_rebuild`) a missing binary (``FileNotFoundError``)
or a non-zero exit (``subprocess.CalledProcessError``) propagates to the caller,
which degrades to the substring fallback; :func:`incremental_update` alone
swallows these, because it runs on the watcher thread.
"""

from __future__ import annotations

import contextlib
import functools
import json
import subprocess
from pathlib import Path
from typing import TYPE_CHECKING, Any

from shards.index.fallback import search_fallback
from shards.index.tagpull import (
    matches_filters,
    meta_tags,
    read_row,
    to_result,
    updated_key,
)
from shards.schemas.config import Config
from shards.schemas.search import SearchResult
from shards.storage.sandbox import safe_resolve

if TYPE_CHECKING:
    # Typing-only import: keep the watcher (and its ``watchdog`` dependency) off
    # this module's runtime import graph so the CLI search path stays cheap.
    from shards.index.watcher import ChangeHooks

_INDEXED_BIN = "indexed"
_DEFAULT_LIMIT = 10
_DEFAULT_THRESHOLD = 0.65

# Two hits whose scores fall within this band are treated as tied on score, and
# broken by recency (``updated`` descending) per search/tech.md.
_TIEBREAK_EPSILON = 0.02


# --------------------------------------------------------------------------- #
# Subprocess seams (mocked in unit tests; the only place ``indexed`` is run)   #
# --------------------------------------------------------------------------- #


def _run_indexed_search(collection: str, query: str, limit: int) -> str:
    """Run ``indexed index search`` and return its raw NDJSON stdout.

    ``query`` is passed as a distinct argv element (never through a shell), so
    agent-authored text stays inert data. Raises ``FileNotFoundError`` if the
    binary is absent and ``subprocess.CalledProcessError`` on a non-zero exit.
    """
    proc = subprocess.run(
        [
            _INDEXED_BIN,
            "index",
            "search",
            query,
            "--collection",
            collection,
            "--json",
            "--limit",
            str(limit),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    return proc.stdout


def _run_indexed_update(path: Path, collection: str) -> None:
    """Run ``indexed index update <path> --collection <c>`` (incremental re-index)."""
    subprocess.run(
        [_INDEXED_BIN, "index", "update", str(path), "--collection", collection],
        check=True,
        capture_output=True,
        text=True,
    )


def _run_indexed_create(root: Path, collection: str) -> None:
    """Run ``indexed index create <root> --collection <c>`` (full rebuild)."""
    subprocess.run(
        [_INDEXED_BIN, "index", "create", str(root), "--collection", collection],
        check=True,
        capture_output=True,
        text=True,
    )


# --------------------------------------------------------------------------- #
# Hit parsing + ordering                                                       #
# --------------------------------------------------------------------------- #


def _parse_ndjson(text: str) -> list[dict[str, Any]]:
    """Parse ``indexed``'s NDJSON stdout into hit dicts (blank/garbled lines skipped)."""
    hits: list[dict[str, Any]] = []
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        try:
            obj = json.loads(stripped)
        except json.JSONDecodeError:
            continue
        if isinstance(obj, dict):
            hits.append(obj)
    return hits


def _compare(a: SearchResult, b: SearchResult) -> int:
    """Order two hits: score descending, with a recency tiebreak within the band.

    When the two scores fall within :data:`_TIEBREAK_EPSILON` the more recently
    ``updated`` file sorts first (older/undated last); otherwise the higher score
    sorts first.
    """
    if abs(a.score - b.score) <= _TIEBREAK_EPSILON:
        ta, tb = updated_key(a), updated_key(b)
        if ta != tb:
            return -1 if ta > tb else 1
        if a.score != b.score:
            return -1 if a.score > b.score else 1
        return 0
    return -1 if a.score > b.score else 1


def _hit_score(hit: dict[str, Any]) -> float | None:
    """The hit's numeric ``score`` (``None`` if missing or non-numeric)."""
    raw = hit.get("score")
    if isinstance(raw, bool) or not isinstance(raw, (int, float)):
        return None
    return float(raw)


# --------------------------------------------------------------------------- #
# Public API                                                                   #
# --------------------------------------------------------------------------- #


def search(
    config: Config,
    query: str,
    limit: int = _DEFAULT_LIMIT,
    threshold: float = _DEFAULT_THRESHOLD,
    type_filter: str | None = None,
    tags: list[str] | None = None,
    owner: str | None = None,
    status: str | None = None,
    quiet: bool = False,
) -> list[SearchResult]:
    """Hybrid recall via ``indexed``; substring fallback when no collection is set.

    Shells ``indexed index search`` for ``query``, maps each NDJSON hit to a
    :class:`SearchResult` (reading frontmatter at the hit ``path``; foreign files →
    ``id=None``), and applies the conjunctive ``tags`` **AND** ``type`` / ``owner`` /
    ``status`` filters plus the ``threshold`` floor. Ordered by ``score`` descending
    with a recency tiebreak (scores within :data:`_TIEBREAK_EPSILON` break on
    ``updated`` descending), capped at ``limit``. Hits whose ``path`` is missing or
    escapes the vault sandbox are dropped.

    With ``[search].collection`` unset, ``indexed`` is disabled and the call
    delegates to :func:`~shards.index.fallback.search_fallback` (forwarding ``quiet``
    so its degradation notice honours the caller's ``--quiet``). A missing binary or
    non-zero exit propagates to the caller.
    """
    collection = config.search.collection
    if collection is None:
        return search_fallback(
            config,
            query,
            type_filter=type_filter,
            tags=tags,
            owner=owner,
            limit=limit,
            threshold=threshold,
            status=status,
            quiet=quiet,
        )

    raw = _run_indexed_search(collection, query, limit)
    vault = config.core.tolaria_path

    results: list[SearchResult] = []
    for hit in _parse_ndjson(raw):
        raw_path = hit.get("path")
        if not isinstance(raw_path, str):
            continue
        score = _hit_score(hit)
        if score is None or score < threshold:
            continue
        try:
            path = safe_resolve(vault, Path(raw_path))
        except ValueError:
            continue  # a hit outside the vault sandbox is never read
        row = read_row(path)
        if row is None:
            continue  # vanished or unreadable file
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
        snippet = hit.get("snippet")
        results.append(
            to_result(
                path,
                row.meta,
                score=score,
                snippet=snippet if isinstance(snippet, str) else None,
            )
        )

    results.sort(key=functools.cmp_to_key(_compare))
    if limit >= 0:
        return results[:limit]
    return results


def incremental_update(config: Config, path: Path) -> None:
    """Re-index a single vault ``path`` in ``indexed`` — best-effort, never raising.

    A no-op when no collection is configured. Any failure of the ``indexed``
    subprocess — a missing binary (``FileNotFoundError``), a non-zero exit
    (``subprocess.CalledProcessError``), or an OS-level error — is **swallowed**.
    This runs on the watchdog observer thread via the watcher change-hook, and an
    escaping exception would kill that thread and freeze freshness for the daemon's
    whole lifetime. Incremental freshness is best-effort; ``shards reindex`` (a full
    rebuild) is the recovery path when an incremental update silently misses.
    """
    collection = config.search.collection
    if collection is None:
        return
    # Best-effort re-index (see docstring): a non-zero exit
    # (``CalledProcessError``) or any ``OSError`` — including a missing ``indexed``
    # binary (``FileNotFoundError`` is an ``OSError`` subclass) — is suppressed so
    # this never propagates onto the watcher thread.
    with contextlib.suppress(subprocess.CalledProcessError, OSError):
        _run_indexed_update(path, collection)


def full_rebuild(config: Config) -> None:
    """Rebuild the whole ``indexed`` collection from the vault (no-op if unset)."""
    collection = config.search.collection
    if collection is None:
        return
    _run_indexed_create(config.core.tolaria_path, collection)


def reindex(config: Config) -> None:
    """Full rebuild — the delegate ``shards reindex`` calls."""
    full_rebuild(config)


def register_hook(config: Config, hooks: ChangeHooks) -> None:
    """Subscribe :func:`incremental_update` to a daemon-owned change-hook registry."""

    def _hook(path: Path) -> None:
        incremental_update(config, path)

    hooks.register(_hook)


__all__ = [
    "full_rebuild",
    "incremental_update",
    "register_hook",
    "reindex",
    "search",
]
