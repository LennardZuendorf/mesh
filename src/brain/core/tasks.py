"""Task domain logic: create tasks and update their lifecycle fields.

A task is a :class:`~brain.schemas.task.Task` — a note with ``type: task`` plus
lifecycle fields — so this module reuses the shared note/storage primitives:
deterministic ``t-`` hash ids, status-driven folder routing
(``tasks/open/`` vs ``tasks/done/``), atomic writes, and the per-entity
``O_EXCL`` lock. This unit owns the *create* and *update* verbs plus the shared
resolver :func:`_resolve_task_path`; sibling units (claim, finish, cancel, list,
delete) build on the same resolver.

``blocks`` and ``blocked_by`` are *recorded but inert* in v1: they are written
and updated verbatim, with no readiness logic (the dependency graph is deferred
to Phase 3). Resolution is **id-only** — a task is found by its filename stem
(the id), never a title slug — because a task id is the coordination handle
agents hand off, and slug matching would make handoff ambiguous.
"""

from __future__ import annotations

import contextlib
import os
import time
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import yaml
from pydantic import ValidationError

from brain.core.ids import generate_task_id
from brain.core.notes import _matches_tags, _parse_since, apply_tag_spec
from brain.core.wikilinks import resolve_wikilinks
from brain.schemas.config import Config
from brain.schemas.task import Task
from brain.storage.files import atomic_write, task_folder
from brain.storage.locks import LockError, acquire
from brain.storage.sandbox import safe_resolve

_ID_PREFIX = "t-"
_TASK_SUBDIRS: tuple[str, ...] = ("open", "done")
_SORT_FIELDS: tuple[str, ...] = ("updated", "created", "title")
_DEFAULT_LIMIT = 20
_LOCK_WAIT_SECONDS = 15.0
_LOCK_POLL_SECONDS = 0.01
# Terminal statuses: a re-run of finish/cancel on one of these is a no-op — the
# file already lives in ``tasks/done/`` and the outcome/cancel section is fixed.
_TERMINAL_STATUSES: frozenset[str] = frozenset({"done", "cancelled"})
_OUTCOME_HEADING = "## Outcome"
_CANCELLED_HEADING = "## Cancelled"


class TaskError(Exception):
    """Base class for task-resolution failures."""


class TaskNotFoundError(TaskError):
    """No task file matches the given id (CLI exit 3)."""


class ClaimConflictError(TaskError):
    """The task is already claimed by a *different* agent (CLI exit 4).

    Carries the ``existing_owner`` recorded in ``claimed_by`` so the CLI can name
    who holds it. A same-agent reclaim is *not* a conflict — it is an idempotent
    no-op (see :func:`claim_task`).
    """

    def __init__(self, existing_owner: str) -> None:
        super().__init__(f"task already claimed by {existing_owner}")
        self.existing_owner = existing_owner


@dataclass(frozen=True)
class TaskView:
    """A task read off disk: validated frontmatter plus the raw body.

    Read verbs (``get`` / ``list``, sibling units) return these; the CLI renders
    them per the active output flags. ``body`` is inert Markdown, never
    interpreted.
    """

    task: Task
    body: str
    path: Path


def _now() -> datetime:
    return datetime.now(UTC)


def _iso_utc(moment: datetime) -> str:
    """Render ``moment`` as a compact ISO-8601 UTC line (``...Z``), matching notes."""
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


def _tasks_root(config: Config) -> Path:
    return config.core.tolaria_path / "tasks"


def _iter_task_files(config: Config) -> Iterator[Path]:
    """Yield every ``*.md`` under ``tasks/open/`` and ``tasks/done/``.

    ``.locks/`` holds no ``.md``, so it is naturally excluded. Both live and
    terminal folders are scanned so a task is resolvable through its whole
    lifecycle.
    """
    root = _tasks_root(config)
    for sub in _TASK_SUBDIRS:
        folder = root / sub
        if folder.is_dir():
            yield from folder.glob("*.md")


def _id_taken(config: Config, candidate: str) -> bool:
    """Whether a task file with stem ``candidate`` already exists (id collision)."""
    return any(path.stem == candidate for path in _iter_task_files(config))


def _resolve_task_path(config: Config, task_id: str) -> Path:
    """Resolve a task ``id`` to its sandbox-checked file path.

    Scans **both** ``tasks/open/`` and ``tasks/done/`` for a file whose stem
    equals ``task_id`` (id-only — no title-slug matching). Returns the canonical
    path; raises :class:`TaskNotFoundError` when nothing matches. Every
    subsequent domain verb (update, claim, finish, cancel, delete) resolves
    through here.
    """
    vault = config.core.tolaria_path
    for path in _iter_task_files(config):
        if path.stem == task_id:
            return safe_resolve(vault, path)
    raise TaskNotFoundError(task_id)


