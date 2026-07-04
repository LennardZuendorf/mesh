"""Asyncio unix-socket server speaking NDJSON RPC.

The daemon is the warm accelerator behind the CLI and MCP surfaces. This module
owns the *transport* — a unix socket at ``$XDG_RUNTIME_DIR/brain.sock`` (mode
``0600``, owner-only, so no other user can connect) and the request/response
envelope — plus a dispatch table. Writes never reach this server: they bypass the
socket and go straight through ``core``/``storage`` (see
:mod:`brain.daemon.client`), so every write works with the daemon down.

**Envelope.** One JSON object per line. Request: ``{"id","method","params"}``.
Success echoes the id: ``{"id","ok":true,"result":{...}}``. Error omits it:
``{"ok":false,"error":{"code","message"}}``. Handlers return a result dict on
success and raise :class:`RpcError` to produce an error envelope, so the envelope
machinery lives in one place — later units claim a dispatch slot by registering a
handler, never by touching :meth:`DaemonServer._dispatch`.

The base table serves ``ping`` and reserves the methods that depend on subsystems
not yet present (``search.*``, ``activity.recent``, ``vault.status``,
``index.reindex``) as ``503`` stubs; unknown methods answer ``404``. When the
server is constructed **with a vault ``config``**, daemon/2 warms a
:class:`~brain.index.watch.VaultIndex` (via a :class:`~brain.index.watch.Watcher`)
*before* the socket accepts connections and swaps the ``activity.recent`` stub for
a real handler served from that warm index. A config-less server (used by the
transport tests) keeps the ``503`` stub, so the watcher is never a hard dependency.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
from collections.abc import Callable
from pathlib import Path
from typing import Any

from brain.daemon.client import default_socket_path
from brain.index.watch import DEFAULT_RECENT_LIMIT, VaultIndex, Watcher
from brain.schemas.config import Config, load_config

Handler = Callable[[dict[str, Any]], dict[str, Any]]

_SOCKET_MODE = 0o600
_RUN_DIR_MODE = 0o700
_STUB_METHODS: tuple[str, ...] = (
    "search.query",
    "search.tag_pull",
    "activity.recent",
    "vault.status",
    "index.reindex",
)


class RpcError(Exception):
    """A handler-signalled error, rendered into an ``ok: false`` envelope."""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


def _ping(_params: dict[str, Any]) -> dict[str, Any]:
    return {"pong": True}


def _not_yet_available(_params: dict[str, Any]) -> dict[str, Any]:
    """Reserved dispatch slot: a later unit replaces this with a real handler."""
    raise RpcError(503, "not yet available")


def default_dispatch() -> dict[str, Handler]:
    """The unit's dispatch table: ``ping`` plus reserved ``503`` stubs."""
    table: dict[str, Handler] = {"ping": _ping}
    for method in _STUB_METHODS:
        table[method] = _not_yet_available
    return table


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

    async def start(self) -> None:
        """Bind the unix socket and lock it down to owner-only (``0600``).

        When a vault ``config`` was supplied, warm the index and start the watcher
        *first*, so the index is hot before the socket accepts a single connection,
        and register the real ``activity.recent`` handler over it.
        """
        # Parent dir 0700 first: even if the socket is briefly group/other-readable
        # between bind and chmod, the enclosing dir already blocks other users.
        self.socket_path.parent.mkdir(parents=True, exist_ok=True, mode=_RUN_DIR_MODE)
        if self._config is not None and self._watcher is None:
            self._index = VaultIndex()
            self._watcher = Watcher(self._config, self._index)
            self._watcher.start()  # warm scan + observer, before we bind the socket
            self._handlers = {**self._handlers, "activity.recent": self._activity_handler()}
        with contextlib.suppress(FileNotFoundError):
            self.socket_path.unlink()  # clear a stale socket from a prior run
        self._server = await asyncio.start_unix_server(
            self._handle_client, path=str(self.socket_path)
        )
        os.chmod(self.socket_path, _SOCKET_MODE)

    def _activity_handler(self) -> Handler:
        """A ``activity.recent`` handler bound to this server's warm index."""
        index = self._index
        assert index is not None

        def handler(params: dict[str, Any]) -> dict[str, Any]:
            limit = params.get("limit")
            # ``bool`` is an ``int`` subclass — exclude it so a stray ``true`` never
            # silently limits to 1.
            if isinstance(limit, int) and not isinstance(limit, bool):
                n = limit
            else:
                n = DEFAULT_RECENT_LIMIT
            return {"entries": index.recent(n)}

        return handler

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
        return {"id": request.get("id"), "ok": True, "result": result}


def serve(socket_path: Path | None = None) -> None:  # pragma: no cover - process entry
    """Blocking entry point: run the daemon (with a warm watcher) until interrupted."""
    path = Path(socket_path) if socket_path is not None else default_socket_path()
    server = DaemonServer(path, config=load_config())
    with contextlib.suppress(KeyboardInterrupt):
        asyncio.run(server.serve_forever())
