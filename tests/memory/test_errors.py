"""agent-usability/5 — Structured MCP errors, first-run config failure.

Brief: ``.superpowers/sdd/shards-3track/agent-usability-5-brief.md``. Two defects:

* **Claim conflict was prose, not data.** ``ClaimConflictError`` always carried
  ``existing_owner``/``task_id`` as attributes, but the MCP boundary
  (``_guarded``) flattened every ``ShardsError`` to ``ToolError(str(exc))`` — an
  English sentence an agent had to parse. ``_guarded`` now raises a ``ToolError``
  whose message is a JSON object: ``{kind, message, next_action}`` plus the
  exception's own structured fields (``task_id``, ``existing_owner``, ...).
* **``load_config`` raised ``SystemExit(2)``, a ``BaseException``.** Neither
  ``_guarded`` nor FastMCP's own dispatcher catches anything above
  ``Exception``, so a missing config on an MCP-only machine could escape a real
  tool call as an unhandled crash. ``load_config`` now raises
  :class:`~shards.schemas.config.ConfigMissingError`, a
  :class:`~shards.core.errors.ShardsError` (plain ``Exception``) — caught by
  ``_guarded`` like any other domain exception. The CLI is unaffected: exit 2,
  same message, now via the one ``cli_errors()`` mapper instead of a bespoke
  ``SystemExit``.

Coverage:

* A claim conflict driven through the *registered* ``shards_task_claim`` tool
  (``server.app.call_tool``) yields ``kind="claim_conflict"`` plus ``task_id``
  and ``existing_owner`` as their own JSON fields — asserted as fields, never a
  substring of the ``message`` sentence.
* Every registered tool, called with no config file present, raises a clean
  ``fastmcp.exceptions.ToolError`` (never ``SystemExit``/any other
  ``BaseException``) naming ``shards init`` — asserted per tool, driven through
  the real registration table so a newly added tool cannot silently skip this
  net.
* A representative spread of the other domain exceptions an agent must branch
  on (not-found, ambiguous slug) also carry their identifying fact as a field,
  not just prose.
* The CLI's exit-code contract is unchanged: missing config still exits 2 (with
  a message), not-found still exits 3, claim conflict still exits 4.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

import pytest
from fastmcp.exceptions import ToolError
from typer.testing import CliRunner

import shards.mcp.server as server
from shards.cli.__main__ import app as cli_app
from shards.core.notes import create_note as core_create_note
from shards.core.tasks import claim_task as core_claim_task
from shards.core.tasks import create_task as core_create_task
from shards.schemas.config import Config, load_config

# --------------------------------------------------------------------------- #
# Fixtures                                                                     #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


@pytest.fixture
def missing_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Export ``SHARDS_CONFIG_PATH`` at a path that does not exist.

    Deliberately does *not* use the ``shards_config`` fixture — the whole point
    is that no config file is present anywhere ``load_config`` would look.
    """
    missing = tmp_path / "nope" / "config.toml"
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(missing))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    return missing


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(cli_app, args)


# --------------------------------------------------------------------------- #
# Claim conflict: a structured field, not a sentence                          #
# --------------------------------------------------------------------------- #


def test_claim_conflict_over_mcp_yields_structured_fields(cfg: Config) -> None:
    """Claiming a task another agent holds yields ``kind``/``task_id``/
    ``existing_owner`` as their own JSON fields an agent can branch on —
    not merely findable as a substring of an English sentence."""
    task = core_create_task(cfg, "Contested Task", body="details")
    core_claim_task(cfg, task.id, "other-agent")

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(
            server.app.call_tool("shards_task_claim", {"task_id": task.id, "claimer": "test-agent"})
        )

    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "claim_conflict"
    assert payload["task_id"] == task.id
    assert payload["existing_owner"] == "other-agent"
    assert "next_action" in payload and payload["next_action"]
    # Identity is trusted local input, not an authorization decision (root
    # AGENTS.md §6) — the next-action text must read as a suggestion, never a
    # command the server executed or an authority claim.
    assert "not authorized" not in payload["next_action"].lower()
    assert "denied" not in payload["next_action"].lower()


def test_release_conflict_over_mcp_yields_structured_fields(cfg: Config) -> None:
    """Same contract on the release side: releasing someone else's live claim
    is also a ``ClaimConflictError``, structured identically."""
    task = core_create_task(cfg, "Contested Release", body="details")
    core_claim_task(cfg, task.id, "other-agent")

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(
            server.app.call_tool("shards_task_release", {"task_id": task.id, "owner": "test-agent"})
        )

    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "claim_conflict"
    assert payload["task_id"] == task.id
    assert payload["existing_owner"] == "other-agent"


# --------------------------------------------------------------------------- #
# No BaseException escapes a handler: every registered tool, missing config   #
# --------------------------------------------------------------------------- #

