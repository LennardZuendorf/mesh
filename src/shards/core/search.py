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
from datetime import datetime
from pathlib import Path
from typing import Any

from shards.daemon.client import DaemonClient
from shards.index import indexed_client
from shards.index.fallback import search_fallback
from shards.schemas.config import Config
from shards.schemas.search import SearchResult
from shards.storage.files import read_body

__all__ = ["hit_dict", "query_search", "resolve_effective_threshold", "search_health"]


def resolve_effective_threshold(flag: float | None, config: Config) -> float | None:
    """Resolve the ``threshold`` to pass to :func:`query_search`.

    ``None`` only when neither an explicit caller value (``--threshold`` on the
    CLI, or the equivalent typed MCP parameter) *nor* an explicit
    ``[search].threshold`` in config was given — :func:`query_search` then lets
    the substring fallback apply its own floor (root tech.md § B5) instead of a
    silently-defaulted cutoff. Shared by the ``shards search`` CLI command and
    the ``shards_search`` MCP tool so this three-way resolution is one mechanism,
    not two copies that could drift (the two surfaces "must behave identically",
    per the module docstring above).
    """
    if flag is not None:
        return flag
    if config.search.threshold_explicit():
        return config.search.threshold
    return None


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
    threshold: float | None,
    quiet: bool,
) -> tuple[list[SearchResult], str]:
    """Route a query to hybrid ``indexed`` recall or the substring fallback.

    Returns ``(results, mode)`` — ``mode`` is ``"indexed"`` only when
    ``indexed`` actually answered, ``"fallback"`` otherwise (agent-usability/4,
    round-1 review Finding 1). This is the branch *actually taken*, not a
    prediction from :func:`search_health`'s static gates: those gates cannot
    see a genuine runtime failure — a non-zero ``indexed`` exit for a reason
    other than "binary absent" (corrupt collection, resource exhaustion, an
    internal crash) — so a caller computing mode from the gates alone would
    confidently mislabel a substring hit as ranked recall. Reusing this
    return value is now the only supported way to know which engine answered;
    computing it independently via a second :func:`search_health` call is the
    bug this replaces.

    Hybrid is attempted only when ``[search].hybrid`` is on, the daemon is up,
    *and* ``[search].collection`` is set (hoisted here from
    ``indexed_client.search``'s own internal check, so an unset collection
    short-circuits to the fallback branch below directly, structurally
    mirroring the gates :func:`search_health` reports, rather than round-
    tripping through ``indexed_client.search``'s own silent redirect and
    risking a mode mismatch). Any ``indexed`` failure (missing binary or
    non-zero exit) degrades to the substring fallback, which prints its own
    stderr notice. Every filter — ``type`` / ``tags`` / ``owner`` / ``status``
    — is applied post-retrieval by both paths; ``quiet`` is forwarded so a
    degradation notice honours ``--quiet`` whichever path emits it.

    ``threshold`` is ``None`` when the caller has no *explicit* value (neither
    ``--threshold`` nor an explicit ``[search].threshold`` in config) — the
    substring fallback then applies its own floor (root tech.md § B5) rather
    than a silently-defaulted cutoff. The ``indexed`` path is unaffected by that
    rule: it always gets a concrete value, defaulting to ``[search].threshold``
    when the caller left it unset.
    """
    if config.search.hybrid and config.search.collection is not None and _daemon_up():
        try:
            results = indexed_client.search(
                config,
                query,
                limit=limit,
                threshold=threshold if threshold is not None else config.search.threshold,
                type_filter=type_filter,
                tags=tags,
                owner=owner,
                status=status,
                quiet=quiet,
            )
            return results, "indexed"
        except (subprocess.CalledProcessError, FileNotFoundError):
            pass  # indexed unavailable → fall through to the substring scan below
    results = search_fallback(
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
    return results, "fallback"


def search_health(config: Config) -> dict[str, Any]:
    """Report which recall path a query would take right now — never raises.

    Silent degradation (root ``tech.md`` § Risks — "``indexed`` drift") means a
    caller has no way to tell a hybrid ``indexed`` hit apart from a substring
    fallback hit short of reading a stderr notice. This surfaces the individual
    gates :func:`query_search` checks — ``hybrid_configured`` (``[search].hybrid``),
    ``collection`` (``[search].collection``), ``daemon_up`` (the warm daemon
    liveness ping), and ``indexed_binary_available`` (whether the ``indexed``
    executable is even on ``PATH``, via :func:`~shards.index.indexed_client.indexed_available`)
    — plus the resulting ``mode`` (``"indexed"`` only when every gate is open,
    ``"fallback"`` otherwise) and a terse ``reason`` naming the first closed gate.
    Every gate is a cheap check (config read, ping, ``PATH`` lookup) — this never
    shells ``indexed`` itself, so it works whether or not the binary is installed.
    """
    hybrid_configured = config.search.hybrid
    collection = config.search.collection
    daemon_up = _daemon_up()
    indexed_binary_available = indexed_client.indexed_available()

    reason: str | None = None
    if not hybrid_configured:
        reason = "hybrid disabled ([search].hybrid = false)"
    elif collection is None:
        reason = "no collection configured ([search].collection unset)"
    elif not daemon_up:
        reason = "daemon down"
    elif not indexed_binary_available:
        reason = "indexed binary not found on PATH"

    payload: dict[str, Any] = {
        "mode": "indexed" if reason is None else "fallback",
        "hybrid_configured": hybrid_configured,
        "collection": collection,
        "daemon_up": daemon_up,
        "indexed_binary_available": indexed_binary_available,
    }
    if reason is not None:
        payload["reason"] = reason
    return payload


def _iso_z(value: datetime) -> str:
    """Render a UTC-aware datetime with a ``Z`` suffix instead of ``+00:00``.

    Mirrors :func:`shards.schemas.note._iso_z` — kept as a local one-liner per
    the DRY-filter convention (root tech.md § Duplication) rather than a shared
    import across an unrelated module boundary.
    """
    text = value.isoformat()
    return f"{text[:-6]}Z" if text.endswith("+00:00") else text


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
        hit["updated"] = _iso_z(result.updated)
    if meta_only:
        return hit
    if full:
        hit["snippet"] = read_body(Path(result.path))
    elif result.snippet is not None:
        hit["snippet"] = result.snippet
    return hit
