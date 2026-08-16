"""core-hardening/6 — daemon lifecycle: ``serve_forever``, stop idempotency,
reconnect after a clean stop, and the transport edges ``_handle_client``/
``_dispatch`` exist to swallow.

Before this unit ``serve_forever`` was never invoked by any test (every daemon
test runs the server through ``start()`` + manual ``stop()``, or through the
shared ``running_daemon`` harness, neither of which calls the real blocking
entry point). "A daemon killed mid-request leaves the client on its fallback"
is already covered — ``tests/daemon/test_warm_reads.py``'s
``test_every_wired_read_degrades_on_a_broken_daemon[killed]`` drops a
connection with zero bytes sent back, which is exactly that scenario from the
client's side — so it is not duplicated here. This file adds what was still
open: the real ``serve_forever`` coroutine actually running and unwinding
cleanly on cancellation, a clean stop→restart reconnect cycle, and the
defensive branches in ``_handle_client``/``_dispatch``/``stop`` that no
existing test reaches (a handler-raised ``RpcError``, a malformed/non-dict
request or params, a write racing a reset connection, and ``stop()`` called on
a server that was never started or already stopped).
"""

from __future__ import annotations

import asyncio
import contextlib
import json
from pathlib import Path

import pytest

from shards.daemon.client import DaemonClient
from shards.daemon.server import DaemonServer, RpcError
from shards.schemas.config import Config, load_config
from tests.daemon.conftest import running_daemon


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


# --------------------------------------------------------------------------- #
# serve_forever — the real blocking entry point, run and cancelled cleanly    #
# --------------------------------------------------------------------------- #


def test_serve_forever_serves_and_unlinks_socket_on_cancel(socket_path: Path) -> None:
    """``serve_forever`` binds, actually answers a real request, and on
    cancellation its ``finally: await self.stop()`` unlinks the socket."""

    async def _run() -> None:
        server = DaemonServer(socket_path)
        task = asyncio.ensure_future(server.serve_forever())
        try:
            for _ in range(100):
                if socket_path.exists():
                    break
                await asyncio.sleep(0.02)
            assert socket_path.exists(), "serve_forever never bound the socket"

            reader, writer = await asyncio.open_unix_connection(str(socket_path))
            writer.write((json.dumps({"id": "1", "method": "ping", "params": {}}) + "\n").encode())
            await writer.drain()
            line = await asyncio.wait_for(reader.readline(), timeout=5)
            writer.close()
            with contextlib.suppress(Exception):
                await writer.wait_closed()
            assert json.loads(line) == {"id": "1", "ok": True, "result": {"pong": True}}
        finally:
            task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await task

    asyncio.run(_run())
    assert not socket_path.exists()  # stop()'s unlink ran, even on cancellation


def test_serve_forever_skips_start_when_already_started(socket_path: Path) -> None:
    """``serve_forever`` on a server that already called ``start()`` does not
    re-bind — it goes straight to serving the existing listener."""

    async def _run() -> None:
        server = DaemonServer(socket_path)
        await server.start()  # pre-started, unlike every other test in this file
        assert socket_path.exists()
        task = asyncio.ensure_future(server.serve_forever())
        try:
            reader, writer = await asyncio.open_unix_connection(str(socket_path))
            writer.write((json.dumps({"id": "1", "method": "ping", "params": {}}) + "\n").encode())
            await writer.drain()
            line = await asyncio.wait_for(reader.readline(), timeout=5)
            writer.close()
            with contextlib.suppress(Exception):
                await writer.wait_closed()
            assert json.loads(line) == {"id": "1", "ok": True, "result": {"pong": True}}
        finally:
            task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await task

    asyncio.run(_run())
    assert not socket_path.exists()


def test_serve_forever_binds_config_ful_warm_handlers(socket_path: Path, cfg: Config) -> None:
    """``serve_forever`` with a config actually registers the warm handlers —
    not just the transport-only ``ping`` table."""

    async def _run() -> dict[str, object]:
        server = DaemonServer(socket_path, config=cfg)
        task = asyncio.ensure_future(server.serve_forever())
        try:
            for _ in range(100):
                if socket_path.exists():
                    break
                await asyncio.sleep(0.02)
            reader, writer = await asyncio.open_unix_connection(str(socket_path))
            req = {"id": "1", "method": "vault.status", "params": {}}
            writer.write((json.dumps(req) + "\n").encode())
            await writer.drain()
            line = await asyncio.wait_for(reader.readline(), timeout=5)
            writer.close()
            with contextlib.suppress(Exception):
                await writer.wait_closed()
            return json.loads(line)  # type: ignore[no-any-return]
        finally:
            task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await task

    reply = asyncio.run(_run())
    assert reply["ok"] is True, reply
    result = reply["result"]
    assert isinstance(result, dict)
    assert set(result) == {"note_count", "task_statuses", "newest"}


# --------------------------------------------------------------------------- #
# Reconnect after a clean stop — distinct from a mid-request kill             #
# --------------------------------------------------------------------------- #


def test_client_reconnects_after_a_clean_stop_and_fresh_start(
    socket_path: Path, cfg: Config
) -> None:
    """A daemon that stops cleanly (socket unlinked) and a fresh instance that
    binds the same path afterward: the client is not left wedged on the old
    connection state — it reconnects to the new daemon transparently."""
    with running_daemon(socket_path, config=cfg):
        client = DaemonClient(socket_path=socket_path)
        assert client.ping() == {"pong": True}
    assert not socket_path.exists()  # the clean stop unlinked it

    # A brand-new server instance binds the identical path -- no stale-socket
    # residue from the previous run blocks the rebind or the reconnect.
    with running_daemon(socket_path, config=cfg):
        client2 = DaemonClient(socket_path=socket_path)
        assert client2.ping() == {"pong": True}
        assert client2.vault_status(cfg)["notes"] == 0
    assert not socket_path.exists()


