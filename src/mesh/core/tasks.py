"""Task domain logic: create tasks and update their lifecycle fields.

A task is a :class:`~mesh.schemas.task.Task` — a note with ``type: task`` plus
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

import os
from collections.abc import Iterable, Iterator, Mapping
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, get_args

import frontmatter
from msgspec import ValidationError

from mesh.core.errors import MeshError
from mesh.core.ids import generate_task_id
from mesh.core.notes import (
    MetaRow,
    _append_to_end,
    _append_under_section,
    _format_block,
    _format_stamp,
    _matches_tags,
    _opt_datetime,
    _opt_int,
    _opt_str,
    _parse_since,
    _str_tuple,
    _title_collision,
    _validate_owner,
    apply_tag_spec,
)
from mesh.core.wikilinks import resolve_wikilinks
from mesh.schemas.config import Config
from mesh.schemas.task import Task, TaskStatus
from mesh.storage.files import atomic_write, dump_post, iter_md, read_post, task_folder
from mesh.storage.locks import allocator_lock_path, hold
from mesh.storage.sandbox import safe_resolve

_ID_PREFIX = "t-"
_TASK_SUBDIRS: tuple[str, ...] = ("open", "done")
_SORT_FIELDS: tuple[str, ...] = ("updated", "created", "title", "priority")
_DEFAULT_LIMIT = 20
# The write-boundary status vocabulary, reused as the *read*-boundary vocabulary
# for ``--status`` (team-awareness/4): a CSV entry outside this set is a caller
# error (exit 2), never a silent empty result. Derived from the schema's own
# ``Literal`` — one vocabulary, not a second copy that could drift from it.
_TASK_STATUSES: tuple[str, ...] = get_args(TaskStatus)
# The *write*-boundary priority vocabulary (team-awareness/5). Deliberately NOT a
# schema ``Literal`` — see ``select_tasks``/``_priority_rank`` below for why the
# read side stays tolerant. ``create_task``/``update_task`` reject anything
# outside this set (CLI exit 2, naming the allowed values), so every *new* write
# is canonical while a legacy free-form value already on disk keeps reading fine.
_PRIORITY_VALUES: tuple[str, ...] = ("high", "normal", "low")
# Canonical rank for ``--sort priority``: ascending, so ``high`` sorts first.
# ``None`` *and* any value outside the vocabulary (a legacy/free-form string)
# share the same trailing rank — "unprioritized" is one bucket, not two, and
# neither is ever dropped from the listing (tolerant read).
_PRIORITY_RANK: dict[str, int] = {"high": 0, "normal": 1, "low": 2}
_PRIORITY_UNRANKED = 3
# Terminal statuses: a re-run of finish/cancel on one of these is a no-op — the
# file already lives in ``tasks/done/`` and the outcome/cancel section is fixed.
_TERMINAL_STATUSES: frozenset[str] = frozenset({"done", "cancelled"})
_OUTCOME_HEADING = "## Outcome"
_CANCELLED_HEADING = "## Cancelled"


class TaskError(MeshError):
    """Base class for task-resolution failures."""


class TaskNotFoundError(TaskError):
    """No task file matches the given id (CLI exit 3)."""

    code = 3

    def __init__(self, task_id: str) -> None:
        self.task_id = task_id
        super().__init__(f"task not found: {task_id}")


class ClaimConflictError(TaskError):
    """The task is already claimed by a *different* agent (CLI exit 4).

    Carries ``task_id`` and the ``existing_owner`` recorded in ``claimed_by`` so
    both the CLI and the MCP boundary (agent-usability/5's structured payload)
    can name which task and who holds it — the one fact an agent needs to
    branch on (pick another task, wait, or escalate to that agent). Per root
    ``AGENTS.md`` §6, ``existing_owner`` is trusted local input naming who
    *claims* to hold the task, never a verified/authorized identity. A
    same-agent reclaim is *not* a conflict — it is an idempotent no-op (see
    :func:`claim_task`).
    """

    code = 4

    def __init__(self, task_id: str, existing_owner: str) -> None:
        # Named once here (shared by claim and release) so a batch script's
        # stderr line carries the id, not just the holder — MCP already exposes
        # ``task_id`` structurally via ``_STRUCTURED_ATTRS``; only the CLI's
        # plain ``str(exc)`` line was missing it.
        super().__init__(f"task {task_id} already claimed by {existing_owner}")
        self.task_id = task_id
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
    return config.core.vault_path / "tasks"


def _iter_task_files(config: Config) -> Iterator[Path]:
    """Yield every ``*.md`` directly under ``tasks/open/`` and ``tasks/done/``.

    Non-recursive, and deliberately so: these two folders *are* the task
    lifecycle, and :func:`_resolve_task_path` / :func:`_id_taken` resolve through
    here, so a file nested any deeper is not a task this program can get, claim or
    finish. Both live and terminal folders are scanned so a task is resolvable
    through its whole lifecycle; ``.locks/`` holds no ``.md``, so it is naturally
    excluded. :func:`in_task_scope` is the membership form of this same scope —
    keep the two in step. Each folder is a non-recursive call onto the one shared
    vault walk (:func:`mesh.storage.files.iter_md`).
    """
    root = _tasks_root(config)
    for sub in _TASK_SUBDIRS:
        yield from iter_md(root / sub, recursive=False)


def in_task_scope(vault: Path, path: Path) -> bool:
    """Whether ``path`` lies in the folder scope :func:`_iter_task_files` walks.

    The membership form of that walk (see :func:`mesh.core.notes.in_note_scope`
    for why the daemon needs one). Note the asymmetry with notes: this scope is
    the two folders themselves, **not** their subtrees, so ``tasks/open/sub/t-x.md``
    and ``tasks/archive/t-x.md`` are outside it — matching the walk, and matching
    the fact that neither file is resolvable by ``task get``.
    """
    root = vault / "tasks"
    return path.suffix == ".md" and path.parent in {root / sub for sub in _TASK_SUBDIRS}


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
    vault = config.core.vault_path
    for path in _iter_task_files(config):
        if path.stem == task_id:
            return safe_resolve(vault, path)
    raise TaskNotFoundError(task_id)


def _lock_path(config: Config, task_id: str) -> Path:
    return _tasks_root(config) / ".locks" / f"{task_id}.lock"


def _validate_priority(priority: str | None) -> None:
    """Reject a ``priority`` outside the write-boundary vocabulary (R5).

    Mirrors :func:`mesh.core.notes._validate_owner`: ``None`` (unset) is
    exempt, an explicit out-of-vocabulary value raises ``ValueError`` (CLI exit
    2) naming the allowed values. Called *before* any lock/write in both
    :func:`create_task` and :func:`update_task`, so a rejected write leaves the
    file untouched. This is the *only* place the vocabulary is enforced — the
    schema stays ``str | None`` (tolerant read) precisely so a legacy file
    already carrying a free-form value never fails validation on read.
    """
    if priority is not None and priority not in _PRIORITY_VALUES:
        raise ValueError(f"invalid priority: {priority!r} (use {', '.join(_PRIORITY_VALUES)})")


def _priority_rank(priority: str | None) -> int:
    """Canonical sort rank for ``--sort priority`` — tolerant of anything.

    Known values rank by :data:`_PRIORITY_RANK`; ``None`` and any unrecognised
    free-form value both fall through to :data:`_PRIORITY_UNRANKED`, the same
    trailing bucket — a legacy priority is never treated as an error here, only
    ordered last (R5's "tolerant read" half).
    """
    if priority is None:
        return _PRIORITY_UNRANKED
    return _PRIORITY_RANK.get(priority, _PRIORITY_UNRANKED)


def _notify(config: Config, path: Path) -> None:
    """Announce a vault change to a running daemon, if there is one.

    Imported lazily: ``daemon.client`` imports this module's filter types, so a
    module-level import here would close a cycle. Lazy import is also what keeps
    the CLI's cold start honest (invariant 6) — a write pays for the client only
    when it actually writes.
    """
    from mesh.daemon.client import notify_change

    notify_change(config, path)


def _persist(config: Config, path: Path, post: frontmatter.Post) -> None:
    """Write the task atomically, then tell any running daemon it changed.

    The notification is best-effort and post-durability: the file is already on
    disk when it is sent, and :func:`notify_change` swallows every failure. It
    exists so an agent that writes and immediately lists sees its own change,
    instead of racing the watcher's event delivery and concluding the write was
    lost.
    """
    atomic_write(path, dump_post(post))
    _notify(config, path)


def create_task(
    config: Config,
    title: str,
    *,
    priority: str | None = None,
    tags: list[str] | None = None,
    owner: str | None = None,
    body: str = "",
    project: str | None = None,
    blocks: list[str] | None = None,
    blocked_by: list[str] | None = None,
) -> Task:
    """Create a new task in ``tasks/open/`` and return its frontmatter (R1).

    Generates a deterministic hash ``t-`` id (extended on collision), writes
    ``tasks/open/<id>.md`` with ``status: open`` and ``claimed_by: ~``, and sets
    ``created == updated`` to the same instant (birth). ``owner`` defaults to the
    resolved config agent when not given; an explicit ``owner`` outside a
    non-empty ``[tasks].collections`` raises ``ValueError`` (CLI exit 2 — checked
    before any write, so nothing is created). An explicit ``priority`` outside
    :data:`_PRIORITY_VALUES` (``high``/``normal``/``low``) likewise raises
    ``ValueError`` (CLI exit 2, via :func:`_validate_priority`, checked before any
    write) — the write boundary is canonical even though the schema stays
    tolerant on read (R5). ``project`` is an optional soft link to a
    ``type: project`` note id (no validation — any string, a dangling id is
    tolerated); like ``priority`` it is a declared optional, written as ``null``
    when unset. ``blocks``/``blocked_by`` are stored verbatim but carry no
    readiness logic in v1. ``related`` is derived from the body's wikilinks,
    exactly as notes. The write is atomic.

    Id allocation (``_id_taken`` scan + the ``generate_task_id`` extension loop)
    and the write both run under the per-kind allocator lock at
    ``tasks/.locks/_create.lock`` (see :func:`mesh.storage.locks.allocator_lock_path`),
    so two concurrent creates that resolve the same candidate id can no longer
    both pass the check and race ``os.replace`` — the second waits, rescans, and
    extends past the collision instead of destroying the first file. A per-entity
    lock cannot serve here (the id does not exist yet to name one), hence the
    coarser per-kind lock; contention is bounded because a create is one scan
    plus one write.
    """
    _validate_owner(config, owner)
    _validate_priority(priority)

    vault = config.core.vault_path
    with hold(allocator_lock_path(_tasks_root(config))):
        now = _now()
        task_id = generate_task_id(
            now.isoformat(), title, exists=lambda candidate: _id_taken(config, candidate)
        )
        _, related = resolve_wikilinks(body, vault)
        task = Task.model_validate(
            {
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
                "project": project,
                "blocks": list(blocks or []),
                "blocked_by": list(blocked_by or []),
            }
        )

        path = safe_resolve(vault, task_folder("open", vault) / f"{task_id}.md")
        post = frontmatter.Post(body)
        # Serialize the frontmatter from the validated model — one on-disk contract.
        post.metadata = task.model_dump(mode="python")
        _persist(config, path, post)
    return task


def _validated_task(meta: dict[str, Any], target: str) -> Task:
    """Decode on-disk frontmatter into a :class:`Task`, or raise not-found.

    The task-side twin of ``notes._validated_note`` — same reasoning, same single
    boundary: a schema-invalid file yields one exit code (3) whatever verb reached
    it, instead of 3 from ``get`` and 2 from every mutation.
    """
    try:
        return Task.model_validate(meta)
    except ValidationError as exc:
        raise TaskNotFoundError(target) from exc


def update_task(
    config: Config,
    task_id: str,
    *,
    priority: str | None = None,
    tags: str | None = None,
    title: str | None = None,
    project: str | None = None,
    owner: str | None = None,
    blocks: list[str] | None = None,
    blocked_by: list[str] | None = None,
) -> Task:
    """Update a task's fields in place and bump ``updated`` (R1, R3).

    Only the supplied fields change: ``priority``, ``title``, ``project`` and
    ``owner`` are set directly, ``tags`` mutates the tag list (additive, delta
    ``+x,-y``, or explicit ``=x,y`` replace — see
    :func:`mesh.core.notes.apply_tag_spec` /
    :data:`mesh.core.notes.TAG_SPEC_SEMANTICS`), and
    ``blocks``/``blocked_by`` are replaced verbatim (inert — no readiness
    logic). The read-modify-write runs under the per-entity ``O_EXCL`` lock and
    mutates the parsed metadata in place (never rebuilding it), so ``status``,
    ``claimed_by``, ``related`` and any unknown keys round-trip untouched — and
    a task that carried no ``project`` key keeps carrying none unless
    ``project`` is passed here. The write is atomic. The id is resolved
    *inside* the lock (the lock id derives from ``task_id``, not the file
    location, so it stays stable across a concurrent finish/cancel move),
    closing the window where a racing finish renames the file open→done before
    this write. Raises :class:`TaskNotFoundError` when the id resolves to no
    file, or when the resolved file turns unreadable/malformed before this read
    (via :func:`mesh.storage.files.read_post`) — matching :func:`get_task`.

    ``owner`` is **reassignment**, a distinct field and a distinct code path
    from :func:`claim_task`/:func:`release_task` — those mutate ``claimed_by``
    (who is executing right now), this mutates ``owner`` (who is accountable).
    An explicit ``owner`` outside a non-empty ``[tasks].collections`` raises
    ``ValueError`` (CLI exit 2 via :func:`mesh.core.notes._validate_owner`),
    checked *before* the lock is taken so an unknown identity writes nothing —
    mirroring :func:`create_task`. Reassignment never touches ``claimed_by``:
    handing a task to someone and having them start work on it are different
    actions (the latter is claiming *as* that agent, via the global
    ``--owner``/``[core].agent`` identity :func:`claim_task` already resolves).

    An explicit ``priority`` outside :data:`_PRIORITY_VALUES` likewise raises
    ``ValueError`` (CLI exit 2 via :func:`_validate_priority`), checked before
    this same lock — a rejected priority write touches nothing on disk (R5).
    """
    _validate_owner(config, owner)
    _validate_priority(priority)
    with hold(_lock_path(config, task_id)):
        path = _resolve_task_path(config, task_id)
        post = read_post(path)
        if post is None:
            raise TaskNotFoundError(task_id)
        if priority is not None:
            post.metadata["priority"] = priority
        if title is not None:
            post.metadata["title"] = title
        if project is not None:
            post.metadata["project"] = project
        if owner is not None:
            post.metadata["owner"] = owner
        if tags is not None:
            current = post.metadata.get("tags") or []
            existing = [str(t) for t in current] if isinstance(current, list) else []
            post.metadata["tags"] = apply_tag_spec(existing, tags)
        if blocks is not None:
            post.metadata["blocks"] = list(blocks)
        if blocked_by is not None:
            post.metadata["blocked_by"] = list(blocked_by)
        post.metadata["updated"] = _now()
        task = _validated_task(post.metadata, task_id)
        _persist(config, path, post)
    return task


def append_task(
    config: Config,
    task_id: str,
    text: str,
    *,
    section: str | None = None,
    timestamp: bool = False,
    actor: str | None = None,
) -> Task:
    """Append ``text`` to a task's body and bump ``updated`` (R2).

    The missing half of "a task is a note": a task body is otherwise write-once
    after creation (only :func:`finish_task`/:func:`cancel_task` ever touch it,
    and only once, terminally). This reuses :func:`mesh.core.notes.append_note`'s
    body-editing helpers verbatim (``_append_to_end``, ``_append_under_section``,
    ``_format_block``) rather than forking a second append implementation — the
    existing precedent for borrowing private note helpers (``_matches_tags``,
    ``_parse_since``, ``_validate_owner``, already imported above).

    Never a lifecycle transition: ``status`` and folder are left untouched, so a
    ``claimed`` task appended to stays ``claimed`` in ``tasks/open/``, and a
    ``done`` task appended to stays ``done`` in ``tasks/done/`` — appending a
    post-mortem to finished work is legitimate and does not resurrect it or add a
    second ``## Outcome``/``## Cancelled`` section. ``related`` is recomputed from
    the amended body, exactly as notes, so a ``[[…]]`` mention in the appended
    text becomes discoverable via ``graph --direction in`` from the mentioned id.

    A ``timestamp`` line names the acting agent (``<iso> — <agent>``,
    team-awareness/8), resolved from ``actor`` when given, else ``config.agent`` —
    the identity of *this* call, not the task's ``owner`` — exactly as
    :func:`mesh.core.notes.append_note`. ``actor`` lets a CLI caller thread the
    resolved ``--owner``/``[core].agent`` acting identity through, rather than the
    stamp silently falling back to ``config.agent`` regardless of ``--owner``.

    Mechanics mirror :func:`update_task`: the id is resolved *inside*
    ``hold(_lock_path(config, task_id))`` (the lock name derives from ``task_id``,
    not the file location) so a concurrent finish/cancel move cannot race the
    read; unknown frontmatter keys round-trip because the parsed metadata dict is
    mutated in place, never rebuilt. Raises :class:`TaskNotFoundError` when the id
    resolves to no file, or when the resolved file turns unreadable/malformed
    before this read (via :func:`mesh.storage.files.read_post`) — matching
    :func:`get_task`.
    """
    block = _format_block(text, timestamp, actor if actor is not None else config.agent)
    with hold(_lock_path(config, task_id)):
        path = _resolve_task_path(config, task_id)
        post = read_post(path)
        if post is None:
            raise TaskNotFoundError(task_id)
        post.content = (
            _append_under_section(post.content, block, section)
            if section is not None
            else _append_to_end(post.content, block)
        )
        _, related = resolve_wikilinks(post.content, config.core.vault_path)
        post.metadata["related"] = related
        post.metadata["updated"] = _now()
        task = _validated_task(post.metadata, task_id)
        _persist(config, path, post)
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
    this read. Raises :class:`TaskNotFoundError` when the id resolves to no file,
    or when the resolved file turns unreadable/malformed before this read (via
    :func:`mesh.storage.files.read_post`) — matching :func:`get_task`.
    """
    with hold(_lock_path(config, task_id)):
        path = _resolve_task_path(config, task_id)
        post = read_post(path)
        if post is None:
            raise TaskNotFoundError(task_id)
        if post.metadata.get("status") in _TERMINAL_STATUSES:
            return _validated_task(post.metadata, task_id)  # idempotent terminal no-op
        existing = post.metadata.get("claimed_by")
        if existing == claimer:
            return _validated_task(post.metadata, task_id)  # idempotent same-owner no-op
        if existing is not None:
            raise ClaimConflictError(task_id, str(existing))
        post.metadata["claimed_by"] = claimer
        post.metadata["status"] = "claimed"
        post.metadata["updated"] = _now()
        task = _validated_task(post.metadata, task_id)
        _persist(config, path, post)
    return task


def release_task(config: Config, task_id: str, releaser: str, *, force: bool = False) -> Task:
    """Atomically release a claim, returning the task to ``open`` (R3).

    The missing inverse of :func:`claim_task`: a compare-and-*clear* on
    ``claimed_by`` under the same per-entity ``O_EXCL`` lock at
    ``tasks/.locks/<id>.lock``, mirroring its four branches:

    * **Terminal** (``done``/``cancelled``) → idempotent no-op: returns the
      current task without writing. A finished/cancelled task never carries a
      live claim to release, and it must never be resurrected into
      ``tasks/open/`` by a release.
    * **Already unclaimed** (``claimed_by`` is null) → idempotent no-op: returns
      the current task without writing (``updated`` untouched). Releasing an
      unclaimed task is not an error — it is the natural end state of "make sure
      nobody is holding this".
    * **Held by `releaser`** → durably clears ``claimed_by``, sets
      ``status=open``, bumps ``updated``, and writes atomically in place.
      ``open`` and ``claimed`` both route to ``tasks/open/``, so a release never
      moves the file.
    * **Held by a different agent** → raises :class:`ClaimConflictError` (CLI
      exit 4) naming the holder, leaving the file untouched — *unless*
      ``force=True``, in which case the clear proceeds exactly as the
      holder-release branch above.

    ``force`` is a **speed bump and an audit affordance, not an authorization
    check**: per root ``AGENTS.md`` §6, ``claimed_by``/``--owner`` are trusted
    local input, not a verified identity, so "holder-only release" is a
    cooperation convention this function enforces by *default* — it is not, and
    cannot be, a security boundary. ``force`` exists precisely so a human/CLI
    operator can recover a claim abandoned by a dead or unresponsive agent
    without that convention becoming a deadlock.

    The lock only serializes the read-modify-write; it is released after the
    write and does **not** encode the claim, so a release is durable exactly as
    a claim is (survives lock TTL expiry and process exit — the state lives in
    the frontmatter). The whole path uses ``storage.locks.acquire`` +
    ``storage.files.atomic_write`` directly, so it behaves identically with the
    daemon down. The id is resolved *inside* the lock (the lock id derives from
    ``task_id``, not the file location, so it stays stable across a concurrent
    finish/cancel move), closing the window where a racing finish renames the
    file open→done before this read. Raises :class:`TaskNotFoundError` when the
    id resolves to no file, or when the resolved file turns unreadable/malformed
    before this read (via :func:`mesh.storage.files.read_post`) — matching
    :func:`get_task`.
    """
    with hold(_lock_path(config, task_id)):
        path = _resolve_task_path(config, task_id)
        post = read_post(path)
        if post is None:
            raise TaskNotFoundError(task_id)
        if post.metadata.get("status") in _TERMINAL_STATUSES:
            return _validated_task(post.metadata, task_id)  # idempotent terminal no-op
        existing = post.metadata.get("claimed_by")
        if existing is None:
            return _validated_task(post.metadata, task_id)  # idempotent already-released no-op
        if existing != releaser and not force:
            raise ClaimConflictError(task_id, str(existing))
        post.metadata["claimed_by"] = None
        post.metadata["status"] = "open"
        post.metadata["updated"] = _now()
        task = _validated_task(post.metadata, task_id)
        _persist(config, path, post)
    return task


def _move_if_needed(config: Config, src: Path, dest: Path) -> None:
    """Atomically rename ``src`` → ``dest`` when they differ (open → done).

    Both paths are announced to any running daemon: the source row must be
    evicted and the destination indexed, or a warm ``task list`` keeps serving a
    finished task at a path that no longer exists.
    """
    if dest != src:
        dest.parent.mkdir(parents=True, exist_ok=True)
        os.replace(src, dest)
        _notify(config, src)
        _notify(config, dest)


def _terminate_task(
    config: Config,
    task_id: str,
    *,
    heading: str,
    status: str,
    text: str | None,
    actor: str | None = None,
) -> Task:
    """Shared machinery behind :func:`finish_task` and :func:`cancel_task`.

    Under the per-entity ``O_EXCL`` lock: append ``heading`` + a stamp line
    (and optional ``text``) to the body, set ``status``, bump ``updated``
    (``created`` untouched), recompute ``related`` from the amended body, write
    atomically in place, then move the file into ``tasks/done/``. The stamp
    names the acting agent (``<iso> — <agent>``, team-awareness/8, via
    :func:`mesh.core.notes._format_stamp`) resolved from ``actor`` when given,
    else ``config.agent`` — the identity of *this* call finishing/cancelling the
    task, not its ``owner`` — degrading to a bare ``iso`` when unset. ``actor``
    lets a CLI caller thread the resolved ``--owner``/``[core].agent`` acting
    identity through, rather than the stamp silently falling back to
    ``config.agent`` regardless of ``--owner``.

    The id is resolved *inside* the lock because a concurrent terminator renames
    the file open→done; the lock id derives from ``task_id`` (not the file
    location) so it stays stable across the move. **Idempotent** on a terminal
    status — no second section is appended.

    The write and the move are two atomic steps, so a crash between them could
    strand a terminal-status file in ``tasks/open/``. The terminal branch
    therefore *reconciles* rather than short-circuiting: it completes any pending
    open→done move, so no crash point leaves an unrecoverable state. Raises
    :class:`TaskNotFoundError` when the id resolves to no file, or when the
    resolved file turns unreadable/malformed before this read (via
    :func:`mesh.storage.files.read_post`) — matching :func:`get_task`.
    """
    vault = config.core.vault_path
    done_path = safe_resolve(vault, task_folder("done", vault) / f"{task_id}.md")
    with hold(_lock_path(config, task_id)):
        path = _resolve_task_path(config, task_id)
        post = read_post(path)
        if post is None:
            raise TaskNotFoundError(task_id)
        if post.metadata.get("status") in _TERMINAL_STATUSES:
            _move_if_needed(config, path, done_path)  # heal a crash-stranded file
            return _validated_task(post.metadata, task_id)

        now = _now()
        stamp = _format_stamp(_iso_utc(now), actor if actor is not None else config.agent)
        block = f"{stamp}\n{text}" if text else stamp
        section = f"{heading}\n\n{block}"
        base = post.content.rstrip("\n")
        post.content = f"{base}\n\n{section}" if base else section

        _, related = resolve_wikilinks(post.content, vault)
        post.metadata["related"] = related
        post.metadata["status"] = status
        post.metadata["updated"] = now
        task = _validated_task(post.metadata, task_id)

        _persist(config, path, post)
        _move_if_needed(config, path, done_path)
    return task


def finish_task(
    config: Config, task_id: str, outcome: str | None = None, *, actor: str | None = None
) -> Task:
    """Finish a task: append a ``## Outcome`` section and move it to ``tasks/done/`` (R3).

    Appends a ``## Outcome`` section (ISO-8601 UTC timestamp + optional ``outcome``
    text), sets ``status=done``, and moves the file open→done. Accepted from any
    non-terminal status (``open``/``claimed``); **idempotent** on a terminal status
    (a re-finish never adds a second ``## Outcome`` and the file stays in
    ``tasks/done/``). ``related`` is recomputed from the amended body; the body is
    inert data. Behaves identically with the daemon down. Raises
    :class:`TaskNotFoundError` when the id resolves to no file. ``actor`` threads
    the resolved acting identity into the stamp — see :func:`_terminate_task`
    for the shared lock/write/move mechanics.
    """
    return _terminate_task(
        config, task_id, heading=_OUTCOME_HEADING, status="done", text=outcome, actor=actor
    )


def cancel_task(
    config: Config, task_id: str, reason: str | None = None, *, actor: str | None = None
) -> Task:
    """Cancel a task: append a ``## Cancelled`` section and move it to ``done/`` (R5).

    The mirror image of :func:`finish_task`: appends a ``## Cancelled`` section
    (ISO-8601 UTC timestamp + optional ``reason`` text), sets ``status=cancelled``,
    and moves the file open→done. Accepted from any non-terminal status;
    **idempotent** on a terminal status (a re-cancel never adds a second section
    and the file stays in ``tasks/done/``). Behaves identically with the daemon
    down. Raises :class:`TaskNotFoundError` when the id resolves to no file.
    ``actor`` threads the resolved acting identity into the stamp — see
    :func:`_terminate_task` for the shared mechanics.
    """
    return _terminate_task(
        config,
        task_id,
        heading=_CANCELLED_HEADING,
        status="cancelled",
        text=reason,
        actor=actor,
    )


def delete_task(config: Config, task_id: str) -> str:
    """Hard-delete a task under the entity lock; return the deleted id (R5).

    Resolves ``task_id`` through :func:`_resolve_task_path` — scanning **both**
    ``tasks/open/`` and ``tasks/done/`` (id-only, no title slug), so a task is
    deletable in *any* lifecycle state — then removes the file permanently: no
    archive, no trash. Resolution happens *inside* the per-entity ``O_EXCL`` lock
    at ``tasks/.locks/<id>.lock`` (the lock id is derived from ``task_id``, not the
    file location, so it stays stable across a concurrent finish/cancel move); this
    serializes the delete against a mid-flight edit so it never unlinks a path that
    a racing finisher just moved open→done. :func:`mesh.storage.locks.hold` clears only *stale*
    lock residue on acquire and releases the lock on exit, so residue is cleaned
    without destroying a live lock. The whole path uses ``storage`` primitives
    directly, so it behaves identically with the daemon down. Raises
    :class:`TaskNotFoundError` when the id resolves to no file.
    """
    with hold(_lock_path(config, task_id)):
        path = _resolve_task_path(config, task_id)
        path.unlink()
        _notify(config, path)  # evict the row before the next read
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
    matched file is unreadable (vanished/permission-denied) or its frontmatter is
    malformed YAML — both surfaced as ``None`` from
    :func:`mesh.storage.files.read_post` and treated as not-found rather than
    crashing; a matching file whose frontmatter parses but is not a valid task
    raises ``ValidationError`` (the CLI maps both to exit 3).
    """
    path = _resolve_task_path(config, task_id)
    post = read_post(path)
    if post is None:
        raise TaskNotFoundError(task_id)
    task = _validated_task(post.metadata, task_id)
    return TaskView(task=task, body=post.content, path=path)


# --------------------------------------------------------------------------- #
# The one task list predicate — shared by the disk walk and the warm index      #
# --------------------------------------------------------------------------- #


def _parse_status_csv(value: str | None) -> tuple[str, ...] | None:
    """Split ``--status`` into a validated, order-preserving set of statuses.

    A single value behaves exactly as before (a one-element tuple); a
    comma-separated list (``"open,claimed"``) becomes a membership test instead
    of the old exact-match, so "all live work" is one call. Unknown entries raise
    ``ValueError`` (CLI exit 2 via ``cli_errors``) naming the offender — a typo in
    ``--status`` must fail loudly, never silently return nothing. Whitespace
    around each entry is trimmed; empty entries (a stray comma) are dropped.
    """
    if value is None:
        return None
    statuses = tuple(dict.fromkeys(s.strip() for s in value.split(",") if s.strip()))
    if not statuses:
        return None
    unknown = [s for s in statuses if s not in _TASK_STATUSES]
    if unknown:
        raise ValueError(f"unknown status: {', '.join(unknown)} (use {', '.join(_TASK_STATUSES)})")
    return statuses


@dataclass(frozen=True)
class TaskFilter:
    """A normalized, socket-transportable ``task list`` filter/sort/limit spec.

    The task twin of :class:`mesh.core.notes.NoteFilter`; see there for the
    build-once/ship-over-the-socket contract. ``mine`` carries its identity
    explicitly in ``me`` rather than reading a config: the daemon's own
    ``[core].agent`` is *not* the calling agent's, so "mine" must be resolved on
    the caller's side and travel with the request.

    ``status`` is a membership set (team-awareness/4): ``None`` matches every
    status, otherwise a task's status must be *in* the set — a single-entry set
    behaves exactly like the old exact-match. ``cutoff`` (``--since``) and
    ``stale_cutoff`` (``--stale``) are the two ends of one recency axis, both
    parsed by :func:`mesh.core.notes._parse_since` from the identical duration
    grammar: ``cutoff`` is a *floor* (keep ``updated >= cutoff``, i.e. "touched
    recently"), ``stale_cutoff`` is a *ceiling* (keep ``updated < stale_cutoff``,
    i.e. "not touched recently") — literal inverses over the same field. They are
    conjunctive and independent of ``status``: passing both narrows to the band
    ``cutoff <= updated < stale_cutoff``, and neither implies any particular
    status filter.

    ``available`` (team-awareness/5) is the single "takeable work" filter:
    ``status == "open" and claimed_by is None``. It is conjunctive with every
    other filter above, exactly like ``mine``/``status``/tags — passing
    ``--available --tags urgent`` narrows to takeable work tagged urgent. There
    is no separate "unowned" concept: ``owner`` stays accountability, and the
    pool this flag selects is defined purely by ``claimed_by``, per the spec's
    "no unowned state" ruling (``--owner ""`` stays rejected elsewhere).
    """

    status: tuple[str, ...] | None = None
    owner: str | None = None
    mine: bool = False
    me: str | None = None
    tags: tuple[str, ...] | None = None
    any_tag: bool = False
    project: str | None = None
    cutoff: datetime | None = None
    stale_cutoff: datetime | None = None
    available: bool = False
    sort: str = "updated"
    limit: int | None = _DEFAULT_LIMIT

    @classmethod
    def build(
        cls,
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
        available: bool = False,
        sort: str = "updated",
        limit: int | None = _DEFAULT_LIMIT,
    ) -> TaskFilter:
        """Validate and normalize the caller-level arguments into a spec.

        Resolves ``mine`` against ``config.agent`` here, at the caller's boundary.
        Raises ``ValueError`` for an unknown ``sort`` field, an unknown status in
        ``status``, or an unparseable ``since``/``stale`` (the boundary mappers
        turn all of them into exit 2) — *before* any socket call, so validation
        never depends on the daemon being up.
        """
        if sort not in _SORT_FIELDS:
            raise ValueError(f"invalid sort field: {sort!r} (use {', '.join(_SORT_FIELDS)})")
        return cls(
            status=_parse_status_csv(status),
            owner=owner,
            mine=mine,
            me=config.agent,
            tags=tuple(tags) if tags else None,
            any_tag=any_tag,
            project=project,
            cutoff=_parse_since(since) if since else None,
            stale_cutoff=_parse_since(stale) if stale else None,
            available=available,
            sort=sort,
            limit=limit,
        )

    def to_params(self) -> dict[str, Any]:
        """Render the spec as JSON-safe RPC params."""
        return {
            "status": list(self.status) if self.status else None,
            "owner": self.owner,
            "mine": self.mine,
            "me": self.me,
            "tags": list(self.tags) if self.tags else None,
            "any_tag": self.any_tag,
            "project": self.project,
            "cutoff": self.cutoff.isoformat() if self.cutoff is not None else None,
            "stale_cutoff": self.stale_cutoff.isoformat()
            if self.stale_cutoff is not None
            else None,
            "available": self.available,
            "sort": self.sort,
            "limit": self.limit,
        }

    @classmethod
    def from_params(cls, params: Mapping[str, Any]) -> TaskFilter:
        """Rebuild a spec from untrusted RPC params (see :meth:`NoteFilter.from_params`).

        The wire is not a validation boundary — :meth:`build` already validated
        ``status``/``sort``/durations on the caller's side — so a wrong-typed or
        unknown ``status`` entry here is dropped defensively via
        :func:`mesh.core.notes._str_tuple` rather than raising, exactly like
        every other field in this method. ``sort`` is whitelisted against
        :data:`_SORT_FIELDS` here too — a ``priority`` sort request travels the
        identical whitelist as every other field, warm or cold (team-awareness/5).
        """
        sort = params.get("sort")
        return cls(
            status=_str_tuple(params.get("status")),
            owner=_opt_str(params.get("owner")),
            mine=bool(params.get("mine", False)),
            me=_opt_str(params.get("me")),
            tags=_str_tuple(params.get("tags")),
            any_tag=bool(params.get("any_tag", False)),
            project=_opt_str(params.get("project")),
            cutoff=_opt_datetime(params.get("cutoff")),
            stale_cutoff=_opt_datetime(params.get("stale_cutoff")),
            available=bool(params.get("available", False)),
            sort=sort if sort in _SORT_FIELDS else "updated",
            limit=_opt_int(params.get("limit")),
        )


def task_rows(config: Config) -> Iterator[MetaRow]:
    """Yield one :data:`~mesh.core.notes.MetaRow` per readable task file.

    Walks ``tasks/open/`` and ``tasks/done/``. Unreadable and malformed files are
    skipped by :func:`mesh.storage.files.read_post`, so a vanished/permission-
    denied sibling is skipped exactly like malformed YAML — never a crashed scan.
    """
    for path in _iter_task_files(config):
        post = read_post(path)
        if post is None:
            continue
        yield path, post.metadata


def find_duplicate_title(config: Config, title: str) -> str | None:
    """Return the id of an existing task whose title slug-collides with ``title`` (R9).

    The task-side twin of :func:`mesh.core.notes.find_duplicate_title`: same
    slug-normalized rule (see that docstring for the full reasoning — it
    mirrors the resolver's own ``_slugify`` comparison, not
    ``wikilinks._title_index``'s exact-match rule), same before-the-lock
    timing, scoped to ``tasks/{open,done}/`` instead of ``notes/`` via
    :func:`task_rows` so a note sharing a task's title never warns here
    (same-kind only, per R9). Shares :func:`mesh.core.notes._title_collision`
    — the one slug-collision engine — rather than a second copy of the compare.
    """
    return _title_collision(task_rows(config), title, _ID_PREFIX)


def select_tasks(rows: Iterable[MetaRow], spec: TaskFilter) -> list[TaskView]:
    """Apply ``spec`` to ``rows`` — the *one* task filter/sort/limit implementation (R4).

    Called with on-disk rows by :func:`list_tasks` and with warm-index rows by the
    daemon's ``task.list`` handler, so the two paths can never drift. Only rows
    whose frontmatter carries a valid mesh id (``t-`` prefix), declare
    ``type: task``, and validate against :class:`Task` are surfaced; foreign
    (any writer sharing the folder) / malformed rows are skipped silently. Filters (all conjunctive):
    ``status`` membership (a CSV set — ``open,claimed`` matches either; a single
    value matches exactly as before), exact ``owner`` (on the ``owner`` field),
    ``mine`` (``owner`` *or* ``claimed_by`` equals ``spec.me``; an unset ``spec.me``
    degrades ``mine`` to matching nothing rather than passing every ``owner: null``
    task through — the same degrade-to-empty convention unset identity gets
    elsewhere), ``tags`` (AND, or
    OR with ``any_tag``), exact ``project`` (the project-scoped view — only tasks
    whose ``project`` soft link matches), the ``cutoff`` recency *floor* on
    ``updated`` (``--since``: keep ``updated >= cutoff``), and the
    ``stale_cutoff`` recency *ceiling* (``--stale``: keep ``updated <
    stale_cutoff``) — the literal inverse of ``cutoff`` over the same field, so
    the pair is a band-pass when both are given; and ``available`` (R5), the
    single takeable-work filter — ``status == "open" and claimed_by is None`` —
    conjunctive with everything above. ``sort`` is ``updated``/``created``
    (descending), ``title`` (ascending), or ``priority`` (rank ascending —
    ``high`` → ``normal`` → ``low`` → unprioritized, ``created`` ascending
    within a rank; see :func:`_priority_rank` — a garbage/legacy value shares the
    trailing rank with ``None`` rather than being dropped, R5's tolerant-read
    half), tie-broken by path so the order is deterministic and identical on
    both paths; ``limit`` caps the result (``None`` for unbounded). Shares the
    ``--since``/tag/sort semantics with :func:`mesh.core.notes.select_notes`.

    The returned views carry ``body=""`` — see :data:`mesh.core.notes.MetaRow`.
    """
    views: list[TaskView] = []
    for path, meta in rows:
        task_id = meta.get("id")
        if not isinstance(task_id, str) or not task_id.startswith(_ID_PREFIX):
            continue
        if meta.get("type") != "task":
            continue
        try:
            task = Task.model_validate(meta)
        except ValidationError:
            continue
        if spec.status is not None and task.status not in spec.status:
            continue
        if spec.owner is not None and task.owner != spec.owner:
            continue
        # An unset ``me`` degrades ``mine`` to matching nothing (the codebase's
        # established convention for unset identity) rather than passing every
        # ``owner: null`` task through both inequality checks — the unsound
        # half of team-awareness/3 this closes.
        if spec.mine and (
            spec.me is None or (task.owner != spec.me and task.claimed_by != spec.me)
        ):
            continue
        if spec.project is not None and task.project != spec.project:
            continue
        if spec.tags and not _matches_tags(task.tags, list(spec.tags), spec.any_tag):
            continue
        if spec.cutoff is not None and task.updated < spec.cutoff:
            continue
        if spec.stale_cutoff is not None and task.updated >= spec.stale_cutoff:
            continue
        if spec.available and (task.status != "open" or task.claimed_by is not None):
            continue
        views.append(TaskView(task=task, body="", path=path))

    views.sort(key=lambda v: str(v.path))  # deterministic tie order under a stable sort
    if spec.sort == "title":
        views.sort(key=lambda v: v.task.title.lower())
    elif spec.sort == "priority":
        # Stable sort composition: created-ascending first, then rank-ascending
        # over it, so ties within a rank land FIFO by ``created`` (and, beneath
        # that, by the path order already applied above).
        views.sort(key=lambda v: v.task.created)
        views.sort(key=lambda v: _priority_rank(v.task.priority))
    else:
        views.sort(key=lambda v: getattr(v.task, spec.sort), reverse=True)

    if spec.limit is not None and spec.limit >= 0:
        return views[: spec.limit]
    return views


def list_tasks(
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
    available: bool = False,
    sort: str = "updated",
    limit: int | None = _DEFAULT_LIMIT,
) -> list[TaskView]:
    """List mesh tasks across ``tasks/open/`` and ``tasks/done/`` — the on-disk path (R4).

    A thin composition of :func:`task_rows` (the walk) and :func:`select_tasks`
    (the predicate); see the latter for the filter/sort/limit semantics, including
    why the views carry no body. This is also the daemon-down fallback behind
    :meth:`DaemonClient.task_list <mesh.daemon.client.DaemonClient.task_list>`.
    """
    return select_tasks(
        task_rows(config),
        TaskFilter.build(
            config,
            status=status,
            owner=owner,
            mine=mine,
            tags=tags,
            any_tag=any_tag,
            project=project,
            since=since,
            stale=stale,
            available=available,
            sort=sort,
            limit=limit,
        ),
    )
