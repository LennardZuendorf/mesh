"""``mesh note`` command surface.

Command bodies for the ``note`` verb live here (per the entry-point contract in
``cli/__main__``, which only wires sub-apps together): ``new``, ``append``,
``update``, ``get``, ``list``, ``delete`` all live on the same :data:`note_app`.

Output honours the global flags stashed on ``ctx.obj``: ``--quiet`` prints the id
only, ``--json`` prints a machine object, otherwise a terse human line. Resolution
failures map to the shared exit codes (3 not-found, 2 validation/ambiguous).

Every command also redeclares ``--json``/``--quiet`` (and, where an owner is
written or filtered, ``--owner``) as its own local option, so a caller can give
them on either side of the command name (R6, root tech.md § Surface C). Each
command coalesces its local values with the root callback's globals via
:func:`mesh.cli._output.coalesce_flags`, called once near the top of the
body — see that function's docstring for the mechanism.

``--type``'s help text is generated from ``_NOTE_TYPES`` (``get_args(NoteType)``),
never a hand-typed list, so it cannot omit a value the schema actually accepts
(the defect this unit fixes: help said "note | log | decision | reference"
while validation also accepted ``project``).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import get_args

import typer
from msgspec import ValidationError

from mesh.cli import _output
from mesh.cli._errors import cli_errors
from mesh.core.notes import (
    TAG_SPEC_SEMANTICS,
    NoteNotFoundError,
    append_note,
    create_note,
    delete_note,
    find_duplicate_title,
    get_note,
    resolve_slug,
    update_note,
)
from mesh.daemon.client import DaemonClient
from mesh.schemas.config import load_config
from mesh.schemas.note import Note, NoteType

_NOTE_TYPES: tuple[str, ...] = get_args(NoteType)

note_app = typer.Typer(
    name="note",
    help="Capture knowledge as Markdown.",
    no_args_is_help=True,
)


def _edit_body() -> str:  # pragma: no cover - interactive-only ($EDITOR) path
    """Open ``$EDITOR`` for the body and return the entered text (TTY only)."""
    import click

    return click.edit() or ""


def _resolve_body(ctx: typer.Context, body: str | None, body_file: str | None) -> str:
    """Resolve the note body per precedence: ``--body`` → ``--file`` → ``$EDITOR``.

    On a headless path (``--json``/``--quiet`` or a non-interactive stdin) with
    neither source given, or an unreadable ``--file``, raises ``ValueError`` —
    mapped to exit 2 by the CLI boundary mapper — rather than launching
    ``$EDITOR``. Callers must invoke this from inside a ``with cli_errors():``
    block.
    """
    if body is not None:
        return body
    if body_file is not None:
        try:
            return Path(body_file).read_text(encoding="utf-8")
        except OSError as exc:
            raise ValueError(f"cannot read --file {body_file}: {exc}") from exc

    if _output.is_machine(ctx) or not _is_tty():
        raise ValueError("no body: pass --body or --file on a non-interactive path")
    return _edit_body()


@note_app.command("new")
def new_command(
    ctx: typer.Context,
    title: str = typer.Argument(..., help="Note title."),
    note_type: str = typer.Option("note", "--type", help=f"Note type: {' | '.join(_NOTE_TYPES)}."),
    tags: str | None = typer.Option(None, "--tags", help="Comma-separated tags."),
    owner: str | None = typer.Option(
        None, "--owner", help="Owner identity (must be in [tasks].collections)."
    ),
    body: str | None = typer.Option(None, "--body", help="Note body text."),
    body_file: str | None = typer.Option(None, "--file", help="Read the body from this file."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Create a note (routed by type); body from --body, --file, or $EDITOR (TTY)."""
    config = load_config()
    _, _, owner = _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet, owner=owner)
    with cli_errors():
        if note_type not in _NOTE_TYPES:
            raise ValueError(f"invalid note type: {note_type}")
        body_text = _resolve_body(ctx, body, body_file)
        tag_list = [t.strip() for t in tags.split(",") if t.strip()] if tags else []
        # Checked before the write, so a hit names the *prior* id rather than the
        # one about to be created (R9). Non-blocking: the create proceeds either
        # way — this is advisory, not a lock-guarded guarantee.
        existing_id = find_duplicate_title(config, title)
        # ValidationError (schema-invalid) and ValueError (owner outside
        # [tasks].collections, enforced once in core) both map to exit 2.
        note = create_note(
            config, title, note_type=note_type, tags=tag_list, owner=owner, body=body_text
        )
    if existing_id is not None and not _output.is_quiet(ctx):
        # Advisory, stderr-only — never inside the --json payload (design.md
        # "Infrastructure on stderr, never in JSON payloads"); --quiet hides it.
        typer.echo(f"note new: duplicate title, also used by {existing_id}", err=True)
    _output.emit_mutation(
        ctx, obj_id=note.id, updated=note.updated, verb="created", fields={"type": note.type}
    )