# --------------------------------------------------------------------------- #
# RpcError — a handler-signalled error reaches its own envelope branch         #
# --------------------------------------------------------------------------- #


def test_rpcerror_carries_its_own_code_and_message() -> None:
    exc = RpcError(409, "custom conflict")
    assert exc.code == 409
    assert exc.message == "custom conflict"
    assert str(exc) == "custom conflict"


def test_dispatch_maps_handler_raised_rpcerror_to_its_own_envelope(socket_path: Path) -> None:
    def _explode(_params: dict[str, object]) -> dict[str, object]:
        raise RpcError(409, "custom conflict")

    server = DaemonServer(socket_path, handlers={"boom": _explode})
    line = (json.dumps({"id": "x", "method": "boom", "params": {}}) + "\n").encode("utf-8")
    reply = server._dispatch(line)
    assert reply == {"ok": False, "error": {"code": 409, "message": "custom conflict"}}
    assert "id" not in reply


# --------------------------------------------------------------------------- #
# _dispatch — malformed input never crashes the connection                    #
# --------------------------------------------------------------------------- #


def test_dispatch_malformed_json_returns_400(socket_path: Path) -> None:
    server = DaemonServer(socket_path)
    reply = server._dispatch(b"not-json-at-all\n")
    assert reply == {"ok": False, "error": {"code": 400, "message": "malformed request"}}


def test_dispatch_non_dict_request_returns_400(socket_path: Path) -> None:
    server = DaemonServer(socket_path)
    reply = server._dispatch(b'["not", "a", "dict"]\n')
    assert reply == {"ok": False, "error": {"code": 400, "message": "malformed request"}}


def test_dispatch_non_dict_params_coerces_to_empty(socket_path: Path) -> None:
    """Malformed ``params`` degrades to ``{}`` rather than crashing the handler."""
    server = DaemonServer(socket_path)
    req = {"id": "1", "method": "ping", "params": "not-a-dict"}
    reply = server._dispatch((json.dumps(req) + "\n").encode("utf-8"))
    assert reply == {"id": "1", "ok": True, "result": {"pong": True}}


# --------------------------------------------------------------------------- #
# _handle_client — a write racing a reset/broken connection is swallowed      #
# --------------------------------------------------------------------------- #


class _OneShotReader:
    """A minimal ``asyncio.StreamReader`` stand-in: one line, then EOF."""

    def __init__(self, line: bytes) -> None:
        self._lines = [line]

    async def readline(self) -> bytes:
        return self._lines.pop(0) if self._lines else b""


class _DyingWriter:
    """A ``StreamWriter`` stand-in whose ``write`` raises as if the peer reset
    the connection mid-response — the exact condition ``_handle_client``'s
    ``except (ConnectionResetError, BrokenPipeError)`` exists for."""

    def __init__(self, exc: type[Exception]) -> None:
        self._exc = exc
        self.closed = False

    def write(self, _data: bytes) -> None:
        raise self._exc

    async def drain(self) -> None:  # pragma: no cover - never reached, write raises first
        pass

    def close(self) -> None:
        self.closed = True

    async def wait_closed(self) -> None:
        pass


@pytest.mark.parametrize("exc_type", [ConnectionResetError, BrokenPipeError])
def test_handle_client_swallows_reset_while_writing_the_reply(
    socket_path: Path, exc_type: type[Exception]
) -> None:
    server = DaemonServer(socket_path)
    line = (json.dumps({"id": "1", "method": "ping", "params": {}}) + "\n").encode("utf-8")
    reader = _OneShotReader(line)
    writer = _DyingWriter(exc_type)

    asyncio.run(server._handle_client(reader, writer))  # ty: ignore[invalid-argument-type]

    assert writer.closed  # the connection is still cleaned up on the way out


# --------------------------------------------------------------------------- #
# stop() — never-started and idempotent-double-stop                          #
# --------------------------------------------------------------------------- #


def test_stop_on_never_started_server_is_a_noop(socket_path: Path) -> None:
    server = DaemonServer(socket_path)
    asyncio.run(server.stop())  # start() was never called
    assert not socket_path.exists()


def test_stop_is_idempotent(socket_path: Path, cfg: Config) -> None:
    async def _run() -> None:
        server = DaemonServer(socket_path, config=cfg)
        await server.start()
        await server.stop()
        await server.stop()  # second stop: must not raise or double-unlink-error

    asyncio.run(_run())
    assert not socket_path.exists()


def test_stop_tolerates_a_watcher_without_hooks(socket_path: Path) -> None:
    """Defensive-only: ``start()`` always sets ``_watcher``/``_hooks`` together,
    so a live watcher with no hooks is unreachable through the public
    ``start()``/``stop()`` cycle. Exercised directly against the private state
    to prove ``stop()`` degrades instead of raising ``AttributeError`` if that
    invariant is ever broken by a future change."""

    class _FakeWatcher:
        def __init__(self) -> None:
            self.stopped = False

        def stop(self) -> None:
            self.stopped = True

    async def _run() -> _FakeWatcher:
        server = DaemonServer(socket_path)
        watcher = _FakeWatcher()
        server._watcher = watcher  # ty: ignore[invalid-assignment]
        server._hooks = None
        await server.stop()
        assert server._watcher is None
        return watcher

    watcher = asyncio.run(_run())
    assert watcher.stopped
