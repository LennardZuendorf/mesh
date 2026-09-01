"""mesh CLI entry point.

Thin Typer app. Three verbs (`note`, `task`, `search`) plus human-only admin
(`init`, `daemon`, `status`, `reindex`) and the Phase-2 session lenses. Sub-app command
bodies live in their feature modules; this module only wires the surface together
so `mesh --help` is always honest about the shape.

Wiring is **eager**: every verb/admin/session module imports at load time and is
attached to the root group in one pass below. core-hardening/8 deleted this
module's former ``_LazyCommandGroup`` (a ``get_command`` override that deferred
each verb's import until Click actually resolved it) after measuring that it
bought ~6ms of a ~70ms cold start while coupling to typer's private vendored
``_click`` module for its type annotations — and per
``tests/test_startup_guard.py``, none of the sibling verb modules import any of
the four watched heavy deps (watchdog/fastmcp/rich/pydantic), so the fast-path
guard never actually depended on the laziness. See
``.spec/features/core-hardening/tech.md`` § Duplication, row 10.

Global flags (`--json`, `--quiet`, `--owner`, `--mine`) are parsed here and
stashed on the Typer context (`ctx.obj`) so every command inherits them.

**R6 flag contract** (`.spec/features/agent-usability/tech.md` § Surface C):
every non-admin leaf/verb command also redeclares `--json`/`--quiet` (and,
where meaningful, `--owner`) as its own local option and coalesces it with
these globals via `mesh.cli._output.coalesce_flags`, so a caller can give a
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

from dataclasses import dataclass
from typing import Any

import typer
from typer.core import TyperGroup

from mesh import __version__
from mesh.cli._errors import cli_errors
from mesh.cli.admin import (
    daemon_app,
    init_command,
    reindex_command,
    status_command,
)
from mesh.cli.note import note_app
from mesh.cli.search import search_app
from mesh.cli.session import (
    build_context_command,
    graph_command,
    project_command,
    recent_activity_command,
    session_start_command,
)
from mesh.cli.task import task_app

# Verb / admin sub-apps (each a ``typer.Typer``): ``name -> the sub-app``.
_SUBAPPS: dict[str, typer.Typer] = {
    "note": note_app,
    "task": task_app,
    "search": search_app,
    "daemon": daemon_app,
}
# Leaf commands (plain callables Typer wraps): ``name -> (function, help)``.
_LEAVES: dict[str, tuple[Any, str]] = {
    "init": (
        init_command,
        "Write ~/.mesh/config.toml (or $MESH_CONFIG_PATH); --force to overwrite.",
    ),
    "status": (
        status_command,
        "Report vault health (counts, freshness, links, locks).",
    ),
    "reindex": (
        reindex_command,
        "Rebuild the search index (delegates to indexed).",
    ),
    "recent-activity": (
        recent_activity_command,
        "List recent vault changes (newest first; --since, --mine).",
    ),
    "build-context": (
        build_context_command,
        "Expand the related graph around a seed id (BFS to --depth).",
    ),
    "graph": (
        graph_command,
        "Query what's connected to a seed id (tree, or JSON nodes+edges).",
    ),
    "project": (
        project_command,
        "Show a project note and the tasks scoped to it (read-only lens).",
    ),
    "session-start": (
        session_start_command,
        "Warm-start payload: my tasks + mentions of me + recent activity.",
    ),
}
# Display order in ``mesh --help`` (matches the pre-decomposition wiring):
# sub-apps first, then leaf commands, each in their dict's declaration order.
_ORDER: tuple[str, ...] = (*_SUBAPPS, *_LEAVES)


class _RootGroup(TyperGroup):
    """Root group: the fixed ``--help`` order plus the one CLI-boundary mapper.

    Click's default group sorts ``list_commands`` alphabetically; overriding it
    keeps ``mesh --help`` in the declared verbs → admin → session-lenses order
    (:data:`_ORDER`) instead.

    ``invoke`` wraps the whole dispatch chain in the one CLI boundary mapper
    (agent-usability/5). Every leaf command already opens its own ``with
    cli_errors():`` around the domain call it makes, but ``load_config()`` runs
    *before* that block in every one of them (a bare call at the top of the
    command body) — a :class:`~mesh.schemas.config.ConfigMissingError` raised
    there would otherwise walk straight out of Click's dispatch chain
    uncaught, since nothing downstream is holding it yet. Wrapping the *whole*
    invocation here — which recurses through every nested sub-app's own
    ``TyperGroup.invoke`` (``note``/``task``/``search``/``daemon``), since
    ``super().invoke()`` is what performs that recursion — closes the gap in
    the one place, without scattering a second ``with cli_errors():`` above
    every ``load_config()`` call site. A ``typer.Exit`` raised by an error
    ``cli_errors()`` already resolved deeper in the stack passes through
    unchanged (it is a ``RuntimeError``, never a ``MeshError``/``ValueError``/
    ``OSError``), so this never double-handles anything.

    Untyped ``ctx`` deliberately: the real parameter type is typer's own
    private vendored ``typer._click.core.Context`` (typer 0.26 ships its own
    click fork), and importing that just to annotate an override is the private-
    API coupling core-hardening/8 removed the rest of this class for.
    """

    def list_commands(self, ctx: Any) -> list[str]:
        return list(_ORDER)

    def invoke(self, ctx: Any) -> Any:
        with cli_errors():
            return super().invoke(ctx)


@dataclass
class GlobalOptions:
    """Cross-cutting flags, propagated to sub-commands via ``ctx.obj``."""

    json: bool = False
    quiet: bool = False
    owner: str | None = None
    mine: bool = False


app = typer.Typer(
    cls=_RootGroup,
    name="mesh",
    help="Three verbs, one folder, one mesh — notes + search = shared memory, tasks = coordination + handoff.",
    no_args_is_help=True,
    add_completion=False,
)

for _subapp in _SUBAPPS.values():
    app.add_typer(_subapp)
for _name, (_func, _help) in _LEAVES.items():
    app.command(name=_name, help=_help)(_func)


@app.callback(invoke_without_command=True)
def _root(
    ctx: typer.Context,
    version: bool = typer.Option(False, "--version", help="Show version and exit."),
    json_out: bool = typer.Option(False, "--json", help="Machine-readable JSON output."),
    quiet: bool = typer.Option(False, "--quiet", help="IDs only; suppress stderr notes."),
    owner: str | None = typer.Option(None, "--owner", help="Act as this agent identity."),
    mine: bool = typer.Option(False, "--mine", help="Filter to owner or claimed_by == me."),
) -> None:
    """mesh — see `mesh <verb> --help`."""
    if version:
        typer.echo(__version__)
        raise typer.Exit(0)
    ctx.obj = GlobalOptions(json=json_out, quiet=quiet, owner=owner, mine=mine)


if __name__ == "__main__":  # pragma: no cover
    app()