def _lock_path(config: Config, task_id: str) -> Path:
    return _tasks_root(config) / ".locks" / f"{task_id}.lock"


@contextlib.contextmanager
def _hold_lock(lock_path: Path) -> Iterator[Path]:
    """Hold the entity ``O_EXCL`` lock, waiting out a live holder.

    ``storage.locks.acquire`` is a non-blocking test-and-set: it raises
    :class:`LockError` when a live, fresh lock is held. This wrapper adds the
    bounded wait-and-retry policy so concurrent edits serialize instead of
    failing. Acquisition is retried; the protected body is not.
    """
    deadline = time.monotonic() + _LOCK_WAIT_SECONDS
    while True:
        cm = acquire(lock_path)
        try:
            cm.__enter__()
        except LockError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(_LOCK_POLL_SECONDS)
            continue
        try:
            yield lock_path
        finally:
            cm.__exit__(None, None, None)
        return


def create_task(
    config: Config,
    title: str,
    *,
    priority: str | None = None,
    tags: list[str] | None = None,
    owner: str | None = None,
    body: str = "",
    blocks: list[str] | None = None,
    blocked_by: list[str] | None = None,
) -> Task:
    """Create a new task in ``tasks/open/`` and return its frontmatter (R1).

    Generates a deterministic hash ``t-`` id (extended on collision), writes
    ``tasks/open/<id>.md`` with ``status: open`` and ``claimed_by: ~``, and sets
    ``created == updated`` to the same instant (birth). ``owner`` defaults to the
    resolved config agent when not given; an explicit ``owner`` outside a
    non-empty ``[tasks].collections`` raises ``ValueError`` (CLI exit 2 — checked
    before any write, so nothing is created). ``blocks``/``blocked_by`` are stored
    verbatim but carry no readiness logic in v1. ``related`` is derived from the
    body's wikilinks, exactly as notes. The write is atomic.
    """
    collections = config.tasks.collections
    if owner is not None and collections and owner not in collections:
        raise ValueError(f"unknown owner: {owner!r}")

    vault = config.core.tolaria_path
    now = _now()
    task_id = generate_task_id(
        now.isoformat(), title, exists=lambda candidate: _id_taken(config, candidate)
    )
    _, related = resolve_wikilinks(body, vault)
    meta: dict[str, object] = {
        "id": task_id,
        "type": "task",
        "title": title,
        "tags": list(tags or []),
        "owner": owner if owner is not None else config.agent,
        "created": now,
        "updated": now,
        "related": related,
        "status": "open",
        "priority": priority,
        "claimed_by": None,
        "blocks": list(blocks or []),
        "blocked_by": list(blocked_by or []),
    }
    task = Task.model_validate(meta)

    path = safe_resolve(vault, task_folder("open", vault) / f"{task_id}.md")
    post = frontmatter.Post(body)
    post.metadata = meta
    atomic_write(path, frontmatter.dumps(post))
    return task


def update_task(
    config: Config,
    task_id: str,
    *,
    priority: str | None = None,
    tags: str | None = None,
    title: str | None = None,
    blocks: list[str] | None = None,
    blocked_by: list[str] | None = None,
) -> Task:
    """Update a task's fields in place and bump ``updated`` (R1).

    Only the supplied fields change: ``priority`` and ``title`` are set directly,
    ``tags`` mutates the tag list (delta ``+x,-y`` or replacement — see
    :func:`brain.core.notes.apply_tag_spec`), and ``blocks``/``blocked_by`` are
    replaced verbatim (inert — no readiness logic). The read-modify-write runs
    under the per-entity ``O_EXCL`` lock and mutates the parsed metadata in place
    (never rebuilding it), so ``status``, ``claimed_by``, ``owner``, ``related``
    and any unknown keys round-trip untouched. The write is atomic. The id is
    resolved *inside* the lock (the lock id derives from ``task_id``, not the file
    location, so it stays stable across a concurrent finish/cancel move), closing
    the window where a racing finish renames the file open→done before this write.
    Raises :class:`TaskNotFoundError` when the id resolves to no file.
    """
    with _hold_lock(_lock_path(config, task_id)):
        path = _resolve_task_path(config, task_id)
        post = frontmatter.loads(path.read_text(encoding="utf-8"))
        if priority is not None:
            post.metadata["priority"] = priority
        if title is not None:
            post.metadata["title"] = title
        if tags is not None:
            current = post.metadata.get("tags") or []
            existing = [str(t) for t in current] if isinstance(current, list) else []
            post.metadata["tags"] = apply_tag_spec(existing, tags)
        if blocks is not None:
            post.metadata["blocks"] = list(blocks)
        if blocked_by is not None:
            post.metadata["blocked_by"] = list(blocked_by)
        post.metadata["updated"] = _now()
        task = Task.model_validate(post.metadata)
        atomic_write(path, frontmatter.dumps(post))
    return task