@note_app.command("append")
def append_command(
    ctx: typer.Context,
    target: str = typer.Argument(..., help="Note id or slug."),
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
    """Append text to a note's body (optionally under a section / timestamped)."""
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        # The stamp names the acting agent — global --owner override, else the
        # configured identity (agent-usability's `--owner` role 5: the body stamp).
        actor = getattr(ctx.obj, "owner", None) or config.agent
        note = append_note(config, target, text, section=section, timestamp=timestamp, actor=actor)
    _output.emit_mutation(
        ctx, obj_id=note.id, updated=note.updated, verb="appended", fields={"type": note.type}
    )


@note_app.command("update")
def update_command(
    ctx: typer.Context,
    target: str = typer.Argument(..., help="Note id or slug."),
    tags: str | None = typer.Option(None, "--tags", help=TAG_SPEC_SEMANTICS),
    new_type: str | None = typer.Option(
        None,
        "--type",
        help=f"Move the note to this type's folder: {' | '.join(_NOTE_TYPES)}.",
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Update a note's fields (tags, type — moving its folder)."""
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        note = update_note(config, target, tags=tags, new_type=new_type)
    _output.emit_mutation(
        ctx, obj_id=note.id, updated=note.updated, verb="updated", fields={"type": note.type}
    )


def _meta_lines(note: Note) -> list[str]:
    """Render a note's canonical frontmatter fields as terse ``key: value`` lines.

    Timestamps go through :func:`mesh.cli._output._iso_z`, the same helper
    ``emit_mutation`` and ``hit_dict`` use — one field must not render ``Z`` on
    the JSON surfaces and ``+00:00`` here (core-hardening/4).
    """
    return [
        f"id: {note.id}",
        f"type: {note.type}",
        f"title: {note.title}",
        f"tags: {', '.join(note.tags)}",
        f"owner: {note.owner or ''}",
        f"created: {_output._iso_z(note.created)}",
        f"updated: {_output._iso_z(note.updated)}",
        f"related: {', '.join(note.related)}",
    ]


@note_app.command("get")
def get_command(
    ctx: typer.Context,
    target: str = typer.Argument(..., help="Note id or slug."),
    full: bool = typer.Option(False, "--full", help="Show the complete body (no truncation)."),
    meta_only: bool = typer.Option(False, "--meta-only", help="Frontmatter only; no body."),
    related: bool = typer.Option(False, "--related", help="Only the related list."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Read a note: frontmatter + a 200-char body preview by default."""
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        try:
            view = get_note(config, target)
        except ValidationError:
            # A mesh-id file with malformed/incomplete frontmatter is unreadable
            # as a note; treat it as not-found rather than crashing.
            raise NoteNotFoundError(target) from None

    if _output.is_quiet(ctx):
        typer.echo(view.note.id)
        return

    as_json = _output.is_json(ctx)
    if related:
        typer.echo(
            json.dumps({"related": view.note.related}) if as_json else "\n".join(view.note.related)
        )
        return

    if as_json:
        obj = view.note.model_dump(mode="json")
        if not meta_only:
            obj["body"] = _output.preview(view.body, full)
        typer.echo(json.dumps(obj))
        return

    lines = _meta_lines(view.note)
    if not meta_only:
        lines += ["", _output.preview(view.body, full)]
    typer.echo("\n".join(lines))


@note_app.command("list")
def list_command(
    ctx: typer.Context,
    tags: str | None = typer.Option(None, "--tags", help="Comma-separated tag filter (AND)."),
    any_tag: bool = typer.Option(False, "--any-tag", help="Switch --tags to OR semantics."),
    owner: str | None = typer.Option(None, "--owner", help="Filter by exact owner."),
    note_type: str | None = typer.Option(None, "--type", help="Filter by note type."),
    since: str | None = typer.Option(None, "--since", help="Recency: 7d or an ISO date."),
    sort: str = typer.Option("updated", "--sort", help="updated | created | title."),
    limit: int = typer.Option(20, "--limit", help="Cap the number of results."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """List mesh notes (id-bearing files only) with filters, sort and limit."""
    config = load_config()
    _, _, owner = _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet, owner=owner)
    tag_list = [t.strip() for t in tags.split(",") if t.strip()] if tags else None
    with cli_errors():
        # Served from the daemon's warm index when it is up, from the identical
        # on-disk walk when it is down — one predicate, either way.
        views = DaemonClient().note_list(
            config,
            tags=tag_list,
            any_tag=any_tag,
            owner=owner,
            note_type=note_type,
            since=since,
            sort=sort,
            limit=limit,
        )

    if _output.is_quiet(ctx):
        for view in views:
            typer.echo(view.note.id)
        return
    if _output.is_json(ctx):
        typer.echo(json.dumps([_output.to_json_obj(view.note) for view in views]))
        return
    for view in views:
        typer.echo(f"{view.note.id}  {view.note.type}  {view.note.title}")


def _is_tty() -> bool:
    """Whether stdin is an interactive terminal (indirection so tests can fake it)."""
    return sys.stdin.isatty()


@note_app.command("delete")
def delete_command(
    ctx: typer.Context,
    target: str = typer.Argument(..., help="Note id or slug."),
    force: bool = typer.Option(
        False, "--force", help="Skip the confirmation prompt and delete immediately."
    ),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
) -> None:
    """Hard-delete a note. Prompts on a TTY; refuses on a machine path without --force."""
    config = load_config()
    _output.coalesce_flags(ctx, json_out=json_out, quiet=quiet)
    with cli_errors():
        note_id = resolve_slug(config, target)
        if not force:
            _output.refuse_delete_if_non_interactive(ctx, tty=_is_tty())
            # click renders "Delete <id>? [y/N]: "; declining aborts (exit 1).
            typer.confirm(f"Delete {note_id}?", abort=True)
        delete_note(config, note_id)

    if _output.is_quiet(ctx):
        typer.echo(note_id)
    elif _output.is_json(ctx):
        typer.echo(json.dumps({"id": note_id, "deleted": True}))
    else:
        typer.echo(f"deleted {note_id}")
