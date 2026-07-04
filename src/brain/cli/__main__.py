"""brain CLI entry point.

Thin Typer app. Three verbs (`note`, `task`, `search`) plus human-only admin
(`daemon`, `status`, `reindex`) and the Phase-2 `session-start` lens. Sub-app
command bodies land with their respective feature units; this module only wires
the surface together so `brain --help` is always honest about the shape.

Global flags (`--json`, `--quiet`, `--owner`, `--mine`) are parsed here and
stashed on the Typer context (`ctx.obj`) so every command inherits them.
"""

from __future__ import annotations

from dataclasses import dataclass

import typer

from brain import __version__
from brain.cli.admin import daemon_app, reindex_command, status_command
from brain.cli.note import note_app
from brain.cli.search import search_app
from brain.cli.task import task_app


@dataclass
class GlobalOptions:
    """Cross-cutting flags, propagated to sub-commands via ``ctx.obj``."""

    json: bool = False
    quiet: bool = False
    owner: str | None = None
    mine: bool = False


app = typer.Typer(
    name="brain",
    help="Three verbs, one daemon, one folder — notes + search = memory, tasks = handoff.",
    no_args_is_help=True,
    add_completion=False,
)

app.add_typer(note_app, name="note")
app.add_typer(task_app, name="task")
app.add_typer(search_app, name="search")
app.add_typer(daemon_app, name="daemon")
app.command(name="status", help="Report vault health (counts, freshness, links, locks).")(
    status_command
)
app.command(name="reindex", help="Rebuild the search index (delegates to indexed).")(
    reindex_command
)


@app.callback(invoke_without_command=True)
def _root(
    ctx: typer.Context,
    version: bool = typer.Option(False, "--version", help="Show version and exit."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
    owner: str | None = typer.Option(None, "--owner", help="Act as this agent identity."),
    mine: bool = typer.Option(False, "--mine", help="Filter to owner or claimed_by == me."),
) -> None:
    """brain — see `brain <verb> --help`."""
    if version:
        typer.echo(__version__)
        raise typer.Exit(0)
    ctx.obj = GlobalOptions(json=json_out, quiet=quiet, owner=owner, mine=mine)


if __name__ == "__main__":  # pragma: no cover
    app()
