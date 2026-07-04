"""Synchronous NDJSON client for the brain daemon, with file-op fallback.

The daemon is an *accelerator, never a gate*: every read has a daemon-down
fallback and every write bypasses the socket entirely (writes go straight through
``core``/``storage``, so no write method lives here). The client is deliberately
synchronous and ``asyncio``-free — a CLI invocation must feel instant, and the
warm work lives in the daemon, not at client startup.

:meth:`DaemonClient.call` connects to the unix socket, sends one NDJSON request
(``{"id","method","params"}``), and returns the ``result`` of the reply. When the
socket is unreachable — ``FileNotFoundError`` (no socket file) or
``ConnectionRefusedError`` (stale socket, no listener) — it invokes the caller's
fallback callable instead and returns its value; the connection failure is never
re-raised. :meth:`ping` is the one exception: it is a liveness probe with no
fallback, so a down daemon propagates.

The convenience read verbs (:meth:`note_get`, :meth:`note_list`, :meth:`task_get`,
:meth:`task_list`) bind the right fallback — a single-file ``python-frontmatter``
read for ``get``, a recursive brain-id scan for ``list`` — delegating to
``brain.core`` so the sandbox check and the ``n-``/``t-`` id gate are inherited,
not re-implemented. Their return shape is a JSON-serializable dict (or list of
dicts), matching what the socket will eventually return.
"""

from __future__ import annotations

import json
import os
import socket
import uuid
from collections.abc import Callable
from pathlib import Path
from typing import Any

from brain.core.notes import get_note, list_notes
from brain.core.tasks import get_task, list_tasks
from brain.index.watch import DEFAULT_RECENT_LIMIT, scan_recent
from brain.schemas.config import Config
from brain.schemas.note import Note

_SOCKET_NAME = "brain.sock"
_DEFAULT_TIMEOUT = 5.0
_RECV_CHUNK = 4096

Fallback = Callable[[], Any]


def default_socket_path() -> Path:
    """Resolve the daemon socket path.

    ``$XDG_RUNTIME_DIR/brain.sock`` when the runtime dir is set, else
    ``~/.brain/run/brain.sock``. An empty ``$XDG_RUNTIME_DIR`` counts as unset.
    """
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime:
        return Path(runtime) / _SOCKET_NAME
    return Path.home() / ".brain" / "run" / _SOCKET_NAME


class DaemonError(Exception):
    """A non-connection error the daemon reported (``ok: false`` envelope).

    Carries the RPC ``code`` and ``message`` so callers can map them to CLI exit
    codes. Distinct from ``ConnectionRefusedError`` / ``FileNotFoundError``, which
    signal the daemon is *down* and therefore trigger the fallback path instead.
    """

    def __init__(self, code: int, message: str) -> None:
        super().__init__(f"[{code}] {message}")
        self.code = code
        self.message = message


def _serialize(model: Note, body: str, path: Path) -> dict[str, Any]:
    """Render a note/task view as a JSON-safe dict: frontmatter + body + path."""
    data: dict[str, Any] = model.model_dump(mode="json")
    data["body"] = body
    data["path"] = str(path)
    return data


class DaemonClient:
    """A thin, synchronous NDJSON client over the daemon's unix socket."""

    def __init__(
        self,
        socket_path: Path | None = None,
        *,
        timeout: float = _DEFAULT_TIMEOUT,
    ) -> None:
        self._socket_path = Path(socket_path) if socket_path is not None else default_socket_path()
        self._timeout = timeout

    # -- transport --------------------------------------------------------- #

    def _request(self, method: str, params: dict[str, Any]) -> Any:
        """Send one NDJSON request and return the reply's ``result``.

        Raises ``ConnectionRefusedError`` / ``FileNotFoundError`` when the socket
        is unreachable (caught by :meth:`call`), and :class:`DaemonError` when the
        daemon answers with an ``ok: false`` envelope.
        """
        request = {"id": str(uuid.uuid4()), "method": method, "params": params}
        payload = (json.dumps(request) + "\n").encode("utf-8")
        buf = bytearray()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(self._timeout)
            sock.connect(str(self._socket_path))
            sock.sendall(payload)
            while b"\n" not in buf:
                chunk = sock.recv(_RECV_CHUNK)
                if not chunk:
                    break
                buf.extend(chunk)
        line = bytes(buf).split(b"\n", 1)[0]
        response: dict[str, Any] = json.loads(line)
        if not response.get("ok", False):
            error = response.get("error") or {}
            raise DaemonError(int(error.get("code", 1)), str(error.get("message", "error")))
        return response.get("result")

    def call(self, method: str, params: dict[str, Any], fallback: Fallback) -> Any:
        """Call ``method`` over the socket; run ``fallback`` if the daemon is down.

        A connection failure (``ConnectionRefusedError`` / ``FileNotFoundError``)
        is swallowed and the fallback's result returned — it is never re-raised.
        Any ``ok: false`` reply from a *live* daemon surfaces as
        :class:`DaemonError`.
        """
        try:
            return self._request(method, params)
        except (ConnectionRefusedError, FileNotFoundError):
            return fallback()

    # -- verbs ------------------------------------------------------------- #

    def ping(self) -> Any:
        """Liveness probe — no fallback; a down daemon propagates the error."""
        return self._request("ping", {})

    def note_get(self, config: Config, id_or_slug: str) -> Any:
        return self.call(
            "note.get",
            {"id": id_or_slug},
            lambda: _serialize(*_note_get_view(config, id_or_slug)),
        )

    def note_list(self, config: Config) -> Any:
        return self.call(
            "note.list",
            {},
            lambda: [_serialize(v.note, v.body, v.path) for v in list_notes(config, limit=None)],
        )

    def task_get(self, config: Config, task_id: str) -> Any:
        return self.call(
            "task.get",
            {"id": task_id},
            lambda: _serialize(*_task_get_view(config, task_id)),
        )

    def task_list(self, config: Config) -> Any:
        return self.call(
            "task.list",
            {},
            lambda: [_serialize(v.task, v.body, v.path) for v in list_tasks(config, limit=None)],
        )

    def activity_recent(self, config: Config, limit: int = DEFAULT_RECENT_LIMIT) -> Any:
        """Most-recently-modified entries; falls back to a dir scan when down.

        A live daemon answers from its warm in-process index; when the socket is
        unreachable the fallback runs :func:`brain.index.watch.scan_recent`, an
        mtime-sorted scan of ``notes/`` and ``tasks/``. Both return the same
        ``{"entries": [...]}`` shape.
        """
        return self.call(
            "activity.recent",
            {"limit": limit},
            lambda: {"entries": scan_recent(config, limit)},
        )


def _note_get_view(config: Config, id_or_slug: str) -> tuple[Note, str, Path]:
    view = get_note(config, id_or_slug)
    return view.note, view.body, view.path


def _task_get_view(config: Config, task_id: str) -> tuple[Note, str, Path]:
    view = get_task(config, task_id)
    return view.task, view.body, view.path
