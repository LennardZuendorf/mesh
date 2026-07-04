"""daemon/1 — socket + fallback: NDJSON RPC envelope and daemon-down fallbacks.

Two halves, matching the unit's acceptance criteria:

* **Server** — an ``asyncio`` unix socket at ``$XDG_RUNTIME_DIR/brain.sock`` (mode
  ``0600``) speaking NDJSON. ``ping`` returns ``{"pong": true}``; unknown methods
  yield a ``404`` error; the five not-yet-built methods yield a ``503`` stub. Error
  envelopes carry **no** ``id`` field (only success echoes the request id). These
  are exercised end-to-end over a real blocking socket against a daemon run in a
  background event-loop thread.
* **Client** — :class:`~brain.daemon.client.DaemonClient` connects, and on a
  connection failure (``ConnectionRefusedError`` / ``FileNotFoundError``) invokes
  the caller's fallback instead of raising. ``note.get`` / ``task.get`` fall back
  to a single-file ``python-frontmatter`` read; ``note.list`` / ``task.list`` fall
  back to a recursive scan returning only brain-id-bearing files. Writes never go
  through the client at all — there is no write method on it.

Sockets live under a short ``/tmp`` dir because ``AF_UNIX`` paths are capped near
104 bytes and ``tmp_path`` is too long; the real default paths are short.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import shutil
import socket
import stat
import tempfile
import threading
from collections.abc import Iterator
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest

from brain.daemon.client import DaemonClient, DaemonError, default_socket_path
from brain.daemon.server import DaemonServer
from brain.schemas.config import Config, load_config
from brain.storage.files import note_folder, task_folder

_STUB_METHODS = (
    "search.query",
    "search.tag_pull",
    "activity.recent",
    "vault.status",
    "index.reindex",
)


# --------------------------------------------------------------------------- #
# Fixtures & helpers                                                          #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg(brain_config: Path) -> Config:
    return load_config()


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


def _seed_note(
    vault: Path,
    *,
    note_id: str,
    note_type: str = "note",
    title: str = "A Note",
    tags: list[str] | None = None,
    owner: str = "seed-agent",
    body: str = "Body line.",
) -> Path:
    when = datetime.now(UTC)
    meta: dict[str, object] = {
        "id": note_id,
        "type": note_type,
        "title": title,
        "tags": list(tags or []),
        "owner": owner,
        "created": when,
        "updated": when,
        "related": [],
    }
    folder = note_folder(note_type, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{note_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


def _seed_task(
    vault: Path,
    *,
    task_id: str,
    title: str = "Seed Task",
    status: str = "open",
    owner: str = "seed-agent",
    body: str = "Task body.",
) -> Path:
    when = datetime.now(UTC)
    meta: dict[str, object] = {
        "id": task_id,
        "type": "task",
        "title": title,
        "tags": [],
        "owner": owner,
        "created": when,
        "updated": when,
        "related": [],
        "status": status,
        "priority": None,
        "claimed_by": None,
        "blocks": [],
        "blocked_by": [],
    }
    folder = task_folder(status, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


@contextlib.contextmanager
def running_daemon(path: Path) -> Iterator[None]:
    """Run a :class:`DaemonServer` on its own event loop in a daemon thread."""
    loop = asyncio.new_event_loop()
    stop_future: asyncio.Future[None] = loop.create_future()
    server = DaemonServer(path)
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
        yield
    finally:
        loop.call_soon_threadsafe(stop_future.set_result, None)
        thread.join(timeout=5)


def _roundtrip(path: Path, request: dict[str, object]) -> dict[str, object]:
    """Send one NDJSON request over a raw blocking socket, return the reply."""
    payload = (json.dumps(request) + "\n").encode("utf-8")
    buf = bytearray()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(5)
        sock.connect(str(path))
        sock.sendall(payload)
        while b"\n" not in buf:
            chunk = sock.recv(4096)
            if not chunk:
                break
            buf.extend(chunk)
    line = bytes(buf).split(b"\n", 1)[0]
    return json.loads(line)  # type: ignore[no-any-return]


# --------------------------------------------------------------------------- #
# Socket path resolution                                                      #
# --------------------------------------------------------------------------- #


def test_default_socket_path_uses_xdg_runtime_dir(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    assert default_socket_path() == tmp_path / "brain.sock"


def test_default_socket_path_falls_back_to_brain_run(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("XDG_RUNTIME_DIR", raising=False)
    assert default_socket_path() == Path.home() / ".brain" / "run" / "brain.sock"


# --------------------------------------------------------------------------- #
# Server — socket creation, envelope, dispatch                                #
# --------------------------------------------------------------------------- #


def test_socket_created_with_mode_0600(socket_path: Path) -> None:
    with running_daemon(socket_path):
        assert socket_path.exists()
        info = os.stat(socket_path)
        assert stat.S_ISSOCK(info.st_mode)
        # 0600: owner rw only — no group/other bits, so no other user can connect.
        assert stat.S_IMODE(info.st_mode) == 0o600


def test_ping_returns_pong_with_echoed_id(socket_path: Path) -> None:
    with running_daemon(socket_path):
        resp = _roundtrip(socket_path, {"id": "req-1", "method": "ping", "params": {}})
    assert resp == {"id": "req-1", "ok": True, "result": {"pong": True}}


def test_unknown_method_returns_404_without_id(socket_path: Path) -> None:
    with running_daemon(socket_path):
        resp = _roundtrip(socket_path, {"id": "req-2", "method": "does.not.exist", "params": {}})
    assert resp == {"ok": False, "error": {"code": 404, "message": "unknown method"}}
    assert "id" not in resp


@pytest.mark.parametrize("method", _STUB_METHODS)
def test_stub_methods_return_503(socket_path: Path, method: str) -> None:
    with running_daemon(socket_path):
        resp = _roundtrip(socket_path, {"id": "s", "method": method, "params": {}})
    assert resp == {"ok": False, "error": {"code": 503, "message": "not yet available"}}
    assert "id" not in resp


# --------------------------------------------------------------------------- #
# Client — talks to a live daemon, prefers socket over fallback               #
# --------------------------------------------------------------------------- #


def test_client_ping_against_running_daemon(socket_path: Path) -> None:
    with running_daemon(socket_path):
        client = DaemonClient(socket_path=socket_path)
        assert client.ping() == {"pong": True}


def test_call_prefers_socket_when_daemon_up(socket_path: Path) -> None:
    with running_daemon(socket_path):
        client = DaemonClient(socket_path=socket_path)
        result = client.call("ping", {}, lambda: {"pong": "FALLBACK"})
    assert result == {"pong": True}


# --------------------------------------------------------------------------- #
# Client — connection failure falls back, never re-raises                     #
# --------------------------------------------------------------------------- #


def test_call_falls_back_on_connection_refused(monkeypatch: pytest.MonkeyPatch) -> None:
    client = DaemonClient(socket_path=Path("/nonexistent/brain.sock"))

    def boom(method: str, params: dict[str, object]) -> object:
        raise ConnectionRefusedError

    monkeypatch.setattr(client, "_request", boom)
    assert client.call("note.get", {}, lambda: "SENTINEL") == "SENTINEL"


def test_call_falls_back_on_file_not_found(monkeypatch: pytest.MonkeyPatch) -> None:
    client = DaemonClient(socket_path=Path("/nonexistent/brain.sock"))

    def boom(method: str, params: dict[str, object]) -> object:
        raise FileNotFoundError

    monkeypatch.setattr(client, "_request", boom)
    assert client.call("note.get", {}, lambda: "SENTINEL") == "SENTINEL"


def test_call_end_to_end_fallback_on_missing_socket(missing_socket: Path) -> None:
    client = DaemonClient(socket_path=missing_socket)
    assert client.call("whatever", {}, lambda: "E2E") == "E2E"


def test_ping_has_no_fallback_and_propagates_when_down(missing_socket: Path) -> None:
    client = DaemonClient(socket_path=missing_socket)
    with pytest.raises((FileNotFoundError, ConnectionRefusedError)):
        client.ping()


# --------------------------------------------------------------------------- #
# Client — note.get / note.list / task.get / task.list fallbacks              #
# --------------------------------------------------------------------------- #


def test_note_get_falls_back_to_single_file_read(
    cfg: Config, vault: Path, missing_socket: Path
) -> None:
    _seed_note(vault, note_id="n-fall", title="Fallback Note", body="hello there")
    client = DaemonClient(socket_path=missing_socket)
    result = client.note_get(cfg, "n-fall")
    assert result["id"] == "n-fall"
    assert result["title"] == "Fallback Note"
    assert result["body"] == "hello there"
    assert str(result["path"]).endswith("n-fall.md")


def test_note_list_falls_back_to_recursive_id_scan(
    cfg: Config, vault: Path, missing_socket: Path
) -> None:
    _seed_note(vault, note_id="n-a", title="A")
    _seed_note(vault, note_id="n-b", title="B", note_type="decision")
    # A foreign Tolaria file (no brain id) must be excluded from the scan.
    foreign = vault / "notes" / "tolaria.md"
    foreign.write_text(frontmatter.dumps(frontmatter.Post("x", title="Tolaria")), encoding="utf-8")
    client = DaemonClient(socket_path=missing_socket)
    results = client.note_list(cfg)
    assert {r["id"] for r in results} == {"n-a", "n-b"}


def test_task_get_falls_back_to_single_file_read(
    cfg: Config, vault: Path, missing_socket: Path
) -> None:
    _seed_task(vault, task_id="t-fall", title="Fallback Task", body="do the thing")
    client = DaemonClient(socket_path=missing_socket)
    result = client.task_get(cfg, "t-fall")
    assert result["id"] == "t-fall"
    assert result["status"] == "open"
    assert result["body"] == "do the thing"


def test_task_list_falls_back_scanning_open_and_done(
    cfg: Config, vault: Path, missing_socket: Path
) -> None:
    _seed_task(vault, task_id="t-open", status="open")
    _seed_task(vault, task_id="t-done", status="done")
    client = DaemonClient(socket_path=missing_socket)
    ids = {r["id"] for r in client.task_list(cfg)}
    assert ids == {"t-open", "t-done"}


# --------------------------------------------------------------------------- #
# Client — writes bypass the socket entirely                                  #
# --------------------------------------------------------------------------- #


def test_client_exposes_no_write_methods() -> None:
    forbidden = {
        "create_note",
        "create_task",
        "atomic_write",
        "write",
        "append_note",
        "update_note",
        "finish_task",
        "cancel_task",
        "claim_task",
    }
    attrs = set(dir(DaemonClient))
    assert forbidden.isdisjoint(attrs)
    public = {a for a in attrs if not a.startswith("_")}
    assert not any("create" in a or "write" in a for a in public)


# --------------------------------------------------------------------------- #
# FIX 1 — call() swallows every transport failure, not just connection refused #
# --------------------------------------------------------------------------- #


def test_call_falls_back_on_timeout(monkeypatch: pytest.MonkeyPatch) -> None:
    """A hung daemon raises TimeoutError (an OSError) — must fall back, not escape."""
    client = DaemonClient(socket_path=Path("/nonexistent/brain.sock"))

    def boom(method: str, params: dict[str, object]) -> object:
        raise TimeoutError

    monkeypatch.setattr(client, "_request", boom)
    assert client.call("note.get", {}, lambda: "TIMED-OUT") == "TIMED-OUT"


def test_call_falls_back_on_broken_pipe(monkeypatch: pytest.MonkeyPatch) -> None:
    """A mid-response crash (BrokenPipeError, an OSError) must fall back."""
    client = DaemonClient(socket_path=Path("/nonexistent/brain.sock"))

    def boom(method: str, params: dict[str, object]) -> object:
        raise BrokenPipeError

    monkeypatch.setattr(client, "_request", boom)
    assert client.call("note.get", {}, lambda: "BROKEN") == "BROKEN"


def test_call_falls_back_on_truncated_reply(monkeypatch: pytest.MonkeyPatch) -> None:
    """A truncated/garbled reply raises json.JSONDecodeError — must fall back."""
    client = DaemonClient(socket_path=Path("/nonexistent/brain.sock"))

    def boom(method: str, params: dict[str, object]) -> object:
        raise json.JSONDecodeError("Expecting value", "", 0)

    monkeypatch.setattr(client, "_request", boom)
    assert client.call("note.get", {}, lambda: "TRUNCATED") == "TRUNCATED"


# --------------------------------------------------------------------------- #
# FIX 3 — _dispatch wraps any non-RpcError handler failure in a 500 envelope   #
# --------------------------------------------------------------------------- #


def test_dispatch_wraps_handler_exception_in_500(socket_path: Path) -> None:
    """A handler raising a plain exception yields a structured 500, not a drop."""

    def _explode(_params: dict[str, object]) -> dict[str, object]:
        raise ValueError("kaboom")

    server = DaemonServer(socket_path, handlers={"boom": _explode})
    line = (json.dumps({"id": "x", "method": "boom", "params": {}}) + "\n").encode("utf-8")
    reply = server._dispatch(line)
    assert reply == {"ok": False, "error": {"code": 500, "message": "internal error"}}
    assert "id" not in reply  # error envelopes never echo the id


# --------------------------------------------------------------------------- #
# FIX 4 — reserved read verbs are 503 stubs; the client falls back on 503      #
# --------------------------------------------------------------------------- #


@pytest.mark.parametrize("method", ["note.get", "note.list", "task.get", "task.list"])
def test_read_methods_are_503_stubs(socket_path: Path, method: str) -> None:
    """note/task reads are reserved-but-unwired → 503 (fallback-eligible), not 404."""
    with running_daemon(socket_path):
        resp = _roundtrip(socket_path, {"id": "r", "method": method, "params": {}})
    assert resp == {"ok": False, "error": {"code": 503, "message": "not yet available"}}


def test_note_get_falls_back_to_file_when_daemon_returns_503(
    cfg: Config, vault: Path, socket_path: Path
) -> None:
    """A live (config-less) daemon answers note.get with 503 → client reads the file."""
    _seed_note(vault, note_id="n-503", title="From Disk", body="disk body")
    with running_daemon(socket_path):
        client = DaemonClient(socket_path=socket_path)
        result = client.note_get(cfg, "n-503")
    assert result["id"] == "n-503"
    assert result["title"] == "From Disk"
    assert result["body"] == "disk body"


def test_task_get_falls_back_to_file_when_daemon_returns_503(
    cfg: Config, vault: Path, socket_path: Path
) -> None:
    _seed_task(vault, task_id="t-503", title="Disk Task", body="disk task body")
    with running_daemon(socket_path):
        client = DaemonClient(socket_path=socket_path)
        result = client.task_get(cfg, "t-503")
    assert result["id"] == "t-503"
    assert result["status"] == "open"
    assert result["body"] == "disk task body"


def test_call_falls_back_on_503_daemon_error(monkeypatch: pytest.MonkeyPatch) -> None:
    """A 503 DaemonError from a live daemon is fallback-eligible by default."""
    client = DaemonClient(socket_path=Path("/nonexistent/brain.sock"))

    def boom(method: str, params: dict[str, object]) -> object:
        raise DaemonError(503, "not yet available")

    monkeypatch.setattr(client, "_request", boom)
    assert client.call("note.get", {}, lambda: "FELL-BACK") == "FELL-BACK"


def test_call_propagates_non_fallback_domain_error(monkeypatch: pytest.MonkeyPatch) -> None:
    """A domain error (e.g. 3 not-found) must propagate — never be swallowed."""
    client = DaemonClient(socket_path=Path("/nonexistent/brain.sock"))
    ran: list[bool] = []

    def boom(method: str, params: dict[str, object]) -> object:
        raise DaemonError(3, "not found")

    def _fallback() -> object:
        ran.append(True)  # pragma: no cover - must never run
        return "SHOULD-NOT-RUN"

    monkeypatch.setattr(client, "_request", boom)
    with pytest.raises(DaemonError) as excinfo:
        client.call("note.get", {}, _fallback)
    assert excinfo.value.code == 3
    assert ran == []  # the fallback was not invoked for a domain error


# --------------------------------------------------------------------------- #
# FIX 5 — no TOCTOU on socket bind: umask makes the node 0600 at creation time  #
# --------------------------------------------------------------------------- #


def test_socket_umask_guard_yields_0600_without_chmod(
    socket_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """With chmod neutralized, the socket is *still* 0600 — proving the umask guard.

    A permissive ambient umask is forced first, so absent the guard the bind would
    create a world-accessible node and this assertion would fail.
    """

    def _no_chmod(*_a: object, **_k: object) -> None:
        return None

    monkeypatch.setattr("brain.daemon.server.os.chmod", _no_chmod)
    old_umask = os.umask(0o000)  # most permissive → bind would yield 0777 without the guard
    server = DaemonServer(socket_path)
    loop = asyncio.new_event_loop()
    try:
        loop.run_until_complete(server.start())
        assert stat.S_IMODE(os.stat(socket_path).st_mode) == 0o600
    finally:
        os.umask(old_umask)
        loop.run_until_complete(server.stop())
        loop.close()
