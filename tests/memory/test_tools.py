"""memory/1 — MCP surface: ``mcp/server.py`` FastMCP ``shards_*`` tools.

Acceptance coverage:

* **Registration** — every tool in the ``memory/tech.md`` table is registered under
  its exact ``shards_*`` name, and *only* those (a set-equality check catches both a
  missing tool and a stray extra one in a single assertion).
* **Withholding** — the unsafe / admin surface (``note_delete``, ``task_delete``,
  ``daemon`` controls, ``reindex``, ``status``, and the Phase-3 ``task_release``) is
  absent.
* **Typed params, not flag strings** — a tool's input schema exposes real typed
  fields (``shards_note_get`` takes ``id: str``), never ``--id`` CLI option strings.
* **Annotation mapping** — at least one tool per class: read-only
  (``readOnlyHint``), idempotent (``idempotentHint``), write (no special hint), and
  destructive (``destructiveHint``).
* **Routing** — a sample read tool returns the right dict shape over a *mocked* core
  layer, and the two lens tools call ``core.activity.recent_activity`` /
  ``core.context.build_context`` (memory/2 & memory/3) rather than re-implementing
  them.
"""

from __future__ import annotations

import asyncio
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import pytest

import shards.mcp.server as server
from shards.core.notes import NoteView
from shards.schemas.config import Config, load_config
from shards.schemas.note import Note

# The tech.md tool table, braces expanded — the complete public MCP surface.
_EXPECTED_TOOLS: frozenset[str] = frozenset(
    {
        "shards_note_new",
        "shards_note_append",
        "shards_note_get",
        "shards_note_list",
        "shards_note_update",
        "shards_task_new",
        "shards_task_get",
        "shards_task_list",
        "shards_task_claim",
        "shards_task_finish",
        "shards_task_update",
        "shards_task_cancel",
        "shards_search",
        "shards_recent_activity",
        "shards_build_context",
        "shards_graph",
        "shards_project",
    }
)

# Explicitly withheld: delete + daemon/admin, and the Phase-3 release verb.
_WITHHELD_TOOLS: frozenset[str] = frozenset(
    {
        "shards_note_delete",
        "shards_task_delete",
        "shards_daemon_start",
        "shards_daemon_stop",
        "shards_daemon",
        "shards_reindex",
        "shards_status",
        "shards_task_release",
    }
)

# Substrings that must never appear in any registered tool name.
_FORBIDDEN_SUBSTRINGS: tuple[str, ...] = (
    "delete",
    "daemon",
    "reindex",
    "status",
    "release",
)


def _registered() -> dict[str, Any]:
    """Map every registered tool name to its FunctionTool (via ``app.list_tools``)."""
    tools = asyncio.run(server.app.list_tools())
    return {tool.name: tool for tool in tools}


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


# --------------------------------------------------------------------------- #
# Registration + withholding                                                  #
# --------------------------------------------------------------------------- #


def test_every_tech_table_tool_is_registered() -> None:
    """Exact set-equality: no missing tool, no stray extra tool."""
    assert set(_registered()) == set(_EXPECTED_TOOLS)


def test_withheld_tools_are_absent() -> None:
    names = set(_registered())
    assert names.isdisjoint(_WITHHELD_TOOLS)
    for name in names:
        for forbidden in _FORBIDDEN_SUBSTRINGS:
            assert forbidden not in name, f"{name!r} exposes withheld surface {forbidden!r}"


# --------------------------------------------------------------------------- #
# Typed params, not CLI flag strings                                          #
# --------------------------------------------------------------------------- #


def test_note_get_takes_typed_id_field_not_flag_string() -> None:
    """``shards_note_get`` exposes ``id: str`` as a schema field, not ``--id``."""
    tool = _registered()["shards_note_get"]
    props = tool.parameters["properties"]
    assert "id" in props
    assert props["id"]["type"] == "string"
    assert "id" in tool.parameters.get("required", [])
    # No CLI-style option strings leak into the parameter names.
    for key in props:
        assert not key.startswith("-")


# --------------------------------------------------------------------------- #
# Annotation mapping — one tool per class                                     #
# --------------------------------------------------------------------------- #


@pytest.mark.parametrize(
    "name",
    [
        "shards_note_get",
        "shards_note_list",
        "shards_task_get",
        "shards_task_list",
        "shards_search",
        "shards_recent_activity",
        "shards_build_context",
        "shards_graph",
    ],
)
def test_read_tools_are_read_only(name: str) -> None:
    tool = _registered()[name]
    assert tool.annotations is not None
    assert tool.annotations.readOnlyHint is True


@pytest.mark.parametrize(
    "name",
    ["shards_note_update", "shards_task_claim", "shards_task_finish", "shards_task_update"],
)
def test_mutating_tools_are_idempotent(name: str) -> None:
    tool = _registered()[name]
    assert tool.annotations is not None
    assert tool.annotations.idempotentHint is True


