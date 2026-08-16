"""Synchronous NDJSON client for the shards daemon, with file-op fallback.

The daemon is an *accelerator, never a gate*: every read has a daemon-down
fallback and every write bypasses the socket entirely (writes go straight through
``core``/``storage``, so no write method lives here). The client is deliberately
synchronous and ``asyncio``-free — a CLI invocation must feel instant, and the
warm work lives in the daemon, not at client startup.

:meth:`DaemonClient.call` connects to the unix socket, sends one NDJSON request
(``{"id","method","params"}``), and returns the ``result`` of the reply. Any
transport failure — a missing socket, a refused/stale socket, a hung daemon
(timeout), a mid-response crash, or a truncated/garbled reply — is swallowed and
the caller's fallback is run instead; the failure is never re-raised. A *live*
daemon that answers ``ok: false`` surfaces as :class:`DaemonError`, except for the
fallback-eligible codes, which also run the fallback. :meth:`ping` is the one
exception: it is a liveness probe with no fallback, so a down daemon propagates.

**The wired read verbs.** :meth:`~DaemonClient.note_list`,
:meth:`~DaemonClient.task_list`, :meth:`~DaemonClient.vault_status` and
:meth:`~DaemonClient.tag_pull` are the list-shaped reads the daemon can actually
accelerate: each is an O(vault) walk plus a YAML parse per file over metadata the
warm :class:`~shards.index.warm.VaultIndex` already holds in RAM. Each ships a
*normalized filter spec* (built and validated on this side, so a bad ``--sort`` or
``--since`` fails identically with the daemon down) and each binds the identical
on-disk selector as its fallback — the same predicate the daemon applies to index
rows, never a second copy. Point reads (``note.get`` / ``task.get``) are
deliberately **not** here: the id already determines the path, so a point read is
one ``open()``, and since the index holds no bodies a warm ``get`` would hit disk
anyway — the socket round-trip would buy nothing.

**List rows carry no body.** The index holds frontmatter only, so the views these
verbs return have ``body=""`` on *both* the warm and the fallback path (parity is
the point). No list consumer reads a list row's body; a caller that needs one
reads it per id, which is what ``session-start`` does for its live queue.
"""

from __future__ import annotations

import json
import os
import socket
import uuid
from collections.abc import Callable
from pathlib import Path
from typing import Any

import msgspec

from shards.core.notes import NoteFilter, NoteView, list_notes
from shards.core.tasks import TaskFilter, TaskView, list_tasks
from shards.index.tagpull import TagPullFilter, tagpull
from shards.index.warm import DEFAULT_RECENT_LIMIT, scan_recent
from shards.schemas.config import Config
from shards.schemas.note import Note
from shards.schemas.search import SearchResult
from shards.schemas.task import Task

_SOCKET_NAME = "shards.sock"
_DEFAULT_TIMEOUT = 5.0
_RECV_CHUNK = 4096
# RPC error codes eligible for the daemon-down fallback even on a *live* daemon:
# ``503`` marks a method that is reserved but not yet wired to the daemon, so the
# CLI degrades to the file-op fallback. Domain errors (2/3/4) are never in here.
_FALLBACK_CODES: frozenset[int] = frozenset({503})
# The wired read verbs widen the fallback set to every *server-state* code:
# ``404`` (the daemon does not know this method — an older binary still running
# after an upgrade, or a config-less server with no warm index), ``500`` (its
# handler failed) and ``503``. All three mean "the daemon cannot serve this",
# which is precisely the condition the file-op fallback exists for; a read must
# never fail because the accelerator did. Domain codes (2/3/4) stay out — these
# handlers never raise one, and a genuine domain error must still propagate.
_READ_FALLBACK_CODES: frozenset[int] = frozenset({404, 500, 503})

Fallback = Callable[[], Any]

# Returned by the wired read verbs' inner fallback to mean "the daemon could not
# serve this" — a sentinel rather than ``None`` so it can never be confused with a
# legitimate ``null`` result off the wire.
_UNSERVED: Any = object()


