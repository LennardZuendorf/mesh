"""Asyncio unix-socket server speaking NDJSON RPC.

The daemon is the warm accelerator behind the CLI and MCP surfaces. This module
owns the *transport* — a unix socket at ``$XDG_RUNTIME_DIR/shards.sock`` (mode
``0600``, owner-only, so no other user can connect) and the request/response
envelope — plus a dispatch table. Writes never reach this server: they bypass the
socket and go straight through ``core``/``storage`` (see
:mod:`shards.daemon.client`), so every write works with the daemon down.

**Envelope.** One JSON object per line. Request: ``{"id","method","params"}``.
Success echoes the id: ``{"id","ok":true,"result":{...}}``. Error omits it:
``{"ok":false,"error":{"code","message"}}``. Handlers return a result dict on
success and raise :class:`RpcError` to produce an error envelope, so the envelope
machinery lives in one place — later units claim a dispatch slot by registering a
handler, never by touching :meth:`DaemonServer._dispatch`.

**What the daemon serves.** The base table holds ``ping`` alone; unknown methods
answer ``404``. When the server is constructed **with a vault ``config``** it warms
a :class:`~shards.index.warm.VaultIndex` (via a
:class:`~shards.index.watcher.Watcher`) *before* the socket accepts a single
connection, then registers the reads that index can actually accelerate:

* ``activity.recent`` — the mtime-ordered lens (daemon/2);
* ``task.list`` / ``note.list`` — the O(vault) walk + YAML parse per invocation
  that the index makes disappear;
* ``vault.status`` — counts and freshness off the index (dangling links and stale
  locks still touch disk: bodies are not indexed, and locks are not vault
  Markdown);
* ``search.tag_pull`` — a pure frontmatter filter over the corpus the index holds
  in full, foreign files included.

Point reads (``note.get`` / ``task.get``), ``search.query`` and ``index.reindex``
are deliberately **absent**: the id already names the file for a point read, the
index holds no bodies, and ranking/rebuilding live in the ``indexed`` subprocess —
so none of the three gets faster for crossing a socket. Nothing in the table
answers ``503``: a handler is either wired or the method is unknown.

Every wired read has a working file-op fallback on the client side, so a
config-less server (used by the transport tests), an older daemon that predates a
method, or no daemon at all all degrade to the identical on-disk answer — the
client treats every server-state code (``404``/``500``/``503``) on those verbs as
fallback-eligible. The daemon accelerates; it never gates.

The config-ful startup also registers the search feature's
:func:`~shards.index.indexed_client.incremental_update` on the server-owned
:class:`~shards.index.watcher.ChangeHooks` registry, so every vault edit
re-indexes that file in ``indexed``. A config-less server registers no hook, so
the watcher is never a hard dependency.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
from collections.abc import Callable
from pathlib import Path
from typing import Any

import msgspec

from shards.core.lenses import status_report
from shards.core.notes import MetaRow, NoteFilter, select_notes
from shards.core.tasks import TaskFilter, select_tasks
from shards.daemon.client import default_socket_path, entity_row
from shards.index.indexed_client import incremental_update
from shards.index.tagpull import TagPullFilter, select_tagpull
from shards.index.warm import DEFAULT_RECENT_LIMIT, IndexEntry, VaultIndex
from shards.index.watcher import ChangeHooks, Watcher
from shards.schemas.config import Config, load_config

Handler = Callable[[dict[str, Any]], dict[str, Any]]

_SOCKET_MODE = 0o600
_RUN_DIR_MODE = 0o700


class RpcError(Exception):
    """A handler-signalled error, rendered into an ``ok: false`` envelope.

    The declared way for a handler to answer with a code rather than a result.
    No handler raises one today — the warm reads either answer or let
    :meth:`DaemonServer._dispatch` wrap an unexpected failure as ``500`` — but it
    is the envelope's extension point, not a leftover stub: the ``503`` stub
    *table* is what core-hardening/5 culled.
    """

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


def _ping(_params: dict[str, Any]) -> dict[str, Any]:
    return {"pong": True}


def default_dispatch() -> dict[str, Handler]:
    """The transport-only dispatch table: ``ping``.

    Everything else is registered at config-ful startup, bound to the warm index
    (see :meth:`DaemonServer.start`). A method that is not in the table answers
    ``404``, which the client treats as fallback-eligible for the wired read
    verbs — so a config-less server degrades instead of failing.
    """
    return {"ping": _ping}


def _meta_rows(entries: list[IndexEntry]) -> list[MetaRow]:
    """Project index entries into the shared selector's ``(path, frontmatter)`` rows."""
    return [(entry.path, entry.meta) for entry in entries]


