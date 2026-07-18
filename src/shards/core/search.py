"""Search core — query-execution + hit-shaping shared by the CLI and MCP surfaces.

``search`` is a recall verb with two entry points (the ``shards search`` CLI
command and the ``shards_search`` MCP tool) that must behave identically. This
module is the one home for the two mechanics they share, so neither surface has to
reach into the other:

* :func:`query_search` — route a query to hybrid ``indexed`` recall (when
  ``[search].hybrid`` is on *and* the warm daemon is up) or the substring fallback,
  degrading on any ``indexed`` failure. This is the "execution" half.
* :func:`hit_dict` — render one :class:`~shards.schemas.search.SearchResult` to the
  declared JSON hit shape, honouring ``--meta-only`` / ``--full``. This is the
  "shaping" half.

Both surfaces keep their own tag-pull branch (a query-less pull is meta-only by
nature) and their own output step (``typer.echo`` vs. a returned list); only the
query execution and hit shaping — the parts they truly share one shape on — live
here. No CLI concerns (``typer``, ``json``) leak into this module: it stays a
plain ``core`` primitive.
"""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any

import frontmatter
import yaml

from shards.daemon.client import DaemonClient
from shards.index import indexed_client
from shards.index.fallback import search_fallback
from shards.schemas.config import Config
from shards.schemas.search import SearchResult

__all__ = ["hit_dict", "query_search"]


def _daemon_up() -> bool:
    """Whether the warm daemon answers a ping (gates hybrid per the degradation matrix).

    A down/absent daemon yields ``False`` so the query takes the substring fallback —
    the same behaviour a daemon-less run has always had, and what keeps recall correct
    when ``indexed``'s index may be stale. Kept as a module-level seam so tests can
    fake daemon liveness without a live socket.
    """
    return DaemonClient().is_up()


def query_search(
    config: Config,
    query: str,
    *,
    type_filter: str | None,
    tags: list[str] | None,
    owner: str | None,
    status: str | None,
    limit: int,
    threshold: float,
    quiet: bool,
) -> list[SearchResult]:
    """Route a query to hybrid ``indexed`` recall or the substring fallback.

    Hybrid runs only when ``[search].hybrid`` is on *and* the daemon is up; any
    ``indexed`` failure (missing binary or non-zero exit) degrades to the substring
    fallback, which prints its own stderr notice. Every filter — ``type`` / ``tags``
    / ``owner`` / ``status`` — is applied post-retrieval by both paths; ``quiet`` is
    forwarded so a degradation notice honours ``--quiet`` whichever path emits it.
    """
    if config.search.hybrid and _daemon_up():
        try:
            return indexed_client.search(
                config,
                query,
                limit=limit,
                threshold=threshold,
                type_filter=type_filter,
                tags=tags,
                owner=owner,
                status=status,
                quiet=quiet,
            )
        except (subprocess.CalledProcessError, FileNotFoundError):
            pass  # indexed unavailable → fall through to the substring scan below
    return search_fallback(
        config,
        query,
        type_filter=type_filter,
        tags=tags,
        owner=owner,
        status=status,
        limit=limit,
        threshold=threshold,
        quiet=quiet,
    )


def _read_body(path: str) -> str:
    """Return the Markdown body at ``path`` (empty on a read/parse failure)."""
    try:
        return frontmatter.loads(Path(path).read_text(encoding="utf-8")).content
    except (OSError, yaml.YAMLError):
        return ""


def hit_dict(result: SearchResult, *, meta_only: bool, full: bool) -> dict[str, Any]:
    """Render one hit to the JSON shape, honouring ``--meta-only`` / ``--full``.

    Optional keys appear only when populated: ``tags`` (non-empty), ``owner``,
    ``updated``. The ``snippet`` field carries an excerpt by default and the full
    Markdown body under ``--full``; ``--meta-only`` drops it entirely (keeping the
    single declared hit shape — there is no separate ``body`` key).
    """
    hit: dict[str, Any] = {
        "id": result.id,
        "type": result.type,
        "title": result.title,
        "score": result.score,
        "path": result.path,
    }
    if result.tags:
        hit["tags"] = result.tags
    if result.owner is not None:
        hit["owner"] = result.owner
    if result.updated is not None:
        hit["updated"] = result.updated.isoformat()
    if meta_only:
        return hit
    if full:
        hit["snippet"] = _read_body(result.path)
    elif result.snippet is not None:
        hit["snippet"] = result.snippet
    return hit
