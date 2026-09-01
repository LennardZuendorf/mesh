"""Shared daemon-test scaffolding: unix-socket paths and a live server harness.

Three test modules in this package need a real :class:`~mesh.daemon.server.DaemonServer`
listening on a real ``AF_UNIX`` socket, so the harness lives here once rather than
being re-pasted per module (core-hardening/5 would otherwise have added a third
copy).

Sockets live under a short ``/tmp`` dir because ``AF_UNIX`` paths are capped near
104 bytes and ``tmp_path`` is too long; the real default paths are short.
"""

from __future__ import annotations

import asyncio
import contextlib
import shutil
import tempfile
import threading
from collections.abc import Iterator
from pathlib import Path

import pytest

from mesh.daemon.server import DaemonServer
from mesh.schemas.config import Config


@pytest.fixture
def sock_dir() -> Iterator[Path]:
    """A short-lived ``/tmp`` dir for unix sockets (AF_UNIX path-length limit)."""
    path = Path(tempfile.mkdtemp(prefix="brn-", dir="/tmp"))
    try:
        yield path
    finally:
        shutil.rmtree(path, ignore_errors=True)


@pytest.fixture
def socket_path(sock_dir: Path) -> Path:
    return sock_dir / "d.sock"


@pytest.fixture
def missing_socket(sock_dir: Path) -> Path:
    """A socket path guaranteed not to exist → connect raises FileNotFoundError."""
    return sock_dir / "absent.sock"


@contextlib.contextmanager
def running_daemon(path: Path, config: Config | None = None) -> Iterator[DaemonServer]:
    """Run a :class:`DaemonServer` on its own event loop in a daemon thread.

    With a ``config`` the server warms its index and registers the warm read
    handlers before the socket accepts anything; without one it serves the
    transport-only table (``ping``), which is what exercises the client's
    degrade-on-``404`` path.
    """
    loop = asyncio.new_event_loop()
    server = DaemonServer(path, config=config)
    stop_future: asyncio.Future[None] = loop.create_future()
    ready = threading.Event()
    start_error: list[BaseException] = []

    async def main() -> None:
        try:
            await server.start()
        except BaseException as exc:  # noqa: BLE001 - surfaced to the test thread
            start_error.append(exc)
            return
        finally:
            ready.set()
        await stop_future
        await server.stop()

    def run() -> None:
        asyncio.set_event_loop(loop)
        loop.run_until_complete(main())
        loop.close()

    thread = threading.Thread(target=run, daemon=True)
    thread.start()
    if not ready.wait(timeout=5):
        raise RuntimeError("daemon did not become ready")
    if start_error:
        thread.join(timeout=5)
        raise start_error[0]
    try:
        yield server
    finally:
        loop.call_soon_threadsafe(stop_future.set_result, None)
        thread.join(timeout=5)
