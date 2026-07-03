"""``brain note`` command surface.

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

import typer

from brain.core.notes import (
    AmbiguousSlugError,
    NoteNotFoundError,
    NoteView,
    append_note,
    delete_note,
    get_note,
    list_notes,
    resolve_slug,
    update_note,
)
from brain.schemas.config import load_config
from brain.schemas.note import Note

note_app = typer.Typer(
    name="note",
    help="Capture knowledge as Markdown.",
    no_args_is_help=True,
)

_PREVIEW_CHARS = 200


def _emit(ctx: typer.Context, note: Note, verb: str) -> None:
    """Report a mutated note per the active global output flags."""
    opts = ctx.obj
    if getattr(opts, "quiet", False):
        typer.echo(note.id)
        return
    if getattr(opts, "json", False):
        typer.echo(
            json.dumps({"id": note.id, "type": note.type, "updated": note.updated.isoformat()})
        )
        return
    typer.echo(f"{verb} {note.id}")


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
    _emit(ctx, note, "appended")


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
    _emit(ctx, note, "updated")


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


def _preview(body: str, full: bool) -> str:
    return body if full else body[:_PREVIEW_CHARS]


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

    opts = ctx.obj
    if getattr(opts, "quiet", False):
        typer.echo(view.note.id)
        return

    as_json = getattr(opts, "json", False)
    if related:
        typer.echo(
            json.dumps({"related": view.note.related}) if as_json else "\n".join(view.note.related)
        )
        return

    if as_json:
        obj = view.note.model_dump(mode="json")
        if not meta_only:
            obj["body"] = _preview(view.body, full)
        typer.echo(json.dumps(obj))
        return

    lines = _meta_lines(view.note)
    if not meta_only:
        lines += ["", _preview(view.body, full)]
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
    """List brain notes (id-bearing files only) with filters, sort and limit."""
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

    opts = ctx.obj
    if getattr(opts, "quiet", False):
        for view in views:
            typer.echo(view.note.id)
        return
    if getattr(opts, "json", False):
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
    opts = ctx.obj
    try:
        note_id = resolve_slug(config, target)
    except NoteNotFoundError:
        typer.echo(f"note not found: {target}", err=True)
        raise typer.Exit(3) from None
    except AmbiguousSlugError as exc:
        typer.echo(f"ambiguous slug: {target}: {', '.join(exc.ids)}", err=True)
        raise typer.Exit(2) from None

    if not force:
        machine = getattr(opts, "json", False) or getattr(opts, "quiet", False)
        if machine or not _is_tty():
            typer.echo(
                "refusing to delete on a non-interactive path; pass --force to confirm",
                err=True,
            )
            raise typer.Exit(2)
        # click renders "Delete <id>? [y/N]: "; declining aborts (exit 1).
        typer.confirm(f"Delete {note_id}?", abort=True)

    delete_note(config, note_id)

    if getattr(opts, "quiet", False):
        typer.echo(note_id)
    elif getattr(opts, "json", False):
        typer.echo(json.dumps({"id": note_id, "deleted": True}))
    else:
        typer.echo(f"deleted {note_id}")
