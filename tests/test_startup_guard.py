"""Startup guard (cli-toolset-rework/2) — heavy deps stay off the CLI fast path.

The "instant CLI" mandate (root ``tech.md`` § Performance, Invariant 6) rests on
the note/task import path pulling only what it uses. This guard is a
*deterministic import-membership* check, not a flaky wall-clock threshold: it
asserts that exercising the ``mesh note`` / ``mesh task`` fast path never
imports the heavy modules —

* ``watchdog`` — daemon-only; its fsevents C-extension must stay lazy;
* ``fastmcp`` — lives behind the separate ``mesh-mcp`` console script;
* ``rich`` — pulled only to render ``--help`` / error output;
* ``pydantic`` — removed from the schema layer this unit (swapped to msgspec);
  a regression that reintroduces it on the CLI path is exactly what this catches.

If a future edit re-introduces an eager heavy import, CI fails here rather than
in a distant profiling pass.
"""

from __future__ import annotations

import subprocess
import sys

# Imports only the note/task fast path, then reports which heavy modules leaked.
# Runs in a *fresh* interpreter on purpose: the pytest process itself imports
# watchdog (daemon tests), fastmcp (mcp tests) and rich, so an in-process
# ``sys.modules`` check would be meaningless.
_PROBE = """
import sys
import mesh.cli.__main__   # the entry point every `mesh <verb>` invocation loads
import mesh.cli.note       # `mesh note ...`
import mesh.cli.task       # `mesh task ...`
heavy = ("watchdog", "fastmcp", "rich", "pydantic")
leaked = sorted(name for name in heavy if name in sys.modules)
sys.stdout.write(",".join(leaked))
"""


def test_note_task_fast_path_excludes_heavy_modules() -> None:
    result = subprocess.run(
        [sys.executable, "-c", _PROBE],
        capture_output=True,
        text=True,
        check=True,
    )
    leaked = [name for name in result.stdout.strip().split(",") if name]
    assert leaked == [], (
        f"heavy modules imported on the note/task fast path: {leaked}. "
        "Keep watchdog/fastmcp lazy (daemon/mcp only), rich on the help path, "
        "and the schema layer on msgspec (not pydantic)."
    )