def _error_envelope(code: int, message: str) -> dict[str, Any]:
    """An error reply — no ``id`` field, by contract."""
    return {"ok": False, "error": {"code": code, "message": message}}


class DaemonServer:
    """Serves NDJSON RPC over a ``0600`` unix socket."""

    def __init__(
        self,
        socket_path: Path,
        handlers: dict[str, Handler] | None = None,
        *,
        config: Config | None = None,
    ) -> None:
        self.socket_path = Path(socket_path)
        self._config = config
        self._handlers = handlers if handlers is not None else default_dispatch()
        self._server: asyncio.AbstractServer | None = None
        self._index: VaultIndex | None = None
        self._watcher: Watcher | None = None
        self._hooks: ChangeHooks | None = None

    async def start(self) -> None:
        """Bind the unix socket and lock it down to owner-only (``0600``).

        When a vault ``config`` was supplied, warm the index and start the watcher
        *first*, so the index is hot before the socket accepts a single connection,
        and register the warm read handlers over it.
        """
        # Parent dir 0700 first: even if the socket is briefly group/other-readable
        # between bind and chmod, the enclosing dir already blocks other users.
        self.socket_path.parent.mkdir(parents=True, exist_ok=True, mode=_RUN_DIR_MODE)
        if self._config is not None and self._watcher is None:
            config = self._config  # local binding: ty narrows it to Config for the closure
            self._index = VaultIndex()
            # The daemon owns the change-hook registry; a fresh instance per start
            # means a restart (stop→start) can never stack a second hook that would
            # double-index every edit.
            self._hooks = ChangeHooks()
            self._watcher = Watcher(config, self._index, self._hooks)
            self._watcher.start()  # warm scan + observer, before we bind the socket
            # Freshness: every watcher-driven vault change re-indexes that one path in
            # ``indexed``. ``incremental_update`` swallows its own failures, so a dead
            # ``indexed`` never crashes the observer thread this hook runs on.
            self._hooks.register(lambda p: incremental_update(config, p))
            self._handlers = {**self._handlers, **self._warm_handlers()}
        with contextlib.suppress(FileNotFoundError):
            self.socket_path.unlink()  # clear a stale socket from a prior run
        # Bind under a restrictive umask so the socket node is created 0600 *at
        # bind time* — closing the TOCTOU window between bind and chmod. The chmod
        # below stays as defense-in-depth (and normalizes any inherited bits).
        old_umask = os.umask(0o177)
        try:
            self._server = await asyncio.start_unix_server(
                self._handle_client, path=str(self.socket_path)
            )
        finally:
            os.umask(old_umask)
        os.chmod(self.socket_path, _SOCKET_MODE)

    def _warm_handlers(self) -> dict[str, Handler]:
        """Every read handler this server can serve from its warm index.

        Each one is the *same* selector the client's file-op fallback runs, fed
        index rows instead of disk rows — the filtering, sorting and limiting is
        never reimplemented against the index, which is exactly the copy-drift the
        warm path exists to remove. Called once at config-ful startup, after the
        index is warm and before the socket binds.
        """
        index = self._index
        config = self._config
        assert index is not None
        assert config is not None

        def activity_recent(params: dict[str, Any]) -> dict[str, Any]:
            limit = params.get("limit")
            # ``bool`` is an ``int`` subclass — exclude it so a stray ``true`` never
            # silently limits to 1.
            if isinstance(limit, int) and not isinstance(limit, bool):
                n = limit
            else:
                n = DEFAULT_RECENT_LIMIT
            return {"entries": index.recent(n)}

        def note_list(params: dict[str, Any]) -> dict[str, Any]:
            views = select_notes(_meta_rows(index.entries()), NoteFilter.from_params(params))
            return {"entries": [entity_row(v.note, v.path) for v in views]}

        def task_list(params: dict[str, Any]) -> dict[str, Any]:
            views = select_tasks(_meta_rows(index.entries()), TaskFilter.from_params(params))
            return {"entries": [entity_row(v.task, v.path) for v in views]}

        def vault_status(_params: dict[str, Any]) -> dict[str, Any]:
            rows = _meta_rows(index.entries())
            return status_report(
                config,
                notes=select_notes(rows, NoteFilter(limit=None)),
                tasks=select_tasks(rows, TaskFilter(limit=None)),
                newest=index.recent(1),
            )

        def tag_pull(params: dict[str, Any]) -> dict[str, Any]:
            # The wider corpus: tag pull covers coexisting foreign Markdown too,
            # which surfaces with ``id: None`` exactly as the disk scan returns it.
            rows = [(entry.path, entry.meta) for entry in index.corpus()]
            hits = select_tagpull(rows, TagPullFilter.from_params(params))
            return {"results": [msgspec.to_builtins(hit) for hit in hits]}

        return {
            "activity.recent": activity_recent,
            "note.list": note_list,
            "task.list": task_list,
            "vault.status": vault_status,
            "search.tag_pull": tag_pull,
        }

    async def serve_forever(self) -> None:
        """Bind (if needed) and serve until cancelled; always unlinks on exit."""
        if self._server is None:
            await self.start()
        server = self._server
        assert server is not None
        try:
            await server.serve_forever()
        finally:
            await self.stop()

    async def stop(self) -> None:
        """Close the listener, join the watcher thread, flush the index, unlink."""
        if self._server is not None:
            self._server.close()
            with contextlib.suppress(Exception):
                await self._server.wait_closed()
            self._server = None
        if self._watcher is not None:
            self._watcher.stop()  # stop+join the observer thread, then clear the index
            self._watcher = None
            # Drop our change-hook so it can't outlive the index it re-indexed into.
            if self._hooks is not None:
                self._hooks.clear()
                self._hooks = None
        with contextlib.suppress(FileNotFoundError):
            self.socket_path.unlink()

    async def _handle_client(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        """Serve NDJSON requests on one connection until EOF."""
        try:
            while True:
                line = await reader.readline()
                if not line:
                    break
                response = self._dispatch(line)
                writer.write((json.dumps(response) + "\n").encode("utf-8"))
                await writer.drain()
        except (ConnectionResetError, BrokenPipeError):
            pass
        finally:
            writer.close()
            with contextlib.suppress(Exception):
                await writer.wait_closed()

    def _dispatch(self, line: bytes) -> dict[str, Any]:
        """Parse one request line and route it, returning a reply envelope."""
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            return _error_envelope(400, "malformed request")
        if not isinstance(request, dict):
            return _error_envelope(400, "malformed request")

        method = request.get("method")
        params = request.get("params")
        if not isinstance(params, dict):
            params = {}

        handler = self._handlers.get(method) if isinstance(method, str) else None
        if handler is None:
            return _error_envelope(404, "unknown method")
        try:
            result = handler(params)
        except RpcError as exc:
            return _error_envelope(exc.code, exc.message)
        except Exception:
            # A handler that fails for any other reason must still answer with a
            # structured envelope — never drop the connection unanswered.
            return _error_envelope(500, "internal error")
        return {"id": request.get("id"), "ok": True, "result": result}


def serve(socket_path: Path | None = None) -> None:  # pragma: no cover - process entry
    """Blocking entry point: run the daemon (with a warm watcher) until interrupted."""
    path = Path(socket_path) if socket_path is not None else default_socket_path()
    server = DaemonServer(path, config=load_config())
    with contextlib.suppress(KeyboardInterrupt):
        asyncio.run(server.serve_forever())
