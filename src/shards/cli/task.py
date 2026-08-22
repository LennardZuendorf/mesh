"""``shards task`` command surface.

Command bodies for the ``task`` verb live here (per the entry-point contract in
``cli/__main__``, which only wires sub-apps together): ``new``, ``update``,
``append``, ``claim``, ``release``, ``finish``, ``cancel``, ``get``, ``list``,
and ``delete`` all live on the same :data:`task_app`.

Output honours the global flags stashed on ``ctx.obj``: ``--quiet`` prints the id
only, ``--json`` prints a machine object (``id``/``status``/``updated``),
otherwise a terse ``<verb> <id>`` line. Resolution failures map to the shared
exit codes (3 not-found, 2 validation).

Every command also redeclares ``--json``/``--quiet`` (and, where an owner is
written or filtered, ``--owner``) as its own local option, so a caller can give
them on either side of the command name (R6, root tech.md § Surface C). Each
command coalesces its local values with the root callback's globals via
:func:`shards.cli._output.coalesce_flags`, called once near the top of the
body. ``claim``/``release`` have no local ``--owner`` of their own — the
acting identity is the global ``--owner`` (or ``[core].agent``) only, read
straight off ``ctx.obj``, unchanged by this unit.

``--status``'s filter help text is generated from ``_TASK_STATUSES``
(``get_args(TaskStatus)``), never a hand-typed list, mirroring ``note.py``'s
``--type`` fix (R7).
"""

from __future__ import annotations

import json
import sys
from typing import get_args

import typer
from msgspec import ValidationError

from shards.cli import _output
from shards.cli._errors import cli_errors
from shards.core.notes import TAG_SPEC_SEMANTICS
from shards.core.tasks import (
    TaskNotFoundError,
    append_task,
    cancel_task,
    claim_task,
    create_task,
    delete_task,
    find_duplicate_title,
    finish_task,
    get_task,
    release_task,
    update_task,
)
from shards.daemon.client import DaemonClient
from shards.schemas.config import load_config
from shards.schemas.task import Task, TaskStatus

_TASK_STATUSES: tuple[str, ...] = get_args(TaskStatus)

task_app = typer.Typer(
    name="task",
    help="Coordinate work as claimable task files.",
    no_args_is_help=True,
)


def _csv(value: str | None) -> list[str] | None:
    """Split a comma-separated option into a trimmed list (``None`` stays ``None``)."""
    if value is None:
        return None
    return [item.strip() for item in value.split(",") if item.strip()]


