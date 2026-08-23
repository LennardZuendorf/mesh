"""The client survives a daemon that is present but misbehaving.

"Daemon down" is the easy case — connect fails and the file-op fallback runs. The
hard case is a peer that is *up* and wrong: an older binary that rejects a method,
a wedged handler trickling bytes, a runaway reply. The contract is the same in all
of them — a read never fails because the accelerator did (`.spec/tech.md`
invariant 1) — and each of these tests pins one way that contract used to break.
"""

from __future__ import annotations

import json
import socket
import threading
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest

from shards.daemon.client import DaemonClient


@contextmanager
def rogue_peer(path: Path, respond: Callable[[socket.socket], None]) -> Iterator[None]:
    """Bind a real AF_UNIX socket that answers with `respond`, not the real daemon."""
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(str(path))
    server.listen(1)
    server.settimeout(5)
    stop = threading.Event()

    def serve() -> None:
        while not stop.is_set():
            try:
                conn, _ = server.accept()
            except OSError:
                return
            with conn:
                try:
                    conn.recv(4096)
                    respond(conn)
                except OSError:
                    return

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    try:
        yield
    finally:
        stop.set()
        server.close()
        thread.join(timeout=5)
        path.unlink(missing_ok=True)


def test_is_up_is_false_against_a_peer_that_rejects_ping(socket_path: Path) -> None:
    """An `ok: false` reply means "no accelerator here", not an exception.

    `is_up` documents "never raises" and gates both the search hybrid path and the
    recent-activity notice. An older daemon that does not know `ping` answers 404;
    that used to escape as an unmapped DaemonError and kill `shards search`.
    """

    def reject(conn: socket.socket) -> None:
        conn.sendall(
            json.dumps({"ok": False, "error": {"code": 404, "message": "unknown method"}}).encode()
            + b"\n"
        )

    with rogue_peer(socket_path, reject):
        assert DaemonClient(socket_path).is_up() is False


def test_a_slow_drip_peer_cannot_block_past_the_timeout(socket_path: Path) -> None:
    """The timeout is a deadline for the exchange, not for each recv.

    A per-recv timeout resets on every byte, so a peer trickling one byte at a time
    held the CLI forever — precisely the hung-daemon case the fallback exists for.
    """

    def drip(conn: socket.socket) -> None:
        for _ in range(200):
            try:
                conn.sendall(b" ")
            except OSError:
                return
            threading.Event().wait(0.05)

    with rogue_peer(socket_path, drip):
        client = DaemonClient(socket_path, timeout=0.5)
        done = threading.Event()
        result: list[BaseException | None] = []

        def call() -> None:
            try:
                client.ping()
                result.append(None)
            except BaseException as exc:  # noqa: BLE001 - recording, not handling
                result.append(exc)
            finally:
                done.set()

        threading.Thread(target=call, daemon=True).start()
        assert done.wait(timeout=10), "client blocked past its own deadline"
        assert isinstance(result[0], OSError)


def test_an_oversized_reply_is_abandoned(
    socket_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A runaway reply must not grow the client's buffer without bound."""
    from shards.daemon import client as client_module

    monkeypatch.setattr(client_module, "_MAX_REPLY_BYTES", 4096)

    def flood(conn: socket.socket) -> None:
        chunk = b"x" * 4096
        for _ in range(20):
            try:
                conn.sendall(chunk)
            except OSError:
                return

    with rogue_peer(socket_path, flood), pytest.raises(OSError):
        DaemonClient(socket_path, timeout=5).ping()


def test_is_up_survives_every_misbehaviour(socket_path: Path) -> None:
    """Whatever the peer does, the liveness probe answers a bool."""

    def garbage(conn: socket.socket) -> None:
        conn.sendall(b"not json at all\n")

    with rogue_peer(socket_path, garbage):
        assert DaemonClient(socket_path, timeout=1).is_up() is False


# --------------------------------------------------------------------------- #
# Server lifecycle                                                            #
# --------------------------------------------------------------------------- #


def test_a_second_daemon_refuses_to_steal_a_live_socket(socket_path: Path) -> None:
    """Starting over a serving daemon must fail, not orphan it.

    Unlinking the node unconditionally left the first daemon running with no
    listener: a second watchdog observer over the same vault, duplicate `indexed`
    updates per edit, two reconcilers racing — and invisible to `daemon status`,
    which reads the PID file rather than the socket.
    """
    from tests.daemon.conftest import running_daemon

    with running_daemon(socket_path):
        assert DaemonClient(socket_path).is_up() is True
        with pytest.raises(OSError, match="already serving"):
            second = _sync_start(socket_path)
            second()
        # The original is still the one serving.
        assert DaemonClient(socket_path).is_up() is True


def test_a_stale_socket_node_is_reclaimed(socket_path: Path) -> None:
    """A leftover node with nothing behind it is cleared, not treated as live."""
    from tests.daemon.conftest import running_daemon

    stale = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    stale.bind(str(socket_path))
    stale.close()  # bound then abandoned: the node exists, nothing listens
    assert socket_path.exists()

    with running_daemon(socket_path):
        assert DaemonClient(socket_path).is_up() is True


def _sync_start(path: Path) -> Callable[[], None]:
    """Return a callable that starts a second DaemonServer on `path` synchronously."""
    import asyncio as _asyncio

    from shards.daemon.server import DaemonServer

    def start() -> None:
        loop = _asyncio.new_event_loop()
        try:
            loop.run_until_complete(DaemonServer(path).start())
        finally:
            loop.close()

    return start