def claim_task(config: Config, task_id: str, claimer: str) -> Task:
    """Atomically claim a task for ``claimer`` (R2).

    Performs a check-and-set on ``claimed_by`` under the per-entity ``O_EXCL``
    lock at ``tasks/.locks/<id>.lock``, so a simultaneous N-way race yields
    exactly one winner:

    * **Unclaimed** (``claimed_by`` is null) → durably records
      ``claimed_by=claimer``, ``status=claimed``, bumps ``updated``, and writes
      atomically to the *resolved* path. ``open`` and ``claimed`` both route to
      ``tasks/open/``, so the file never moves on a claim.
    * **Same agent** (``claimed_by == claimer``) → idempotent no-op: returns the
      current task without writing (``updated`` is left untouched).
    * **Different agent** → raises :class:`ClaimConflictError` (CLI exit 4),
      leaving the file untouched.

    A **terminal** task (``done``/``cancelled``) is never claimable: claiming one
    is an idempotent no-op that returns the current task without writing, so the
    one-way lifecycle (``open→claimed→done|cancelled``) and status/folder routing
    are never violated (a ``done`` task keeps ``status=done`` and stays in
    ``tasks/done/``).

    The lock only serializes the read-modify-write; it is released after the
    write and does **not** encode the claim, which lives durably in the
    frontmatter and so survives lock TTL expiry and process exit. The whole path
    uses ``storage.locks.acquire`` + ``storage.files.atomic_write`` directly, so
    it behaves identically with the daemon down. ``claimer`` must be a non-empty
    identity (the CLI resolves ``--owner``/``[core].agent`` before calling). The
    id is resolved *inside* the lock (the lock id derives from ``task_id``, not
    the file location, so it stays stable across a concurrent finish/cancel move),
    closing the window where a racing finish renames the file open→done before
    this read. Raises :class:`TaskNotFoundError` when the id resolves to no file.
    """
    with _hold_lock(_lock_path(config, task_id)):
        path = _resolve_task_path(config, task_id)
        post = frontmatter.loads(path.read_text(encoding="utf-8"))
        if post.metadata.get("status") in _TERMINAL_STATUSES:
            return Task.model_validate(post.metadata)  # idempotent terminal no-op
        existing = post.metadata.get("claimed_by")
        if existing == claimer:
            return Task.model_validate(post.metadata)  # idempotent same-owner no-op
        if existing is not None:
            raise ClaimConflictError(str(existing))
        post.metadata["claimed_by"] = claimer
        post.metadata["status"] = "claimed"
        post.metadata["updated"] = _now()
        task = Task.model_validate(post.metadata)
        atomic_write(path, frontmatter.dumps(post))
    return task