# Minimal valid arguments per registered tool — just enough to satisfy each
# tool's own required-parameter schema so the call reaches its body (where
# ``load_config()`` runs, before anything else). Keyed by tool name and
# asserted for exact set-equality against the live registration table below,
# so a newly added tool cannot silently skip this net.
_MINIMAL_ARGS: dict[str, dict[str, Any]] = {
    "shards_note_get": {"id": "n-doesnotmatter"},
    "shards_note_list": {},
    "shards_task_get": {"id": "t-doesnotmatter"},
    "shards_task_list": {},
    "shards_search": {},
    "shards_health": {},
    "shards_recent_activity": {},
    "shards_build_context": {"seed_id": "n-doesnotmatter"},
    "shards_graph": {"seed_id": "n-doesnotmatter"},
    "shards_project": {"project_id": "n-doesnotmatter"},
    "shards_session_start": {},
    "shards_note_new": {"title": "x"},
    "shards_note_append": {"target": "n-doesnotmatter", "text": "y"},
    "shards_task_new": {"title": "x"},
    "shards_task_append": {"task_id": "t-doesnotmatter", "text": "y"},
    "shards_note_update": {"target": "n-doesnotmatter"},
    "shards_task_claim": {"task_id": "t-doesnotmatter"},
    "shards_task_release": {"task_id": "t-doesnotmatter"},
    "shards_task_finish": {"task_id": "t-doesnotmatter"},
    "shards_task_update": {"task_id": "t-doesnotmatter"},
    "shards_task_cancel": {"task_id": "t-doesnotmatter"},
}


def _registered_tool_names() -> list[str]:
    tools = asyncio.run(server.app.list_tools())
    return [tool.name for tool in tools]


def test_minimal_args_cover_every_registered_tool() -> None:
    """Guard the guard: if a tool is added/renamed without updating
    ``_MINIMAL_ARGS``, fail loudly here instead of quietly under-testing."""
    assert set(_MINIMAL_ARGS) == set(_registered_tool_names())


@pytest.mark.parametrize("tool_name", sorted(_MINIMAL_ARGS))
def test_no_baseexception_escapes_registered_tool_with_missing_config(
    tool_name: str, missing_config: Path
) -> None:
    """Every registered tool, called with no config file anywhere, raises a
    clean ``fastmcp.exceptions.ToolError`` naming ``shards init`` — never
    ``SystemExit`` or any other ``BaseException`` reaching past the handler."""
    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool(tool_name, _MINIMAL_ARGS[tool_name]))

    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "config_missing"
    assert "shards init" in payload["message"]
    assert payload["cfg_path"] == str(missing_config)
    assert "next_action" in payload and "shards init" in payload["next_action"]


# --------------------------------------------------------------------------- #
# Other kinds carry their identifying fact as a field, not just prose         #
# --------------------------------------------------------------------------- #


def test_note_not_found_over_mcp_yields_structured_field(cfg: Config) -> None:
    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("shards_note_get", {"id": "n-ghost"}))

    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "not_found"
    assert payload["id_or_slug"] == "n-ghost"


def test_task_not_found_over_mcp_yields_structured_field(cfg: Config) -> None:
    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("shards_task_get", {"id": "t-ghost"}))

    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "not_found"
    assert payload["task_id"] == "t-ghost"


def test_ambiguous_slug_over_mcp_yields_structured_ids(cfg: Config) -> None:
    core_create_note(cfg, "Duplicate Title Probe", body="one")
    core_create_note(cfg, "Duplicate Title Probe", body="two")

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("shards_note_get", {"id": "Duplicate Title Probe"}))

    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "ambiguous_slug"
    assert payload["slug"] == "Duplicate Title Probe"
    assert isinstance(payload["ids"], list)
    assert len(payload["ids"]) == 2


# --------------------------------------------------------------------------- #
# CLI boundary: exit codes unchanged                                          #
# --------------------------------------------------------------------------- #


def test_cli_missing_config_still_exits_2_with_message(missing_config: Path) -> None:
    """``ConfigMissingError`` reaches ``cli_errors()`` exactly like any other
    ``ShardsError`` — the CLI's exit code and message are unchanged."""
    result = _invoke(["note", "list"])
    assert result.exit_code == 2, result.output
    assert str(missing_config) in result.output
    assert "shards init" in result.output
    assert result.exception is None or isinstance(result.exception, SystemExit), result.output


def test_cli_note_not_found_still_exits_3(cfg: Config) -> None:
    result = _invoke(["note", "get", "n-ghost"])
    assert result.exit_code == 3, result.output


def test_cli_claim_conflict_still_exits_4(cfg: Config) -> None:
    task = core_create_task(cfg, "CLI Contested Task", body="details")
    core_claim_task(cfg, task.id, "other-agent")

    result = _invoke(["task", "claim", task.id])
    assert result.exit_code == 4, result.output