@task_app.command("new")
def new_command(
    ctx: typer.Context,
    title: str = typer.Argument(..., help="Task title."),
    priority: str | None = typer.Option(None, "--priority", help="Priority label, e.g. high."),
    tags: str | None = typer.Option(None, "--tags", help="Comma-separated tags."),
    owner: str | None = typer.Option(
        None, "--owner", help="Owner identity (must be in [tasks].collections)."
    ),
    body: str | None = typer.Option(None, "--body", help="Task body text."),
    project: str | None = typer.Option(
        None, "--project", help="Soft-link this task to a project note id."
    ),
    blocks: str | None = typer.Option(
        None, "--blocks", help="Comma-separated task ids this blocks (inert v1)."
    ),
    blocked_by: str | None = typer.Option(
        None, "--blocked-by", help="Comma-separated task ids blocking this (inert v1)."
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Create a task in tasks/open/ (status open, unclaimed)."""
    config = load_config()
    _, _, owner = _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet, owner=owner)
    tag_list = _csv(tags) or []
    with cli_errors():
        # Checked before the write, so a hit names the *prior* id rather than the
        # one about to be created (R9). Non-blocking: the create proceeds either
        # way — this is advisory, not a lock-guarded guarantee.
        existing_id = find_duplicate_title(config, title)
        # ValidationError (schema-invalid) and ValueError (owner outside
        # [tasks].collections, enforced once in core) both map to exit 2.
        task = create_task(
            config,
            title,
            priority=priority,
            tags=tag_list,
            owner=owner,
            body=body or "",
            project=project,
            blocks=_csv(blocks),
            blocked_by=_csv(blocked_by),
        )
    if existing_id is not None and not _output.is_quiet(ctx):
        # Advisory, stderr-only — never inside the --json payload (design.md
        # "Infrastructure on stderr, never in JSON payloads"); --quiet hides it.
        typer.echo(f"task new: duplicate title, also used by {existing_id}", err=True)
    _output.emit_mutation(
        ctx, obj_id=task.id, updated=task.updated, verb="created", fields={"status": task.status}
    )


@task_app.command("update")
def update_command(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="Task id."),
    priority: str | None = typer.Option(None, "--priority", help="Set the priority label."),
    tags: str | None = typer.Option(None, "--tags", help=TAG_SPEC_SEMANTICS),
    title: str | None = typer.Option(None, "--title", help="Rewrite the task title."),
    project: str | None = typer.Option(
        None, "--project", help="Set the project soft link (a project note id)."
    ),
    owner: str | None = typer.Option(
        None, "--owner", help="Reassign owner (must be in [tasks].collections)."
    ),
    blocks: str | None = typer.Option(
        None, "--blocks", help="Replace the blocks list (comma-separated, inert v1)."
    ),
    blocked_by: str | None = typer.Option(
        None, "--blocked-by", help="Replace the blocked_by list (comma-separated, inert v1)."
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Update a task's fields (priority, tags, title, project, owner, blocks/blocked_by).

    ``--owner`` reassigns accountability and never touches ``claimed_by`` — use
    ``task claim``/``task release`` for the execution handle. Deliberately
    **not** coalesced with the global ``--owner``: unlike a create (which
    always writes *some* owner), an update's owner is opt-in reassignment —
    folding the ambient "acting identity" flag into an unrelated ``--priority``/
    ``--tags`` update would silently reassign accountability nobody asked to
    change (the same silent-mutation risk R3 rejected for bare-list tags).
    Only an explicit local ``--owner`` reassigns; the global flag still
    coalesces for ``--json``/``--quiet``.
    """
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        task = update_task(
            config,
            task_id,
            priority=priority,
            tags=tags,
            title=title,
            project=project,
            owner=owner,
            blocks=_csv(blocks),
            blocked_by=_csv(blocked_by),
        )
    _output.emit_mutation(
        ctx, obj_id=task.id, updated=task.updated, verb="updated", fields={"status": task.status}
    )


@task_app.command("append")
def append_command(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="Task id."),
    text: str = typer.Argument(..., help="Text to append to the body."),
    section: str | None = typer.Option(
        None, "--section", help="Append under this ## heading (created if absent)."
    ),
    timestamp: bool = typer.Option(
        False, "--timestamp", help="Prepend an ISO-8601 UTC timestamp line."
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Append text to a task's body (no status/folder change; mirrors note append)."""
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        # The stamp names the acting agent — global --owner override, else the
        # configured identity (agent-usability's `--owner` role 5: the body stamp).
        actor = getattr(ctx.obj, "owner", None) or config.agent
        task = append_task(config, task_id, text, section=section, timestamp=timestamp, actor=actor)
    _output.emit_mutation(
        ctx, obj_id=task.id, updated=task.updated, verb="appended", fields={"status": task.status}
    )


@task_app.command("claim")
def claim_command(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="Task id."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Claim a task for the acting agent (atomic; exit 4 if held by another)."""
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        # The acting agent = global --owner override, else the configured identity.
        claimer = getattr(ctx.obj, "owner", None) or config.agent
        if not claimer:
            raise ValueError("no agent identity: set [core].agent or pass --owner")
        task = claim_task(config, task_id, claimer)
    _output.emit_mutation(
        ctx, obj_id=task.id, updated=task.updated, verb="claimed", fields={"status": task.status}
    )


@task_app.command("release")
def release_command(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="Task id."),
    force: bool = typer.Option(
        False, "--force", help="Break another agent's claim (cooperation override, not auth)."
    ),
    note: str | None = typer.Option(
        None, "--note", help="Append this text via task append (e.g. why you're releasing)."
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Release a claim, returning the task to open (idempotent; exit 4 without --force)."""
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        # The acting agent = global --owner override, else the configured identity.
        releaser = getattr(ctx.obj, "owner", None) or config.agent
        if not releaser:
            raise ValueError("no agent identity: set [core].agent or pass --owner")
        task = release_task(config, task_id, releaser, force=force)
        if note is not None:
            # Reuses append_task (R2) rather than a second body-writing path.
            # Stamped and attributed to the releaser (agent-usability's `--owner`
            # role 5: the body stamp) — a handoff note is exactly the accountability
            # record team-awareness/8 stamps everywhere else.
            task = append_task(config, task_id, note, timestamp=True, actor=releaser)
    _output.emit_mutation(
        ctx, obj_id=task.id, updated=task.updated, verb="released", fields={"status": task.status}
    )


@task_app.command("finish")
def finish_command(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="Task id."),
    outcome: str | None = typer.Option(
        None, "--outcome", help="Outcome text recorded under the ## Outcome section."
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Finish a task: append an outcome and move it to tasks/done/ (idempotent)."""
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        # The ## Outcome stamp names the acting agent — global --owner override,
        # else the configured identity (agent-usability's `--owner` role 5).
        actor = getattr(ctx.obj, "owner", None) or config.agent
        task = finish_task(config, task_id, outcome, actor=actor)
    _output.emit_mutation(
        ctx, obj_id=task.id, updated=task.updated, verb="finished", fields={"status": task.status}
    )


@task_app.command("cancel")
def cancel_command(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="Task id."),
    reason: str | None = typer.Option(
        None, "--reason", help="Reason recorded under the ## Cancelled section."
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Cancel a task: append a reason and move it to tasks/done/ (idempotent)."""
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        # The ## Cancelled stamp names the acting agent — global --owner override,
        # else the configured identity (agent-usability's `--owner` role 5).
        actor = getattr(ctx.obj, "owner", None) or config.agent
        task = cancel_task(config, task_id, reason, actor=actor)
    _output.emit_mutation(
        ctx, obj_id=task.id, updated=task.updated, verb="cancelled", fields={"status": task.status}
    )


def _task_meta_lines(task: Task) -> list[str]:
    """Render a task's canonical frontmatter fields as terse ``key: value`` lines.

    Timestamps go through :func:`shards.cli._output._iso_z`, the same helper
    ``emit_mutation`` and ``hit_dict`` use — one field must not render ``Z`` on
    the JSON surfaces and ``+00:00`` here (core-hardening/4).
    """
    return [
        f"id: {task.id}",
        f"type: {task.type}",
        f"title: {task.title}",
        f"status: {task.status}",
        f"priority: {task.priority or ''}",
        f"owner: {task.owner or ''}",
        f"claimed_by: {task.claimed_by or ''}",
        f"project: {task.project or ''}",
        f"tags: {', '.join(task.tags)}",
        f"blocks: {', '.join(task.blocks)}",
        f"blocked_by: {', '.join(task.blocked_by)}",
        f"created: {_output._iso_z(task.created)}",
        f"updated: {_output._iso_z(task.updated)}",
        f"related: {', '.join(task.related)}",
    ]


@task_app.command("get")
def get_command(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="Task id."),
    full: bool = typer.Option(False, "--full", help="Show the complete body (no truncation)."),
    meta_only: bool = typer.Option(False, "--meta-only", help="Frontmatter only; no body."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Read a task: frontmatter + a 200-char body preview by default."""
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        try:
            view = get_task(config, task_id)
        except ValidationError:
            # A t-id file with malformed/incomplete frontmatter is unreadable as a
            # task; treat it as not-found rather than crashing.
            raise TaskNotFoundError(task_id) from None

    if _output.is_quiet(ctx):
        typer.echo(view.task.id)
        return

    if _output.is_json(ctx):
        obj = view.task.model_dump(mode="json")
        if not meta_only:
            obj["body"] = _output.preview(view.body, full)
        typer.echo(json.dumps(obj))
        return

    lines = _task_meta_lines(view.task)
    if not meta_only:
        lines += ["", _output.preview(view.body, full)]
    typer.echo("\n".join(lines))


@task_app.command("list")
def list_command(
    ctx: typer.Context,
    status: str | None = typer.Option(
        None,
        "--status",
        help=(
            f"Filter by status (comma-separated union, e.g. open,claimed): "
            f"{' | '.join(_TASK_STATUSES)}."
        ),
    ),
    owner: str | None = typer.Option(None, "--owner", help="Filter by exact owner."),
    mine: bool = typer.Option(False, "--mine", help="Only tasks I own or have claimed."),
    tags: str | None = typer.Option(None, "--tags", help="Comma-separated tag filter (AND)."),
    any_tag: bool = typer.Option(False, "--any-tag", help="Switch --tags to OR semantics."),
    project: str | None = typer.Option(
        None, "--project", help="Only tasks scoped to this project note id."
    ),
    since: str | None = typer.Option(
        None, "--since", help="Recency floor: updated within <dur> (7d) or since an ISO date."
    ),
    stale: str | None = typer.Option(
        None,
        "--stale",
        help="Recency ceiling: not updated within <dur> (2d) — the inverse of --since.",
    ),
    available: bool = typer.Option(
        False,
        "--available",
        help="Only takeable work: status open and unclaimed (defaults --sort to priority).",
    ),
    sort: str | None = typer.Option(
        None,
        "--sort",
        help="updated | created | title | priority (default: updated, or priority with --available).",
    ),
    limit: int = typer.Option(20, "--limit", help="Cap the number of results."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """List shards tasks (open and done) with filters, sort and limit."""
    config = load_config()
    _, _, owner = _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet, owner=owner)
    tag_list = [t.strip() for t in tags.split(",") if t.strip()] if tags else None
    # Honour --mine whether it lands on this command or as a global flag.
    mine_flag = mine or getattr(ctx.obj, "mine", False)
    # --available's default order is priority — but only when the caller did not
    # ask for a specific --sort themselves.
    sort_field = sort if sort is not None else ("priority" if available else "updated")
    with cli_errors():
        # Served from the daemon's warm index when it is up, from the identical
        # on-disk walk when it is down — one predicate, either way.
        views = DaemonClient().task_list(
            config,
            status=status,
            owner=owner,
            mine=mine_flag,
            tags=tag_list,
            any_tag=any_tag,
            project=project,
            since=since,
            stale=stale,
            available=available,
            sort=sort_field,
            limit=limit,
        )

    if _output.is_quiet(ctx):
        for view in views:
            typer.echo(view.task.id)
        return
    if _output.is_json(ctx):
        typer.echo(json.dumps([_output.to_json_obj(view.task) for view in views]))
        return
    for view in views:
        # id / status / holder (or "-" when unclaimed) / title — tab-separated so
        # a row stays trivially splittable (`cut -f`) even though a title may
        # contain spaces. ``--json`` is unchanged (the full model, untouched).
        holder = view.task.claimed_by or "-"
        typer.echo(f"{view.task.id}\t{view.task.status}\t{holder}\t{view.task.title}")


def _is_tty() -> bool:
    """Whether stdin is an interactive terminal (indirection so tests can fake it)."""
    return sys.stdin.isatty()


@task_app.command("delete")
def delete_command(
    ctx: typer.Context,
    task_id: str = typer.Argument(..., help="Task id."),
    force: bool = typer.Option(
        False, "--force", help="Skip the confirmation prompt and delete immediately."
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Hard-delete a task. Prompts on a TTY; refuses on a machine path without --force."""
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        if not force:
            _output.refuse_delete_if_non_interactive(ctx, tty=_is_tty())
            # click renders "Delete <id>? [y/N]: "; declining aborts (exit 1).
            typer.confirm(f"Delete {task_id}?", abort=True)
        deleted = delete_task(config, task_id)

    if _output.is_quiet(ctx):
        typer.echo(deleted)
    elif _output.is_json(ctx):
        typer.echo(json.dumps({"id": deleted, "deleted": True}))
    else:
        typer.echo(f"deleted {deleted}")