def finish_task(config: Config, task_id: str, outcome: str | None = None) -> Task:
    """Finish a task: append an outcome section and move it to ``tasks/done/`` (R3).

    Under the per-entity ``O_EXCL`` lock, this appends a ``## Outcome`` section
    carrying an ISO-8601 UTC timestamp line and the (optional) ``outcome`` text to
    the body, sets ``status=done``, bumps ``updated`` (``created`` is untouched),
    then moves the file from ``tasks/open/`` to ``tasks/done/``:

    * the updated content is written atomically **in place** at the resolved open
      path, then :func:`os.replace` atomically renames it into ``tasks/done/`` —
      so exactly one ``<id>.md`` exists at every instant (no duplicate/ghost);
    * accepted from any *non-terminal* status (``open`` or ``claimed`` — R3);
    * **idempotent** on a terminal status (``done``/``cancelled``): returns the
      current task without writing, appending, or moving, so a re-finish never
      adds a second ``## Outcome`` section and the file stays in ``tasks/done/``.

    ``outcome`` is optional — when omitted the heading and timestamp line are
    still appended. ``related`` is recomputed from the amended body (a pure
    function of the body, matching :func:`brain.core.notes.append_note`). The body
    is inert data — never interpreted. The whole path uses ``storage`` primitives
    directly, so it behaves identically with the daemon down. Raises
    :class:`TaskNotFoundError` when the id resolves to no file.
    """
    vault = config.core.tolaria_path
    # Resolve *inside* the lock: finish renames the file (open→done), so a
    # concurrent finisher must re-resolve after the winner's move — otherwise it
    # would read a path that no longer exists. The lock id is derived from
    # ``task_id`` (not the file location), so it is stable across the move.
    with _hold_lock(_lock_path(config, task_id)):
        path = _resolve_task_path(config, task_id)
        post = frontmatter.loads(path.read_text(encoding="utf-8"))
        if post.metadata.get("status") in _TERMINAL_STATUSES:
            return Task.model_validate(post.metadata)  # idempotent terminal no-op

        now = _now()
        stamp = _iso_utc(now)
        block = f"{stamp}\n{outcome}" if outcome else stamp
        section = f"{_OUTCOME_HEADING}\n\n{block}"
        base = post.content.rstrip("\n")
        post.content = f"{base}\n\n{section}" if base else section

        _, related = resolve_wikilinks(post.content, vault)
        post.metadata["related"] = related
        post.metadata["status"] = "done"
        post.metadata["updated"] = now
        task = Task.model_validate(post.metadata)

        atomic_write(path, frontmatter.dumps(post))
        done_path = safe_resolve(vault, task_folder("done", vault) / f"{task_id}.md")
        if done_path != path:
            done_path.parent.mkdir(parents=True, exist_ok=True)
            os.replace(path, done_path)
    return task


def cancel_task(config: Config, task_id: str, reason: str | None = None) -> Task:
    """Cancel a task: append a ``## Cancelled`` section and move it to ``done/`` (R5).

    The mirror image of :func:`finish_task`. Under the per-entity ``O_EXCL`` lock,
    this appends a ``## Cancelled`` section carrying an ISO-8601 UTC timestamp line
    and the (optional) ``reason`` text to the body, sets ``status=cancelled``,
    bumps ``updated`` (``created`` is untouched), then moves the file from
    ``tasks/open/`` to ``tasks/done/``:

    * the updated content is written atomically **in place** at the resolved open
      path, then :func:`os.replace` atomically renames it into ``tasks/done/`` — so
      exactly one ``<id>.md`` exists at every instant (no duplicate/ghost);
    * accepted from any *non-terminal* status (``open`` or ``claimed``);
    * **idempotent** on a terminal status (``done``/``cancelled``): returns the
      current task without writing, appending, or moving, so a re-cancel never adds
      a second ``## Cancelled`` section and the file stays in ``tasks/done/``.

    ``reason`` is optional — when omitted the heading and timestamp line are still
    appended. ``related`` is recomputed from the amended body. The body is inert
    data — never interpreted. The id is resolved *inside* the lock (cancel renames
    the file, so a concurrent canceller must re-resolve after the winner's move);
    the lock id is derived from ``task_id`` and stays stable across the move. The
    whole path uses ``storage`` primitives directly, so it behaves identically with
    the daemon down. Raises :class:`TaskNotFoundError` when the id resolves to no
    file.
    """
    vault = config.core.tolaria_path
    with _hold_lock(_lock_path(config, task_id)):
        path = _resolve_task_path(config, task_id)
        post = frontmatter.loads(path.read_text(encoding="utf-8"))
        if post.metadata.get("status") in _TERMINAL_STATUSES:
            return Task.model_validate(post.metadata)  # idempotent terminal no-op

        now = _now()
        stamp = _iso_utc(now)
        block = f"{stamp}\n{reason}" if reason else stamp
        section = f"{_CANCELLED_HEADING}\n\n{block}"
        base = post.content.rstrip("\n")
        post.content = f"{base}\n\n{section}" if base else section

        _, related = resolve_wikilinks(post.content, vault)
        post.metadata["related"] = related
        post.metadata["status"] = "cancelled"
        post.metadata["updated"] = now
        task = Task.model_validate(post.metadata)

        atomic_write(path, frontmatter.dumps(post))
        done_path = safe_resolve(vault, task_folder("done", vault) / f"{task_id}.md")
        if done_path != path:
            done_path.parent.mkdir(parents=True, exist_ok=True)
            os.replace(path, done_path)
    return task