@pytest.mark.parametrize("name", ["shards_note_new", "shards_note_append", "shards_task_new"])
def test_write_tools_carry_no_special_hint(name: str) -> None:
    """Plain writes get *no* hint (not read-only, not idempotent, not destructive)."""
    tool = _registered()[name]
    assert tool.annotations is None


def test_cancel_tool_is_destructive() -> None:
    tool = _registered()["shards_task_cancel"]
    assert tool.annotations is not None
    assert tool.annotations.destructiveHint is True


# --------------------------------------------------------------------------- #
# Routing — mocked core layer                                                 #
# --------------------------------------------------------------------------- #


def test_note_get_returns_dict_shape_over_mocked_core(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The read tool serialises a core ``NoteView`` into the expected dict."""
    now = datetime.now(UTC)
    note = Note.model_validate(
        {
            "id": "n-abcd",
            "type": "note",
            "title": "Sample",
            "tags": ["x"],
            "owner": "test-agent",
            "created": now,
            "updated": now,
            "related": ["n-other"],
        }
    )
    view = NoteView(note=note, body="Body text.", path=Path("/vault/notes/n-abcd.md"))

    def _fake_get_note(config: Config, id_or_slug: str) -> NoteView:
        assert id_or_slug == "n-abcd"
        return view

    # Patch the name the tool binds (imported into the server module).
    monkeypatch.setattr(server, "get_note", _fake_get_note)

    result = server.shards_note_get(id="n-abcd")

    assert result["id"] == "n-abcd"
    assert result["type"] == "note"
    assert result["title"] == "Sample"
    assert result["related"] == ["n-other"]
    assert result["body"] == "Body text."
    assert result["path"] == "/vault/notes/n-abcd.md"

    # And through FastMCP's real dispatch (typed-field coercion + result wrapping),
    # not just the raw function — the same shape must land in structured content.
    dispatched = asyncio.run(server.app.call_tool("shards_note_get", {"id": "n-abcd"}))
    assert dispatched.structured_content == result


def test_recent_activity_tool_calls_core_recent_activity(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """memory/2 wiring: the tool delegates to ``core.activity.recent_activity``."""
    sentinel = [{"id": "n-1", "type": "note", "title": "One", "path": "/p", "mtime": 1.0}]
    seen: dict[str, Any] = {}

    def _spy(
        config: Config, *, since: str | None, owner: str | None, mine: bool, limit: int
    ) -> list[dict[str, Any]]:
        seen["since"], seen["owner"], seen["mine"], seen["limit"] = since, owner, mine, limit
        return sentinel

    monkeypatch.setattr(server, "recent_activity", _spy)

    out = server.shards_recent_activity(since="7d", owner=None, mine=True, limit=5)

    assert out == sentinel
    assert seen == {"since": "7d", "owner": None, "mine": True, "limit": 5}


def test_build_context_tool_calls_core_build_context(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """memory/3 wiring: the tool delegates to ``core.context.build_context``."""
    sentinel = [{"id": "n-seed", "type": "note", "title": "Seed", "path": "/p"}]
    seen: dict[str, Any] = {}

    def _spy(config: Config, seed_id: str, depth: int = 1) -> list[dict[str, Any]]:
        seen["seed_id"], seen["depth"] = seed_id, depth
        return sentinel

    monkeypatch.setattr(server, "build_context", _spy)

    out = server.shards_build_context(seed_id="n-seed", depth=2)

    assert out == sentinel
    assert seen == {"seed_id": "n-seed", "depth": 2}


# --------------------------------------------------------------------------- #
# core-hardening/3 — boundary mapping: domain/OSError -> a clean ToolError    #
# --------------------------------------------------------------------------- #


def test_note_not_found_surfaces_as_clean_tool_error(cfg: Config) -> None:
    """A domain ``NoteNotFoundError`` reaches the client as a ``ToolError`` —
    a clean one-line message, never a raw traceback string."""
    from fastmcp.exceptions import ToolError

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("shards_note_get", {"id": "n-nope"}))

    assert "note not found" in str(exc_info.value)
    assert "Traceback" not in str(exc_info.value)


def test_write_oserror_surfaces_as_clean_tool_error(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """An infrastructure ``OSError`` (ENOSPC, read-only vault, ...) reaching a
    write tool also becomes a clean ``ToolError`` with an ``io error:`` line."""
    from fastmcp.exceptions import ToolError

    def boom(config: Config, title: str, **kwargs: Any) -> Any:  # noqa: ARG001
        raise OSError(28, "No space left on device")

    monkeypatch.setattr(server, "create_note", boom)

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("shards_note_new", {"title": "x"}))

    assert "io error:" in str(exc_info.value)
    assert "Traceback" not in str(exc_info.value)