def default_socket_path() -> Path:
    """Resolve the daemon socket path.

    ``$XDG_RUNTIME_DIR/shards.sock`` when the runtime dir is set, else
    ``~/.shards/run/shards.sock``. An empty ``$XDG_RUNTIME_DIR`` counts as unset.
    """
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if runtime:
        return Path(runtime) / _SOCKET_NAME
    return Path.home() / ".shards" / "run" / _SOCKET_NAME


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


# --------------------------------------------------------------------------- #
# Wire codec for list rows                                                     #
# --------------------------------------------------------------------------- #
#
# A list row is ``{"meta": <frontmatter dict>, "path": <str>}``. The frontmatter
# is kept *nested* rather than merged with ``path`` because ``Note``/``Task``
# round-trip unknown frontmatter keys through an ``extra`` stash — a merged
# ``path`` key would be stashed as a foreign frontmatter key and then reappear in
# every ``--json`` payload. The daemon server imports :func:`entity_row` so the
# two ends of the codec are written once, in the module that owns the protocol.


def entity_row(model: Note, path: Path) -> dict[str, Any]:
    """Encode one list row for the wire: frontmatter under ``meta``, location under ``path``."""
    return {"meta": model.model_dump(mode="json"), "path": str(path)}


def _wire_list(result: Any, key: str) -> list[Any] | None:
    """The row list under ``key`` in a reply, or ``None`` for any unexpected shape."""
    if not isinstance(result, dict):
        return None
    rows = result.get(key)
    return rows if isinstance(rows, list) else None


