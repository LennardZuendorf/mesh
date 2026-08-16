"""shards CLI entry point.

Thin Typer app. Three verbs (`note`, `task`, `search`) plus human-only admin
(`init`, `daemon`, `status`, `reindex`) and the Phase-2 session lenses. Sub-app command
bodies live in their feature modules; this module only wires the surface together
so `shards --help` is always honest about the shape.

Wiring is **lazy**: :class:`_LazyCommandGroup` imports a verb's module only when
that verb is actually invoked, so `shards note new` never pulls the sibling
`search` / `admin` / `session` modules (invariant 6 hygiene — every command still
pays the shared schema floor, but not its siblings' import cost). `shards --help`
lists every command because rendering the summary resolves each one.

Global flags (`--json`, `--quiet`, `--owner`, `--mine`) are parsed here and
stashed on the Typer context (`ctx.obj`) so every command inherits them.

**R6 flag contract** (`.spec/features/agent-usability/tech.md` § Surface C):
every non-admin leaf/verb command also redeclares `--json`/`--quiet` (and,
where meaningful, `--owner`) as its own local option and coalesces it with
these globals via `shards.cli._output.coalesce_flags`, so a caller can give a
flag on either side of the command name with identical effect. `--owner`
means one thing everywhere it is accepted: the identity this invocation acts
as — honoured on creation (defaults the written `owner`) and on filters
(`note list`/`task list`/`search`), unchanged on `task claim`/`task
release`/`session-start` (already read it), and deliberately *not* folded
into `task update`'s reassignment `--owner` (opt-in only — see
`cli/task.py::update_command`). Admin commands (`init`/`daemon`/`status`/`reindex`)
are out of scope for this contract; their reader is a later unit's refactor.
"""

from __future__ import annotations

import importlib
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

import typer
from typer.core import TyperGroup

from shards import __version__
from shards.cli._errors import cli_errors

if TYPE_CHECKING:
    # Match ``TyperGroup.get_command``'s own annotations, which use typer's
    # vendored click (typer 0.26 ships its own copy under ``typer._click``).
    from typer._click.core import Command, Context

# Verb / admin sub-apps (each a ``typer.Typer``): ``name -> (module, attribute)``.
_SUBAPPS: dict[str, tuple[str, str]] = {
    "note": ("shards.cli.note", "note_app"),
    "task": ("shards.cli.task", "task_app"),
    "search": ("shards.cli.search", "search_app"),
    "daemon": ("shards.cli.admin", "daemon_app"),
}
# Leaf commands (plain callables Typer wraps): ``name -> (module, function, help)``.
_LEAVES: dict[str, tuple[str, str, str]] = {
    "init": (
        "shards.cli.admin",
        "init_command",
        "Write ~/.shards/config.toml (or $SHARDS_CONFIG_PATH); --force to overwrite.",
    ),
    "status": (
        "shards.cli.admin",
        "status_command",
        "Report vault health (counts, freshness, links, locks).",
    ),
    "reindex": (
        "shards.cli.admin",
        "reindex_command",
        "Rebuild the search index (delegates to indexed).",
    ),
    "recent-activity": (
        "shards.cli.session",
        "recent_activity_command",
        "List recent vault changes (newest first; --since, --mine).",
    ),
    "build-context": (
        "shards.cli.session",
        "build_context_command",
        "Expand the related graph around a seed id (BFS to --depth).",
    ),
    "graph": (
        "shards.cli.session",
        "graph_command",
        "Query what's connected to a seed id (tree, or JSON nodes+edges).",
    ),
    "project": (
        "shards.cli.session",
        "project_command",
        "Show a project note and the tasks scoped to it (read-only lens).",
    ),
    "session-start": (
        "shards.cli.session",
        "session_start_command",
        "Warm-start payload: my tasks + mentions of me + recent activity.",
    ),
}
# Display order in ``shards --help`` (matches the pre-decomposition wiring):
# sub-apps first, then leaf commands, each in their dict's declaration order.
_ORDER: tuple[str, ...] = (*_SUBAPPS, *_LEAVES)


class _LazyCommandGroup(TyperGroup):
    """Root group that imports a verb's module only when the verb is resolved.

    Keeps the note/task fast path from pulling the sibling verb modules — a
    concrete invocation like ``shards note new`` imports only ``shards.cli.note``,
    while ``shards --help`` resolves every command to render its summary line.
    """

    def list_commands(self, ctx: Context) -> list[str]:
        return list(_ORDER)

    def invoke(self, ctx: Context) -> Any:
        """Wrap the whole dispatch chain in the one CLI boundary mapper (agent-usability/5).

        Every leaf command already opens its own ``with cli_errors():`` around
        the domain call it makes, but ``load_config()`` runs *before* that
        block in every one of them (a bare call at the top of the command
        body) — a :class:`~shards.schemas.config.ConfigMissingError` raised
        there would otherwise walk straight out of Click's dispatch chain
        uncaught, since nothing downstream is holding it yet. Wrapping the
        *whole* invocation here — which recurses through every nested
        sub-app's own ``TyperGroup.invoke`` (``note``/``task``/``search``/
        ``daemon``), since ``super().invoke()`` is what performs that
        recursion — closes the gap in the one place, without scattering a
        second ``with cli_errors():`` above every ``load_config()`` call site.
        A ``typer.Exit`` raised by an error ``cli_errors()`` already resolved
        deeper in the stack passes through unchanged (it is a ``RuntimeError``,
        never a ``ShardsError``/``ValueError``/``OSError``), so this never
        double-handles anything.
        """
        with cli_errors():
            return super().invoke(ctx)

    def get_command(self, ctx: Context, cmd_name: str) -> Command | None:
        if cmd_name in _SUBAPPS:
            module, attr = _SUBAPPS[cmd_name]
            return typer.main.get_command(getattr(importlib.import_module(module), attr))
        if cmd_name in _LEAVES:
            module, func, help_text = _LEAVES[cmd_name]
            leaf = typer.Typer(add_completion=False)
            leaf.command(name=cmd_name, help=help_text)(
                getattr(importlib.import_module(module), func)
            )
            # A single-command Typer collapses to a bare command; use it directly.
            command = typer.main.get_command(leaf)
            command.name = cmd_name
            return command
        return None


@dataclass
class GlobalOptions:
    """Cross-cutting flags, propagated to sub-commands via ``ctx.obj``."""

    json: bool = False
    quiet: bool = False
    owner: str | None = None
    mine: bool = False


app = typer.Typer(
    cls=_LazyCommandGroup,
    name="shards",
    help="Three verbs, one folder, one mesh — notes + search = shared memory, tasks = coordination + handoff.",
    no_args_is_help=True,
    add_completion=False,
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
    """shards — see `shards <verb> --help`."""
    if version:
        typer.echo(__version__)
        raise typer.Exit(0)
    ctx.obj = GlobalOptions(json=json_out, quiet=quiet, owner=owner, mine=mine)


def __getattr__(name: str) -> Any:
    """Expose the verb sub-apps (``note_app``, ``task_app``, …) as lazy attributes.

    The sub-apps are imported on demand rather than at module load, but callers
    and wiring introspection still reach the real singletons — ``main.task_app is
    task.task_app`` holds — without forcing an eager import of every verb.
    """
    for module, attr in _SUBAPPS.values():
        if name == attr:
            return getattr(importlib.import_module(module), attr)
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


if __name__ == "__main__":  # pragma: no cover
    app()
