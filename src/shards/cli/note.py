"""``shards note`` command surface.

Command bodies for the ``note`` verb live here (per the entry-point contract in
``cli/__main__``, which only wires sub-apps together). This unit lands ``append``
and ``update``; sibling units add ``new``, ``get``, ``list``, ``delete`` to the
same :data:`note_app`.

Output honours the global flags stashed on ``ctx.obj``: ``--quiet`` prints the id
only, ``--json`` prints a machine object, otherwise a terse human line. Resolution
failures map to the shared exit codes (3 not-found, 2 validation/ambiguous).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import get_args

import typer
from pydantic import ValidationError

from shards.cli import _output
from shards.core.notes import (
    AmbiguousSlugError,
    NoteNotFoundError,
    NoteView,
    append_note,
    create_note,
    delete_note,
    get_note,
    list_notes,
    resolve_slug,
    update_note,
)
from shards.schemas.config import load_config
from shards.schemas.note import Note, NoteType

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
    neither source given, refuses (exit 2) rather than launching ``$EDITOR``.
    """
    if body is not None:
        return body
    if body_file is not None:
        try:
            return Path(body_file).read_text(encoding="utf-8")
        except OSError as exc:
            typer.echo(f"cannot read --file {body_file}: {exc}", err=True)
            raise typer.Exit(2) from None

    if _output.is_machine(ctx) or not _is_tty():
        typer.echo(
            "no body: pass --body or --file on a non-interactive path",
            err=True,
        )
        raise typer.Exit(2)
    return _edit_body()


@note_app.command("new")
def new_command(
    ctx: typer.Context,
    title: str = typer.Argument(..., help="Note title."),
    note_type: str = typer.Option("note", "--type", help="note | log | decision | reference."),
    tags: str | None = typer.Option(None, "--tags", help="Comma-separated tags."),
    owner: str | None = typer.Option(
        None, "--owner", help="Owner identity (must be in [tasks].collections)."
    ),
    body: str | None = typer.Option(None, "--body", help="Note body text."),
    body_file: str | None = typer.Option(None, "--file", help="Read the body from this file."),
) -> None:
    """Create a note (routed by type); body from --body, --file, or $EDITOR (TTY)."""
    config = load_config()
    if note_type not in _NOTE_TYPES:
        typer.echo(f"invalid note type: {note_type}", err=True)
        raise typer.Exit(2)

    body_text = _resolve_body(ctx, body, body_file)
    tag_list = [t.strip() for t in tags.split(",") if t.strip()] if tags else []
    try:
        note = create_note(
            config, title, note_type=note_type, tags=tag_list, owner=owner, body=body_text
        )
    except ValidationError:
        typer.echo("invalid note", err=True)
        raise typer.Exit(2) from None
    except ValueError as exc:
        # Owner outside [tasks].collections (enforced once in core, before any write).
        typer.echo(str(exc), err=True)
        raise typer.Exit(2) from None
    _output.emit_mutation(ctx, obj_id=note.id, updated=note.updated, verb="created", fields={"type": note.type})


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
) -> None:
    """Append text to a note's body (optionally under a section / timestamped)."""
    config = load_config()
    try:
        note = append_note(config, target, text, section=section, timestamp=timestamp)
    except NoteNotFoundError:
        typer.echo(f"note not found: {target}", err=True)
        raise typer.Exit(3) from None
    except AmbiguousSlugError:
        typer.echo(f"ambiguous slug: {target}", err=True)
        raise typer.Exit(2) from None
    _output.emit_mutation(ctx, obj_id=note.id, updated=note.updated, verb="appended", fields={"type": note.type})


@note_app.command("update")
def update_command(
    ctx: typer.Context,
    target: str = typer.Argument(..., help="Note id or slug."),
    tags: str | None = typer.Option(
        None, "--tags", help="Delta (+x,-y) or replacement (x,y) tag list."
    ),
    new_type: str | None = typer.Option(
        None, "--type", help="Move the note to this type's folder."
    ),
) -> None:
    """Update a note's fields (tags, type — moving its folder)."""
    config = load_config()
    try:
        note = update_note(config, target, tags=tags, new_type=new_type)
    except NoteNotFoundError:
        typer.echo(f"note not found: {target}", err=True)
        raise typer.Exit(3) from None
    except AmbiguousSlugError:
        typer.echo(f"ambiguous slug: {target}", err=True)
        raise typer.Exit(2) from None
    except ValueError as exc:
        typer.echo(str(exc), err=True)
        raise typer.Exit(2) from None
    _output.emit_mutation(ctx, obj_id=note.id, updated=note.updated, verb="updated", fields={"type": note.type})


def _meta_lines(note: Note) -> list[str]:
    """Render a note's canonical frontmatter fields as terse ``key: value`` lines."""
    return [
        f"id: {note.id}",
        f"type: {note.type}",
        f"title: {note.title}",
        f"tags: {', '.join(note.tags)}",
        f"owner: {note.owner or ''}",
        f"created: {note.created.isoformat()}",
        f"updated: {note.updated.isoformat()}",
        f"related: {', '.join(note.related)}",
    ]


@note_app.command("get")
def get_command(
    ctx: typer.Context,
    target: str = typer.Argument(..., help="Note id or slug."),
    full: bool = typer.Option(False, "--full", help="Show the complete body (no truncation)."),
    meta_only: bool = typer.Option(False, "--meta-only", help="Frontmatter only; no body."),
    related: bool = typer.Option(False, "--related", help="Only the related list."),
) -> None:
    """Read a note: frontmatter + a 200-char body preview by default."""
    config = load_config()
    try:
        view = get_note(config, target)
    except NoteNotFoundError:
        typer.echo(f"note not found: {target}", err=True)
        raise typer.Exit(3) from None
    except AmbiguousSlugError as exc:
        typer.echo(f"ambiguous slug: {target}: {', '.join(exc.ids)}", err=True)
        raise typer.Exit(2) from None
    except ValidationError:
        # A shards-id file with malformed/incomplete frontmatter is unreadable as a
        # note; treat it as not-found rather than crashing with a traceback.
        typer.echo(f"note not found: {target}", err=True)
        raise typer.Exit(3) from None

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
) -> None:
    """List shards notes (id-bearing files only) with filters, sort and limit."""
    config = load_config()
    tag_list = [t.strip() for t in tags.split(",") if t.strip()] if tags else None
    try:
        views = list_notes(
            config,
            tags=tag_list,
            any_tag=any_tag,
            owner=owner,
            note_type=note_type,
            since=since,
            sort=sort,
            limit=limit,
        )
    except ValueError as exc:
        typer.echo(str(exc), err=True)
        raise typer.Exit(2) from None

    if _output.is_quiet(ctx):
        for view in views:
            typer.echo(view.note.id)
        return
    if _output.is_json(ctx):
        typer.echo(json.dumps([_list_obj(view) for view in views]))
        return
    for view in views:
        typer.echo(f"{view.note.id}  {view.note.type}  {view.note.title}")


def _list_obj(view: NoteView) -> dict[str, object]:
    return view.note.model_dump(mode="json")


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
) -> None:
    """Hard-delete a note. Prompts on a TTY; refuses on a machine path without --force."""
    config = load_config()
    try:
        note_id = resolve_slug(config, target)
    except NoteNotFoundError:
        typer.echo(f"note not found: {target}", err=True)
        raise typer.Exit(3) from None
    except AmbiguousSlugError as exc:
        typer.echo(f"ambiguous slug: {target}: {', '.join(exc.ids)}", err=True)
        raise typer.Exit(2) from None

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