def delete_task(config: Config, task_id: str) -> str:
    """Hard-delete a task under the entity lock; return the deleted id (R5).

    Resolves ``task_id`` through :func:`_resolve_task_path` — scanning **both**
    ``tasks/open/`` and ``tasks/done/`` (id-only, no title slug), so a task is
    deletable in *any* lifecycle state — then removes the file permanently: no
    archive, no trash. Resolution happens *inside* the per-entity ``O_EXCL`` lock
    at ``tasks/.locks/<id>.lock`` (the lock id is derived from ``task_id``, not the
    file location, so it stays stable across a concurrent finish/cancel move); this
    serializes the delete against a mid-flight edit so it never unlinks a path that
    a racing finisher just moved open→done. :func:`_hold_lock` clears only *stale*
    lock residue on acquire and releases the lock on exit, so residue is cleaned
    without destroying a live lock. The whole path uses ``storage`` primitives
    directly, so it behaves identically with the daemon down. Raises
    :class:`TaskNotFoundError` when the id resolves to no file.
    """
    with _hold_lock(_lock_path(config, task_id)):
        path = _resolve_task_path(config, task_id)
        path.unlink()
    return task_id


# --------------------------------------------------------------------------- #
# Read verbs — get / list (tasks/5). Direct on-disk reads, daemon-independent. #
# --------------------------------------------------------------------------- #


def get_task(config: Config, task_id: str) -> TaskView:
    """Read a single task by id into a :class:`TaskView` (R4).

    Resolves ``task_id`` through :func:`_resolve_task_path`, which scans **both**
    ``tasks/open/`` and ``tasks/done/`` (id-only, no title slug), so a task is
    readable through its whole lifecycle. The frontmatter is validated into a
    :class:`Task` and the raw body is returned verbatim (the CLI truncates for
    previews). Raises :class:`TaskNotFoundError` when no file matches *or* the
    matched file's frontmatter is malformed YAML (unparseable — treated as
    not-found rather than crashing); a matching file whose frontmatter parses but
    is not a valid task raises ``ValidationError`` (the CLI maps both to exit 3).
    """
    path = _resolve_task_path(config, task_id)
    try:
        post = frontmatter.loads(path.read_text(encoding="utf-8"))
    except yaml.YAMLError as exc:
        raise TaskNotFoundError(task_id) from exc
    task = Task.model_validate(post.metadata)
    return TaskView(task=task, body=post.content, path=path)


def list_tasks(
    config: Config,
    *,
    status: str | None = None,
    owner: str | None = None,
    mine: bool = False,
    tags: list[str] | None = None,
    any_tag: bool = False,
    since: str | None = None,
    sort: str = "updated",
    limit: int | None = _DEFAULT_LIMIT,
) -> list[TaskView]:
    """List brain tasks across ``tasks/open/`` and ``tasks/done/``, filtered/sorted (R4).

    Only files whose frontmatter carries a valid brain id (``t-`` prefix), declares
    ``type: task``, and validates against :class:`Task` are surfaced; Tolaria /
    foreign / malformed files are skipped silently. Filters (all conjunctive):
    exact ``status``, exact ``owner`` (on the ``owner`` field), ``mine`` (``owner``
    *or* ``claimed_by`` equals the configured agent), ``tags`` (AND, or OR with
    ``any_tag``), and ``since`` recency on ``updated``. ``sort`` is
    ``updated``/``created`` (descending) or ``title`` (ascending); ``limit`` caps
    the result (``None`` for unbounded). Shares the ``--since``/tag/sort semantics
    with :func:`brain.core.notes.list_notes`.
    """
    if sort not in _SORT_FIELDS:
        raise ValueError(f"invalid sort field: {sort!r} (use {', '.join(_SORT_FIELDS)})")
    cutoff = _parse_since(since) if since else None
    me = config.agent

    views: list[TaskView] = []
    for path in _iter_task_files(config):
        try:
            meta = frontmatter.loads(path.read_text(encoding="utf-8"))
        except yaml.YAMLError:
            continue  # malformed YAML — skip silently, like foreign/invalid files
        task_id = meta.metadata.get("id")
        if not isinstance(task_id, str) or not task_id.startswith(_ID_PREFIX):
            continue
        if meta.metadata.get("type") != "task":
            continue
        try:
            task = Task.model_validate(meta.metadata)
        except ValidationError:
            continue
        if status is not None and task.status != status:
            continue
        if owner is not None and task.owner != owner:
            continue
        if mine and task.owner != me and task.claimed_by != me:
            continue
        if tags and not _matches_tags(task.tags, tags, any_tag):
            continue
        if cutoff is not None and task.updated < cutoff:
            continue
        views.append(TaskView(task=task, body=meta.content, path=path))

    if sort == "title":
        views.sort(key=lambda v: v.task.title.lower())
    else:
        views.sort(key=lambda v: getattr(v.task, sort), reverse=True)

    if limit is not None and limit >= 0:
        return views[:limit]
    return views
