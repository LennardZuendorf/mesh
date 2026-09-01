"""agent-usability/3 — Tag mutation contract.

Acceptance coverage (brief: ``.superpowers/sdd/mesh-3track/agent-usability-3-brief.md``):

* **The silent-wipe regression, locked** — a note tagged ``["infra", "urgent", "q3"]``
  updated with ``tags="urgent"`` retains all three, driven end to end through the
  *registered* MCP tool (``server.app.call_tool``), not just the bare core function
  (``tests/notes/test_append_update.py`` / ``tests/tasks/test_new_update.py`` cover the
  core layer directly).
* **Delta still works** — ``+x,-y`` still adds and removes; removing an absent tag is a
  no-op.
* **Explicit replace, and only that path replaces** — ``=x,y`` replaces the whole list;
  neither the additive bare-list form nor the delta form ever does.
* **One sentence, three surfaces, asserted identical** — the same
  :data:`mesh.core.notes.TAG_SPEC_SEMANTICS` sentence appears verbatim in the MCP
  ``tags`` parameter description (``note_update``/``task_update``), the server-level
  ``instructions`` block, and the CLI ``--tags`` help (``note update``/``task update``) —
  so the four call sites cannot drift from each other.
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any

import pytest
import typer
from fastmcp.tools.base import ToolResult
from typer.core import TyperGroup, TyperOption

import mesh.mcp.server as server
from mesh.core.notes import TAG_SPEC_SEMANTICS
from mesh.core.tasks import create_task as core_create_task
from mesh.mcp.instructions import build_instructions
from mesh.schemas.config import Config, load_config

# --------------------------------------------------------------------------- #
# Fixtures                                                                     #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg(mesh_config: Path) -> Config:
    return load_config()


def _registered() -> dict[str, Any]:
    tools = asyncio.run(server.app.list_tools())
    return {tool.name: tool for tool in tools}


def _content(dispatched: ToolResult) -> dict[str, Any]:
    """Narrow ``ToolResult.structured_content`` (``dict[str, Any] | None``) for tests
    that know a given tool call always returns structured content."""
    assert dispatched.structured_content is not None
    return dispatched.structured_content


def _cli_tags_help(sub_app: typer.Typer, command: str) -> str:
    """The exact ``--tags`` help string typer stores for ``<sub_app> <command>``,
    read off the click ``Parameter`` directly — bypasses Rich's line-wrapped
    ``--help`` rendering, which would otherwise fracture the sentence across
    box-drawing lines and defeat a substring check."""
    click_group = typer.main.get_command(sub_app)
    assert isinstance(click_group, TyperGroup)
    click_command = click_group.commands[command]
    for param in click_command.params:
        if param.name == "tags":
            assert isinstance(param, TyperOption)
            assert param.help is not None
            return param.help
    raise AssertionError(f"no --tags option on {sub_app} {command}")


# --------------------------------------------------------------------------- #
# One sentence, three surfaces — cannot drift                                 #
# --------------------------------------------------------------------------- #


def test_semantics_sentence_identical_in_mcp_schema_note_update() -> None:
    props = _registered()["mesh_note_update"].parameters["properties"]
    assert props["tags"]["description"] == TAG_SPEC_SEMANTICS


def test_semantics_sentence_identical_in_mcp_schema_task_update() -> None:
    props = _registered()["mesh_task_update"].parameters["properties"]
    assert props["tags"]["description"] == TAG_SPEC_SEMANTICS


def test_semantics_sentence_identical_in_instructions_block(cfg: Config) -> None:
    block = build_instructions(cfg)
    assert TAG_SPEC_SEMANTICS in block


def test_semantics_sentence_identical_in_cli_note_update_help() -> None:
    from mesh.cli.note import note_app

    assert _cli_tags_help(note_app, "update") == TAG_SPEC_SEMANTICS


def test_semantics_sentence_identical_in_cli_task_update_help() -> None:
    from mesh.cli.task import task_app

    assert _cli_tags_help(task_app, "update") == TAG_SPEC_SEMANTICS


# --------------------------------------------------------------------------- #
# The silent-wipe regression, locked (MCP layer)                              #
# --------------------------------------------------------------------------- #


def test_mcp_note_update_bare_tags_is_additive_not_replace(cfg: Config, vault: Path) -> None:
    dispatched = asyncio.run(
        server.app.call_tool(
            "mesh_note_new",
            {"title": "Silent Wipe Regression", "tags": ["infra", "urgent", "q3"]},
        )
    )
    note_id = _content(dispatched)["id"]

    dispatched = asyncio.run(
        server.app.call_tool("mesh_note_update", {"target": note_id, "tags": "urgent"})
    )

    assert _content(dispatched)["tags"] == ["infra", "urgent", "q3"]


def test_mcp_task_update_bare_tags_is_additive_not_replace(cfg: Config, vault: Path) -> None:
    oracle = core_create_task(cfg, "Silent Wipe Regression")
    # Seed the tag list through the core layer (create_task takes tags too) so
    # this test exercises only the update path's semantics.
    dispatched = asyncio.run(
        server.app.call_tool(
            "mesh_task_new", {"title": "Silent Wipe Twin", "tags": ["infra", "urgent", "q3"]}
        )
    )
    task_id = _content(dispatched)["id"]

    dispatched = asyncio.run(
        server.app.call_tool("mesh_task_update", {"task_id": task_id, "tags": "urgent"})
    )

    assert _content(dispatched)["tags"] == ["infra", "urgent", "q3"]
    assert oracle.id != task_id  # oracle only exists to keep the vault non-empty


# --------------------------------------------------------------------------- #
# Delta and explicit replace, over the same MCP surface                       #
# --------------------------------------------------------------------------- #


def test_mcp_note_update_delta_adds_and_removes(cfg: Config, vault: Path) -> None:
    dispatched = asyncio.run(
        server.app.call_tool("mesh_note_new", {"title": "Delta Note", "tags": ["ndc", "stale"]})
    )
    note_id = _content(dispatched)["id"]

    dispatched = asyncio.run(
        server.app.call_tool("mesh_note_update", {"target": note_id, "tags": "+flights,-stale"})
    )
    assert _content(dispatched)["tags"] == ["ndc", "flights"]


def test_mcp_note_update_delta_remove_absent_is_noop(cfg: Config, vault: Path) -> None:
    dispatched = asyncio.run(
        server.app.call_tool("mesh_note_new", {"title": "Delta Noop Note", "tags": ["ndc"]})
    )
    note_id = _content(dispatched)["id"]

    dispatched = asyncio.run(
        server.app.call_tool("mesh_note_update", {"target": note_id, "tags": "-nope"})
    )
    assert _content(dispatched)["tags"] == ["ndc"]


def test_mcp_note_update_explicit_replace_only_path_that_replaces(cfg: Config, vault: Path) -> None:
    dispatched = asyncio.run(
        server.app.call_tool("mesh_note_new", {"title": "Replace Note", "tags": ["ndc", "stale"]})
    )
    note_id = _content(dispatched)["id"]

    # Bare list: additive, does not replace.
    dispatched = asyncio.run(
        server.app.call_tool("mesh_note_update", {"target": note_id, "tags": "x"})
    )
    assert _content(dispatched)["tags"] == ["ndc", "stale", "x"]

    # Explicit "=" prefix: the only path that replaces.
    dispatched = asyncio.run(
        server.app.call_tool("mesh_note_update", {"target": note_id, "tags": "=y,z"})
    )
    assert _content(dispatched)["tags"] == ["y", "z"]


# --------------------------------------------------------------------------- #
# Mixed-prefix spec — rejected, not silently written as a garbage tag         #
# (fix round 1: "+x,y" used to fall through to additive and write a literal   #
# "+x" tag into the vault permanently)                                       #
# --------------------------------------------------------------------------- #


def test_mcp_note_update_mixed_spec_surfaces_as_clean_tool_error(cfg: Config, vault: Path) -> None:
    from fastmcp.exceptions import ToolError

    dispatched = asyncio.run(
        server.app.call_tool("mesh_note_new", {"title": "Mixed Spec Note", "tags": ["ndc"]})
    )
    note_id = _content(dispatched)["id"]

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("mesh_note_update", {"target": note_id, "tags": "+x,y"}))
    assert "ambiguous tag spec" in str(exc_info.value)
    assert "Traceback" not in str(exc_info.value)

    # Rejected before any write — the tag list is untouched.
    dispatched = asyncio.run(server.app.call_tool("mesh_note_get", {"id": note_id}))
    assert _content(dispatched)["tags"] == ["ndc"]


def test_mcp_task_update_mixed_spec_surfaces_as_clean_tool_error(cfg: Config, vault: Path) -> None:
    from fastmcp.exceptions import ToolError

    dispatched = asyncio.run(
        server.app.call_tool("mesh_task_new", {"title": "Mixed Spec Task", "tags": ["ndc"]})
    )
    task_id = _content(dispatched)["id"]

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("mesh_task_update", {"task_id": task_id, "tags": "+x,y"}))
    assert "ambiguous tag spec" in str(exc_info.value)


def test_cli_note_update_mixed_spec_exits_2_and_writes_nothing(cfg: Config, vault: Path) -> None:
    from typer.testing import CliRunner

    from mesh.cli.__main__ import app as cli_app
    from mesh.core.notes import create_note

    note = create_note(cfg, "Mixed Spec CLI Note", tags=["ndc"], body="x")
    result = CliRunner().invoke(cli_app, ["note", "update", note.id, "--tags", "+x,y"])

    assert result.exit_code == 2
    assert "ambiguous tag spec" in result.output

    from mesh.core.notes import get_note

    assert get_note(cfg, note.id).note.tags == ["ndc"]