def _decode_rows(result: Any, model: type[Note] | type[Task]) -> list[tuple[Any, Path]] | None:
    """Decode ``{"entries": [...]}`` into ``(model, path)`` pairs — all or nothing.

    ``None`` means *this reply cannot be trusted*, and the caller must run its
    file-op fallback rather than hand back what it managed to parse. A single
    undecodable row is enough: on disk a malformed file is **data** and skipping
    it is the documented tolerance, but on the wire it is **protocol skew** — an
    older daemon still running after a schema change answers ``ok: true`` with
    rows that no longer validate, and silently returning a short (or empty) list
    would show the user "no notes" while the vault is full of them. That is the
    accelerator gating the read, which invariant 1 forbids.
    """
    rows = _wire_list(result, "entries")
    if rows is None:
        return None
    decoded: list[tuple[Any, Path]] = []
    for row in rows:
        if not isinstance(row, dict):
            return None
        meta = row.get("meta")
        path = row.get("path")
        if not isinstance(meta, dict) or not isinstance(path, str):
            return None
        try:
            decoded.append((model.model_validate(meta), Path(path)))
        except (msgspec.ValidationError, TypeError):
            return None
    return decoded


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

        Raises an ``OSError`` (missing/refused socket, timeout, broken pipe) when
        the exchange fails and ``json.JSONDecodeError`` on a truncated reply — both
        caught by :meth:`call` — and :class:`DaemonError` when the daemon answers
        with an ``ok: false`` envelope.
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

    def call(
        self,
        method: str,
        params: dict[str, Any],
        fallback: Fallback,
        *,
        fallback_codes: frozenset[int] = _FALLBACK_CODES,
    ) -> Any:
        """Call ``method`` over the socket; run ``fallback`` if the daemon is down.

        Any transport failure — a missing/refused socket (``OSError``), a hung
        daemon (``TimeoutError``), a mid-response crash
        (``BrokenPipeError`` / ``ConnectionResetError``), or a truncated reply
        (``json.JSONDecodeError``) — is swallowed and the fallback's result
        returned; it is never re-raised. A *live* daemon's ``ok: false`` reply
        surfaces as :class:`DaemonError`, except for ``fallback_codes`` (default
        ``{503}`` — reserved-but-unimplemented methods), which also run the
        fallback. Every other code (domain errors 2/3/4) propagates.
        """
        try:
            return self._request(method, params)
        except (OSError, json.JSONDecodeError):
            return fallback()
        except DaemonError as exc:
            if exc.code in fallback_codes:
                return fallback()
            raise

    def _warm(self, method: str, params: dict[str, Any]) -> Any:
        """Ask the daemon for a wired read; :data:`_UNSERVED` when it cannot answer.

        Wraps :meth:`call` with the read verbs' wider fallback-code set and a
        sentinel fallback, so each verb decides *how* to compute its own on-disk
        result instead of building it eagerly for a call that will usually succeed.
        """
        return self.call(method, params, lambda: _UNSERVED, fallback_codes=_READ_FALLBACK_CODES)

    # -- verbs ------------------------------------------------------------- #

    def ping(self) -> Any:
        """Liveness probe — no fallback; a down daemon propagates the error."""
        return self._request("ping", {})

    def is_up(self) -> bool:
        """Whether the daemon answers a liveness ping (never raises).

        A down/absent daemon (missing or refused socket, timeout, garbled reply)
        yields ``False`` — the single liveness check both the ``search`` hybrid gate
        and the ``recent-activity`` degradation notice share.
        """
        try:
            self.ping()
        except (OSError, json.JSONDecodeError):
            return False
        return True

    def note_list(
        self,
        config: Config,
        *,
        tags: list[str] | None = None,
        any_tag: bool = False,
        owner: str | None = None,
        note_type: str | None = None,
        since: str | None = None,
        sort: str = "updated",
        limit: int | None = None,
    ) -> list[NoteView]:
        """List notes — warm-index served when the daemon is up, disk-walked when down.

        The spec is built (and validated) here, so an invalid ``sort``/``since``
        raises ``ValueError`` on both paths before any socket call. The fallback
        is :func:`~shards.core.notes.list_notes` — the same
        :func:`~shards.core.notes.select_notes` predicate the daemon applies to
        index rows, over the on-disk walk instead — so the two results are
        identical. Returned views carry ``body=""`` (see the module docstring).
        """
        spec = NoteFilter.build(
            tags=tags,
            any_tag=any_tag,
            owner=owner,
            note_type=note_type,
            since=since,
            sort=sort,
            limit=limit,
        )
        decoded = _decode_rows(self._warm("note.list", spec.to_params()), Note)
        if decoded is None:
            return list_notes(
                config,
                tags=tags,
                any_tag=any_tag,
                owner=owner,
                note_type=note_type,
                since=since,
                sort=sort,
                limit=limit,
            )
        return [NoteView(note=note, body="", path=path) for note, path in decoded]

    def task_list(
        self,
        config: Config,
        *,
        status: str | None = None,
        owner: str | None = None,
        mine: bool = False,
        tags: list[str] | None = None,
        any_tag: bool = False,
        project: str | None = None,
        since: str | None = None,
        stale: str | None = None,
        sort: str = "updated",
        limit: int | None = None,
    ) -> list[TaskView]:
        """List tasks — warm-index served when the daemon is up, disk-walked when down.

        Mirrors :meth:`note_list`; ``mine`` is resolved against ``config.agent``
        here and travels with the request, because the daemon's own configured
        identity is not the calling agent's. ``status`` accepts a comma-separated
        set (team-awareness/4); ``stale`` is the inverse of ``since`` over the
        same ``updated`` field — see :class:`~shards.core.tasks.TaskFilter` for
        the exact semantics, built once here so both the warm request and the
        on-disk fallback validate identically before any socket call.
        """
        spec = TaskFilter.build(
            config,
            status=status,
            owner=owner,
            mine=mine,
            tags=tags,
            any_tag=any_tag,
            project=project,
            since=since,
            stale=stale,
            sort=sort,
            limit=limit,
        )
        decoded = _decode_rows(self._warm("task.list", spec.to_params()), Task)
        if decoded is None:
            return list_tasks(
                config,
                status=status,
                owner=owner,
                mine=mine,
                tags=tags,
                any_tag=any_tag,
                project=project,
                since=since,
                stale=stale,
                sort=sort,
                limit=limit,
            )
        return [TaskView(task=task, body="", path=path) for task, path in decoded]

    def tag_pull(
        self,
        config: Config,
        *,
        tags: list[str] | None = None,
        type_filter: str | None = None,
        owner: str | None = None,
        status: str | None = None,
        limit: int = 10,
    ) -> list[SearchResult]:
        """Frontmatter tag pull — warm-index served when up, corpus-walked when down.

        The tag pull is a pure metadata filter over the *whole* corpus (foreign
        files included, surfacing with ``id=None``), which is exactly what the
        index holds — so the warm answer is exact, not approximate.
        """
        spec = TagPullFilter.build(
            tags=tags,
            type_filter=type_filter,
            owner=owner,
            status=status,
            limit=limit,
        )
        hits = _decode_results(self._warm("search.tag_pull", spec.to_params()))
        if hits is None:
            return tagpull(
                config,
                tags=tags,
                type_filter=type_filter,
                owner=owner,
                status=status,
                limit=limit,
            )
        return hits

    def vault_status(self, config: Config) -> dict[str, Any]:
        """Vault health — warm counts when the daemon is up, a direct scan when down.

        Same payload either way: note count, tasks-by-status, freshness, dangling
        links, stale locks.

        The daemon ships only the half its index can derive — note count, task
        statuses, the freshness row — and *this* side finishes the report through
        :func:`shards.core.lenses.status_report`. The remainder reads note/task
        bodies (dangling links) and lists lock files off disk, and the daemon
        dispatches handlers synchronously on its event loop, so computing it there
        would block every other agent's warm read behind one ``shards status``.
        Here it blocks only this invocation. A reply that is missing or ill-typed
        is treated as unserved: the two primitives are then computed from the same
        :func:`shards.core.lenses.status_inputs` over the on-disk lenses, so the
        report is never returned half-built.
        """
        # Imported lazily: ``core.lenses`` imports this module (for the project
        # lens's task list), so a module-level import here would be circular.
        from shards.core.lenses import status_inputs, status_report

        served: dict[str, Any] | None = _decode_status(self._warm("vault.status", {}))
        if served is None:
            served = {
                **status_inputs(list_notes(config, limit=None), list_tasks(config, limit=None)),
                "newest": scan_recent(config, 1),
            }
        return status_report(
            config,
            note_count=served["note_count"],
            task_statuses=served["task_statuses"],
            newest=served["newest"],
        )

    def activity_recent(self, config: Config, limit: int = DEFAULT_RECENT_LIMIT) -> Any:
        """Most-recently-modified entries; falls back to a dir scan when down.

        A live daemon answers from its warm in-process index; when the socket is
        unreachable the fallback runs :func:`shards.index.warm.scan_recent`, an
        mtime-sorted scan of ``notes/`` and ``tasks/``. Both return the same
        ``{"entries": [...]}`` shape.

        ``fallback_codes`` is empty here: a config-less daemon does not register
        ``activity.recent`` at all and answers ``404``, which must *propagate* (it
        is a server-state signal the ``recent-activity`` lens reports on), so only
        a genuine socket-down error triggers the scan fallback.
        """
        return self.call(
            "activity.recent",
            {"limit": limit},
            lambda: {"entries": scan_recent(config, limit)},
            fallback_codes=frozenset(),
        )


def _decode_results(result: Any) -> list[SearchResult] | None:
    """Decode ``{"results": [...]}`` into hits — all or nothing (see :func:`_decode_rows`)."""
    rows = _wire_list(result, "results")
    if rows is None:
        return None
    hits: list[SearchResult] = []
    for row in rows:
        try:
            hits.append(msgspec.convert(row, SearchResult))
        except (msgspec.ValidationError, TypeError):
            return None
    return hits


def _decode_status(result: Any) -> dict[str, Any] | None:
    """Decode the ``vault.status`` half-report, or ``None`` if it is not usable.

    The daemon ships only what its index can derive — ``note_count``,
    ``task_statuses`` and the one-row ``newest`` window; anything missing or
    ill-typed means the client computes the whole report itself.
    """
    if not isinstance(result, dict):
        return None
    note_count = result.get("note_count")
    statuses = result.get("task_statuses")
    newest = result.get("newest")
    if isinstance(note_count, bool) or not isinstance(note_count, int):
        return None
    if not isinstance(statuses, list) or not all(isinstance(s, str) for s in statuses):
        return None
    if not isinstance(newest, list):
        return None
    return {"note_count": note_count, "task_statuses": statuses, "newest": newest}
