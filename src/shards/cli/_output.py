"""Shared presentation helpers for the CLI verbs.

The command modules (``note`` / ``task`` / ``session``) stay thin: reading the
global ``--quiet`` / ``--json`` flags off ``ctx.obj``, truncating body previews,
reporting a mutated object, and the machine-path delete guard all live here so
``note`` and ``task`` never each re-implement them.

Every helper is pure presentation — no disk, no network — and honours the flags
the root callback stashes on ``ctx.obj``. The interactive-tty check stays a
module-level seam in each command module (the delete tests fake it by path), so
:func:`refuse_delete_if_non_interactive` takes the tty result as an argument
rather than probing stdin itself.
"""

from __future__ import annotations

import json
from datetime import datetime
from typing import Any

import typer

PREVIEW_CHARS = 200


def _iso_z(value: datetime) -> str:
    """Render a UTC-aware datetime with a ``Z`` suffix instead of ``+00:00``.

    Mirrors :func:`shards.schemas.note._iso_z` — kept as a local one-liner per
    the DRY-filter convention (root tech.md § Duplication) rather than a shared
    import across an unrelated module boundary.
    """
    text = value.isoformat()
    return f"{text[:-6]}Z" if text.endswith("+00:00") else text


def is_quiet(ctx: typer.Context) -> bool:
    """Whether ``--quiet`` (id-only output) is active."""
    return bool(getattr(ctx.obj, "quiet", False))


def is_json(ctx: typer.Context) -> bool:
    """Whether ``--json`` (machine output) is active."""
    return bool(getattr(ctx.obj, "json", False))


def is_machine(ctx: typer.Context) -> bool:
    """A non-interactive path: ``--json`` or ``--quiet`` was given."""
    return is_json(ctx) or is_quiet(ctx)


def preview(body: str, full: bool) -> str:
    """The body, or its first :data:`PREVIEW_CHARS` characters unless ``--full``."""
    return body if full else body[:PREVIEW_CHARS]


def emit_mutation(
    ctx: typer.Context,
    *,
    obj_id: str,
    updated: datetime,
    verb: str,
    fields: dict[str, Any],
) -> None:
    """Report a mutated note/task per the active global output flags.

    ``--quiet`` → the id alone; ``--json`` → ``{"id", **fields, "updated"}`` where
    ``fields`` carries the type discriminator (``{"type": …}`` for a note,
    ``{"status": …}`` for a task); otherwise a terse ``"<verb> <id>"`` line.
    """
    if is_quiet(ctx):
        typer.echo(obj_id)
        return
    if is_json(ctx):
        typer.echo(json.dumps({"id": obj_id, **fields, "updated": _iso_z(updated)}))
        return
    typer.echo(f"{verb} {obj_id}")


def refuse_delete_if_non_interactive(ctx: typer.Context, *, tty: bool) -> None:
    """Raise ``ValueError`` on a machine path (``--json``/``--quiet``) or non-tty stdin.

    Mapped to exit 2 by the CLI boundary mapper (:func:`shards.cli._errors.cli_errors`)
    — callers must invoke this from inside a ``with cli_errors():`` block. ``tty``
    is injected — each command passes its own ``_is_tty()`` seam — so the
    module-level tty fake in the delete tests keeps working while the guard itself
    lives in one place.
    """
    if is_machine(ctx) or not tty:
        raise ValueError("refusing to delete on a non-interactive path; pass --force to confirm")
