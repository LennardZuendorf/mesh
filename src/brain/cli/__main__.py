"""brain CLI entry point.

Thin Typer app. Three verbs (`note`, `task`, `search`) plus human-only admin
(`daemon`, `status`, `reindex`) and the Phase-2 `session-start` lens. Sub-app
command bodies land with their respective feature units; this module only wires
the surface together so `brain --help` is always honest about the shape.
"""

from __future__ import annotations

import typer

from brain import __version__

app = typer.Typer(
    name="brain",
    help="Three verbs, one daemon, one folder — notes + search = memory, tasks = handoff.",
    no_args_is_help=True,
    add_completion=False,
)

note_app = typer.Typer(name="note", help="Capture knowledge as Markdown.", no_args_is_help=True)
task_app = typer.Typer(name="task", help="Coordinate work as claimable task files.", no_args_is_help=True)

app.add_typer(note_app, name="note")
app.add_typer(task_app, name="task")


@app.callback(invoke_without_command=True)
def _root(
    version: bool = typer.Option(False, "--version", help="Show version and exit."),
) -> None:
    """brain — see `brain <verb> --help`."""
    if version:
        typer.echo(__version__)
        raise typer.Exit(0)


if __name__ == "__main__":  # pragma: no cover
    app()
