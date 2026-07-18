"""Recent-activity lens — a time-ordered view over the one Markdown folder.

:func:`recent_activity` is Phase-2's read-only "what changed lately" primitive,
shared by the ``shards recent-activity`` CLI command and (later) the
``shards_recent_activity`` MCP tool. It is a *lens*, not a store: the rows come
from the daemon's warm frontmatter index when it is up, and from a direct
mtime-sorted directory scan when it is down — the daemon is an accelerator, never
a gate.

The delegation is deliberately thin: it calls
:meth:`shards.daemon.client.DaemonClient.activity_recent`, whose own fallback
contract already routes to :func:`shards.index.warm.scan_recent` on a socket-down
error. So this module inherits the daemon-up/daemon-down behaviour for free and
never speaks to the socket itself.

Each activity row is the minimal JSON-serializable shape
``{id, type, title, path, mtime}`` — note there is **no** ``owner`` key. The
``--owner`` / ``--mine`` filters therefore re-read each candidate's frontmatter
from disk (owner + ``claimed_by``), and ``--since`` is applied as an *mtime*
cutoff on those same rows. When any filter is active the fetch is unbounded and
``limit`` is applied last, as a display cap — so a filter is a true filter, not
something the daemon's own row cap can starve.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from shards.core.notes import _parse_since
from shards.daemon.client import DaemonClient, DaemonError
from shards.index.warm import scan_recent
from shards.schemas.config import Config
from shards.storage.files import read_post

__all__ = ["recent_activity"]


def recent_activity(
    config: Config,
    *,
    since: str | None,
    owner: str | None,
    mine: bool,
    limit: int,
) -> list[dict[str, Any]]:
    """Return recent vault changes as ``{id, type, title, path, mtime}`` rows.

    Delegates to :meth:`DaemonClient.activity_recent` (which scans on a daemon-down
    socket error), newest-first. ``since`` (``7d`` / ``12h`` / ISO) is applied as an
    mtime cutoff; ``owner`` and ``mine`` re-read each row's frontmatter (the row
    carries no owner) — ``mine`` keeps rows whose ``owner`` **or** ``claimed_by``
    equals ``config.agent``. When any filter is active the fetch is unbounded and
    ``limit`` is applied afterwards as a display cap (``limit < 0`` → uncapped);
    with no filter, ``limit`` is forwarded to the fetch as-is.

    Raises ``ValueError`` on an unparseable ``since`` (the CLI maps it to exit 2).
    """
    cutoff = _parse_since(since).timestamp() if since else None
    filtered = cutoff is not None or owner is not None or mine

    fetch_limit = -1 if filtered else limit
    try:
        result = DaemonClient().activity_recent(config, fetch_limit)
        entries: list[dict[str, Any]] = (
            list(result.get("entries", [])) if isinstance(result, dict) else []
        )
    except DaemonError:
        # The daemon is up but ``activity.recent`` is unavailable — a config-less
        # 503 stub, or a 500 from its warm-index handler. The accelerator is never a
        # gate, so scan the folder directly for the same rows the client's own
        # socket-down fallback returns, rather than surfacing a traceback.
        entries = scan_recent(config, fetch_limit)

    if cutoff is not None:
        entries = [e for e in entries if _mtime(e) >= cutoff]
    if owner is not None or mine:
        entries = [e for e in entries if _owner_match(config, e, owner=owner, mine=mine)]

    if filtered and limit >= 0:
        entries = entries[:limit]
    return entries


def _mtime(entry: dict[str, Any]) -> float:
    """The row's mtime as a float (``0.0`` if missing/unparseable — sorts oldest)."""
    try:
        return float(entry.get("mtime", 0.0))
    except (TypeError, ValueError):
        return 0.0


def _read_meta(path: str) -> dict[str, Any] | None:
    """Frontmatter metadata at ``path``, or ``None`` on a read/parse failure."""
    post = read_post(Path(path))
    return dict(post.metadata) if post is not None else None


def _owner_match(config: Config, entry: dict[str, Any], *, owner: str | None, mine: bool) -> bool:
    """Whether ``entry`` passes the ``owner`` / ``mine`` filters (via re-read frontmatter).

    The activity row has no ``owner`` key, so ownership is read fresh from the
    file. A row whose file cannot be read/parsed fails an active owner/mine filter.
    """
    meta = _read_meta(str(entry.get("path", "")))
    if meta is None:
        return False
    if owner is not None and meta.get("owner") != owner:
        return False
    if mine:
        me = config.agent
        if meta.get("owner") != me and meta.get("claimed_by") != me:
            return False
    return True
