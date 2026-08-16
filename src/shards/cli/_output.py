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

**The R6 flag contract** (root tech.md § Surface C): ``--json`` / ``--quiet`` /
``--owner`` are accepted on either side of the command name, with identical
effect, on every non-admin command. The root callback parses the *global*
side; a leaf command that redeclares one of these flags for the *local* side
calls :func:`coalesce_flags` once, early, so every later read through
``is_json`` / ``is_quiet`` / ``emit_mutation`` / ``refuse_delete_if_non_interactive``
sees the merged value with no further plumbing. This is the one shared
coalescing implementation — ``note``, ``task``, ``search``, and the session
lenses (``cli/session.py``) all call it rather than each hand-rolling the
OR-with-``ctx.obj`` logic. ``admin.py``'s own private ``_json``/``_quiet``
booleans (core-hardening/8) were the same one-line read as :func:`is_json` /
:func:`is_quiet`, so they were collapsed into these; ``admin.py``'s ``_emit``/
``_notice`` stay put — they answer to admin's own ``{payload, human}`` shape
(``daemon start|stop|status``, ``init``, no per-target id), a different
contract from :func:`emit_mutation`'s ``{obj_id, updated, verb, fields}``
note/task shape, and unifying the two would need a discriminator between
them.
"""

from __future__ import annotations

import json
from collections.abc import Callable
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


def coalesce_flags(
    ctx: typer.Context,
    *,
    json_out: bool = False,
    quiet: bool = False,
    owner: str | None = None,
) -> tuple[bool, bool, str | None]:
    """Fold a leaf command's own ``--json``/``--quiet``/``--owner`` into ``ctx.obj``.

    A flag given on either side of the command name takes effect — the local
    value wins when the caller gave one; otherwise the root callback's global
    value (parsed ahead of the command name) applies. Call this once, early,
    in any leaf command that redeclares these flags for local-side parity;
    everything downstream (:func:`is_json`, :func:`is_quiet`,
    :func:`emit_mutation`, :func:`refuse_delete_if_non_interactive`) reads
    ``ctx.obj`` and so sees the merged value automatically. Returns the three
    coalesced values too, for callers that need them as plain locals (e.g. to
    pass ``owner`` into ``create_note``/``create_task``).

    A command with no local ``--owner`` of its own simply omits the
    ``owner=`` kwarg — the global value (if any) round-trips through
    ``ctx.obj`` unchanged.
    """
    ctx.obj.json = json_out or is_json(ctx)
    ctx.obj.quiet = quiet or is_quiet(ctx)
    ctx.obj.owner = owner if owner is not None else getattr(ctx.obj, "owner", None)
    return ctx.obj.json, ctx.obj.quiet, ctx.obj.owner


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


def to_json_obj(model: Any) -> dict[str, object]:
    """Render a validated Note/Task model to its ``--json`` list-row dict.

    Both ``note list --json`` and ``task list --json`` dump the whole validated
    model verbatim — the one-line shape ``NoteView``/``TaskView`` differ only in
    which field (``.note``/``.task``) wraps, so the caller unwraps the view
    first and this stays a plain ``model_dump``.
    """
    return model.model_dump(mode="json")


def emit_entries(
    ctx: typer.Context,
    entries: list[dict[str, Any]],
    render: Callable[[dict[str, Any]], str],
) -> None:
    """Report a list of dict rows per the active global output flags.

    ``--json`` → the raw list, ``json.dumps``'d whole. ``--quiet`` → each row's
    ``id`` field, one per line. Otherwise ``render(entry)`` is called once per
    row and the result echoed. The per-row text shape is the *only* thing that
    varies across ``recent-activity`` / ``session-start`` / ``build-context``
    (a tab-separated column set, different per command), so it comes in as a
    plain callable rather than a strategy object with internal branches — the
    same collapse :func:`emit_mutation` already made for its ``fields`` dict.

    Not used by ``graph`` / ``project``: their ``--json`` payload is a single
    object (``{seed, nodes, edges}`` / ``{project, tasks}``), not a row list,
    and their text rendering is either pre-built lines or a two-part
    project-then-tasks walk — forcing them through this list-shaped helper
    would need a payload-shape discriminator, which is the duplication the DRY
    filter rejects, not the one it merges.
    """
    if is_json(ctx):
        typer.echo(json.dumps(entries))
        return
    if is_quiet(ctx):
        for entry in entries:
            typer.echo(str(entry.get("id", "")))
        return
    for entry in entries:
        typer.echo(render(entry))


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
