"""memory/1 — MCP surface: ``mcp/server.py`` FastMCP ``shards_*`` tools.

Acceptance coverage:

* **Registration** — every tool in the ``memory/tech.md`` table is registered under
  its exact ``shards_*`` name, and *only* those (a set-equality check catches both a
  missing tool and a stray extra one in a single assertion).
* **Withholding** — the unsafe / admin surface (``note_delete``, ``task_delete``,
  ``daemon`` controls, ``reindex``, ``status``) is absent. ``shards_task_release``
  is *not* withheld as of team-awareness/10 (it shipped, the Phase-3 deferral
  note it used to carry is gone) — but it carries no ``force`` parameter.
* **Typed params, not flag strings** — a tool's input schema exposes real typed
  fields (``shards_note_get`` takes ``id: str``), never ``--id`` CLI option strings.
* **Annotation mapping** — at least one tool per class: read-only
  (``readOnlyHint``), idempotent (``idempotentHint``), write (no special hint), and
  destructive (``destructiveHint``).
* **Routing** — a sample read tool returns the right dict shape over a *mocked* core
  layer, and the two lens tools call ``core.activity.recent_activity`` /
  ``core.context.build_context`` (memory/2 & memory/3) rather than re-implementing
  them.
* **MCP parity (team-awareness/10)** — ``shards_session_start``, ``shards_task_append``,
  ``shards_task_release``, and the new ``shards_task_list``/``shards_task_update``
  params each get a parity check against the same ``core`` function (or the CLI,
  over one fixture vault) the tool wraps, driven through the *registered* tool
  (``server.app.call_tool``), not the bare Python function.
* **Search-mode marker + health (agent-usability/4)** — ``shards_health`` is a pure
  delegate to ``core.search.search_health`` (same call the CLI's ``--health`` flag
  makes); a ``shards_search`` hit carries ``mode`` (``"indexed"``/``"fallback"``)
  only when a query ran, proven by driving the real fallback and hybrid paths
  through the registered tool and checking the field differs, not by asserting it
  merely exists in one mode.
"""

from __future__ import annotations

import asyncio
import json
import subprocess
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import frontmatter
import pytest
from fastmcp.tools.base import ToolResult
from typer.testing import CliRunner

import shards.mcp.server as server
from shards.cli.__main__ import app as cli_app
from shards.core.notes import NoteView
from shards.core.notes import create_note as core_create_note
from shards.core.tasks import append_task as core_append_task
from shards.core.tasks import claim_task as core_claim_task
from shards.core.tasks import create_task as core_create_task
from shards.core.tasks import get_task as core_get_task
from shards.core.tasks import release_task as core_release_task
from shards.core.tasks import update_task as core_update_task
from shards.index import indexed_client
from shards.schemas.config import Config, load_config
from shards.schemas.note import Note


def _content(dispatched: ToolResult) -> dict[str, Any]:
    """Narrow ``ToolResult.structured_content`` (``dict[str, Any] | None``) for tests
    that know a given tool call always returns structured content."""
    assert dispatched.structured_content is not None
    return dispatched.structured_content


# The tech.md tool table, braces expanded — the complete public MCP surface.
_EXPECTED_TOOLS: frozenset[str] = frozenset(
    {
        "shards_note_new",
        "shards_note_append",
        "shards_note_get",
        "shards_note_list",
        "shards_note_update",
        "shards_task_new",
        "shards_task_append",
        "shards_task_get",
        "shards_task_list",
        "shards_task_claim",
        "shards_task_release",
        "shards_task_finish",
        "shards_task_update",
        "shards_task_cancel",
        "shards_search",
        "shards_health",
        "shards_recent_activity",
        "shards_build_context",
        "shards_graph",
        "shards_project",
        "shards_session_start",
    }
)

# Explicitly withheld: delete + daemon/admin surface. ``shards_task_release``
# ships as of team-awareness/10 (no longer Phase-3-deferred) — see the module
# docstring — so it is intentionally *not* in this set.
_WITHHELD_TOOLS: frozenset[str] = frozenset(
    {
        "shards_note_delete",
        "shards_task_delete",
        "shards_daemon_start",
        "shards_daemon_stop",
        "shards_daemon",
        "shards_reindex",
        "shards_status",
    }
)

# Substrings that must never appear in any registered tool name. "release" is
# no longer forbidden (``shards_task_release`` is now a legitimate tool name);
# its absence is instead pinned by asserting it carries no ``force`` param.
_FORBIDDEN_SUBSTRINGS: tuple[str, ...] = (
    "delete",
    "daemon",
    "reindex",
    "status",
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
        "shards_health",
        "shards_recent_activity",
        "shards_build_context",
        "shards_graph",
        "shards_project",
        "shards_session_start",
    ],
)
def test_read_tools_are_read_only(name: str) -> None:
    tool = _registered()[name]
    assert tool.annotations is not None
    assert tool.annotations.readOnlyHint is True


@pytest.mark.parametrize(
    "name",
    [
        "shards_note_update",
        "shards_task_claim",
        "shards_task_release",
        "shards_task_finish",
        "shards_task_update",
    ],
)
def test_mutating_tools_are_idempotent(name: str) -> None:
    tool = _registered()[name]
    assert tool.annotations is not None
    assert tool.annotations.idempotentHint is True


@pytest.mark.parametrize(
    "name",
    ["shards_note_new", "shards_note_append", "shards_task_new", "shards_task_append"],
)
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
# Duplicate-title warnings (team-awareness/9, R9)                             #
# --------------------------------------------------------------------------- #
#
# MCP has no stream an agent reads, so the same non-blocking advisory the CLI
# puts on stderr travels in the JSON result's ``warnings`` key instead — never
# on a stream nobody reads, never mixed into the entity's own fields.


def test_note_new_duplicate_title_returns_warning(cfg: Config, vault: Path) -> None:
    first = server.shards_note_new(title="Japan visa requirements for Q3 trip", body="x")
    second = server.shards_note_new(title="Japan visa requirements for Q3 trip", body="y")

    # Non-blocking: the second create still succeeded (a real id, a real file).
    assert second["id"] != first["id"]
    assert (vault / "notes" / f"{second['id']}.md").exists()
    assert second["warnings"] == [f"duplicate title, also used by {first['id']}"]


def test_note_new_unique_title_returns_empty_warnings(cfg: Config, vault: Path) -> None:
    result = server.shards_note_new(title="A Wholly Unique MCP Title", body="x")
    assert result["warnings"] == []


def test_task_new_duplicate_title_returns_warning(cfg: Config, vault: Path) -> None:
    first = server.shards_task_new(title="Ship the Q3 report")
    second = server.shards_task_new(title="Ship the Q3 report")

    assert second["id"] != first["id"]
    assert (vault / "tasks" / "open" / f"{second['id']}.md").exists()
    assert second["warnings"] == [f"duplicate title, also used by {first['id']}"]


def test_task_new_unique_title_returns_empty_warnings(cfg: Config, vault: Path) -> None:
    result = server.shards_task_new(title="A Wholly Unique MCP Task Title")
    assert result["warnings"] == []


def test_note_new_case_whitespace_duplicate_returns_warning(cfg: Config, vault: Path) -> None:
    """Slug-normalized rule, not exact-match — mirrors the CLI/core assertion
    (a case/whitespace-only variant still collides, since it's the same
    collision that would poison the slug resolver)."""
    first = server.shards_note_new(title="Case Insensitive MCP Title", body="x")
    second = server.shards_note_new(title=" case  insensitive mcp title ", body="y")
    assert second["warnings"] == [f"duplicate title, also used by {first['id']}"]


def test_note_and_task_sharing_title_do_not_warn_over_mcp(cfg: Config, vault: Path) -> None:
    """Same-kind only: a task and a note sharing a title never warn each other,
    matching the CLI's cross-kind decision (R9)."""
    server.shards_task_new(title="Cross-Kind MCP Title")
    note = server.shards_note_new(title="Cross-Kind MCP Title", body="x")
    assert note["warnings"] == []


def test_note_new_duplicate_title_warning_reaches_structured_content(
    cfg: Config, vault: Path
) -> None:
    """The warning lands in the *structured* MCP result, not a stream nobody
    reads — dispatched through FastMCP's real ``call_tool``, not just the raw
    function."""
    server.shards_note_new(title="Structured Dup", body="x")
    dispatched = asyncio.run(
        server.app.call_tool("shards_note_new", {"title": "Structured Dup", "body": "y"})
    )
    assert dispatched.structured_content is not None
    assert len(_content(dispatched)["warnings"]) == 1
    assert "duplicate title" in _content(dispatched)["warnings"][0]


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


def test_invalid_owner_surfaces_as_clean_tool_error(cfg: Config) -> None:
    """A bare ``ValueError`` (unknown owner, outside ``[tasks].collections``) also
    becomes a clean ``ToolError`` — not FastMCP's generic catch-all fallback.

    Drives the real (unmocked) ``create_note`` -> ``_validate_owner`` path, the
    exact asymmetry the review flagged: ``_guarded`` used to catch only
    ``ShardsError``/``OSError``, so this bare ``ValueError`` fell through to
    FastMCP's own wrapping instead of the boundary mapper.
    """
    from fastmcp.exceptions import ToolError

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(
            server.app.call_tool(
                "shards_note_new", {"title": "x", "owner": "ghost-agent", "body": "y"}
            )
        )

    assert "unknown owner" in str(exc_info.value)
    assert "Traceback" not in str(exc_info.value)


def test_invalid_note_type_rejected_by_schema_before_reaching_core(cfg: Config) -> None:
    """agent-usability/2: ``note_type`` is now typed ``NoteType`` (``schemas/note.py``)
    on the tool signature, so an out-of-vocabulary value is rejected by FastMCP's own
    schema validation *before* ``create_note``'s ``ValueError`` branch is ever reached
    — superseding the prior ``ToolError``-via-``_guarded`` assertion this test used to
    make (``create_note`` is never even called; nothing is written). Still a clean,
    listable rejection naming the valid values, never a raw traceback — the same
    "no crash, no partial write" guarantee, just enforced one layer earlier."""
    from pydantic import ValidationError

    with pytest.raises(ValidationError) as exc_info:
        asyncio.run(
            server.app.call_tool(
                "shards_note_new", {"title": "x", "note_type": "bogus", "body": "y"}
            )
        )

    message = str(exc_info.value)
    assert "note_type" in message
    assert "note" in message and "decision" in message  # the schema enum, named in the error
    assert "Traceback" not in message


# --------------------------------------------------------------------------- #
# core-hardening/4 — the MCP surface must mirror the CLI's threshold fix       #
# --------------------------------------------------------------------------- #
#
# `core/search.py::query_search` states the CLI and MCP surfaces "must behave
# identically". `shards_search` calls the same `resolve_effective_threshold`
# helper `cli/search.py` calls (core-hardening/4 review finding: the two
# surfaces used to hand-roll the same three-way branch, unverified on the MCP
# side). These tests drive the real registered tool via `app.call_tool`, not
# `shards_search` directly, so the MCP dispatch itself is covered — mirroring
# `tests/index/test_fallback_threshold.py::test_default_config_no_indexed_returns_body_hit`.


@pytest.fixture
def default_threshold_config(
    vault: Path, config_path: Path, monkeypatch: pytest.MonkeyPatch
) -> Path:
    """Config pointed at ``vault`` whose ``[search]`` section omits ``threshold``
    (the fresh-install case: no explicit value anywhere)."""
    config_path.write_text(
        "\n".join(
            (
                "[core]",
                f'tolaria_path = "{vault}"',
                'agent = "test-agent"',
                "",
                "[search]",
                'collection = "test-vault"',
                "hybrid = true",
                "",
                "[tasks]",
                'collections = ["test-agent"]',
                "",
            )
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(config_path))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    return config_path


def _seed_body_only_hit(vault: Path) -> None:
    """A note whose title/tags don't match but whose body contains 'eTA'."""
    meta: dict[str, object] = {
        "id": "n-visa",
        "type": "note",
        "title": "Travel Notes",
        "tags": ["travel"],
        "owner": "seed-agent",
        "created": "2026-06-01T00:00:00+00:00",
        "updated": "2026-06-01T00:00:00+00:00",
        "related": [],
    }
    body = "Remember to apply for the eTA before the flight."
    folder = vault / "notes"
    folder.mkdir(parents=True, exist_ok=True)
    post = frontmatter.Post(body)
    post.metadata = meta
    (folder / "n-visa.md").write_text(frontmatter.dumps(post), encoding="utf-8")


def test_mcp_search_default_config_no_indexed_returns_body_hit(
    default_threshold_config: Path, vault: Path
) -> None:
    """Default config, no `indexed` on PATH, no daemon: the body-only hit (score
    0.4) is returned through the real `shards_search` MCP tool dispatch — the
    CLI-side fix (`test_default_config_no_indexed_returns_body_hit`) mirrored on
    the MCP surface."""
    _seed_body_only_hit(vault)

    dispatched = asyncio.run(server.app.call_tool("shards_search", {"query": "eTA"}))

    hits = _content(dispatched)["result"]
    assert {h["id"] for h in hits} == {"n-visa"}
    assert hits[0]["score"] == 0.4


def test_mcp_search_explicit_threshold_param_still_excludes_body_hit(
    default_threshold_config: Path, vault: Path
) -> None:
    """The tool's own typed `threshold` parameter is the MCP equivalent of
    `--threshold`; passing it explicitly still filters, even over a default
    config with no other explicit threshold source."""
    _seed_body_only_hit(vault)

    dispatched = asyncio.run(
        server.app.call_tool("shards_search", {"query": "eTA", "threshold": 0.7})
    )

    assert _content(dispatched)["result"] == []


def test_mcp_search_explicit_config_threshold_behaves_as_today(cfg: Config, vault: Path) -> None:
    """`cfg` (the `shards_config` fixture) sets an explicit `[search].threshold
    = 0.65` — the body-only hit stays excluded, same as before this fix."""
    _seed_body_only_hit(vault)

    dispatched = asyncio.run(server.app.call_tool("shards_search", {"query": "eTA"}))

    assert _content(dispatched)["result"] == []


# --------------------------------------------------------------------------- #
# team-awareness/10 — MCP parity sweep                                        #
# --------------------------------------------------------------------------- #
#
# Every test below drives the *registered* tool (``server.app.call_tool``),
# never the bare module function, and asserts the result matches the same
# ``core`` function (or the CLI, which itself routes through that ``core``
# function) over one fixture vault — the parity contract this unit exists to
# close.


def _cli(args: list[str]) -> Any:
    """Invoke the real CLI app and parse its ``--json`` stdout."""
    result = CliRunner().invoke(cli_app, args)
    assert result.exit_code == 0, result.output
    return json.loads(result.output)


# --- enumeration / schema — the new params exist with the right shape ------ #


def test_task_list_new_params_present_with_correct_types(cfg: Config) -> None:
    """``stale`` / ``available`` / ``sort`` (35f7301, 3235de3) reach the schema."""
    props = _registered()["shards_task_list"].parameters["properties"]
    assert "stale" in props  # nullable str -> {"anyOf": [{"type": "string"}, {"type": "null"}]}
    assert props["available"]["type"] == "boolean"
    assert "sort" in props


def test_task_update_owner_param_present(cfg: Config) -> None:
    """``owner`` (tech.md's R10 parity table: ``shards_task_update(owner=…)``)."""
    props = _registered()["shards_task_update"].parameters["properties"]
    assert "owner" in props


def test_task_release_has_no_force_param(cfg: Config) -> None:
    """The binding constraint, pinned at the schema: ``--force`` never reaches MCP."""
    props = _registered()["shards_task_release"].parameters["properties"]
    assert "force" not in props
    assert set(props) == {"task_id", "owner"}


def test_graph_direction_param_present_with_out_default(cfg: Config) -> None:
    """8854319 landed ``direction`` already — verify, don't rebuild (debt item 5)."""
    props = _registered()["shards_graph"].parameters["properties"]
    assert props["direction"]["default"] == "out"


def test_session_start_params_present(cfg: Config) -> None:
    props = _registered()["shards_session_start"].parameters["properties"]
    assert set(props) == {"owner", "team", "meta_only"}


def test_module_docstring_phase3_deferral_note_is_corrected() -> None:
    """The stale ``mcp/server.py`` header comment (test scenario 5) is gone."""
    assert server.__doc__ is not None
    assert "Phase-3" not in server.__doc__
    assert "not the Phase-3" not in server.__doc__
    assert "shards_task_release" in server.__doc__


# --- routing — mocked core layer, no second implementation ----------------- #


def test_task_append_tool_calls_core_append_task(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen: dict[str, Any] = {}

    def _spy(config: Config, task_id: str, text: str, *, section: str | None, timestamp: bool):
        seen["task_id"], seen["text"] = task_id, text
        seen["section"], seen["timestamp"] = section, timestamp
        return core_create_task(config, "spy result")

    monkeypatch.setattr(server, "append_task", _spy)

    dispatched = asyncio.run(
        server.app.call_tool(
            "shards_task_append", {"task_id": "t-fake", "text": "hi", "section": "Log"}
        )
    )

    assert seen == {"task_id": "t-fake", "text": "hi", "section": "Log", "timestamp": False}
    assert _content(dispatched)["title"] == "spy result"


def test_task_release_tool_calls_core_release_task(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen: dict[str, Any] = {}

    def _spy(config: Config, task_id: str, releaser: str):
        seen["task_id"], seen["releaser"] = task_id, releaser
        return core_create_task(config, "spy released")

    monkeypatch.setattr(server, "release_task", _spy)

    dispatched = asyncio.run(
        server.app.call_tool("shards_task_release", {"task_id": "t-fake", "owner": "test-agent"})
    )

    assert seen == {"task_id": "t-fake", "releaser": "test-agent"}
    assert _content(dispatched)["title"] == "spy released"


def test_session_start_tool_calls_core_session_start_entries(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """team-awareness/10's headline tool composes the same core composer the CLI
    does — not a re-implementation of the tasks/mentions/activity merge."""
    sentinel = [{"id": "t-1", "type": "task", "reason": "task", "title": "One", "path": "/p"}]
    seen: dict[str, Any] = {}

    def _spy(task_views, activity, mentions, *, meta_only):
        seen["meta_only"] = meta_only
        return sentinel

    monkeypatch.setattr(server, "session_start_entries", _spy)

    dispatched = asyncio.run(server.app.call_tool("shards_session_start", {}))

    assert _content(dispatched)["result"] == sentinel
    assert seen == {"meta_only": False}


# --- parity — the registered tool matches the core/CLI path it wraps ------- #


def test_parity_task_append(cfg: Config, vault: Path) -> None:
    """``shards_task_append`` lands the identical body edit ``core.append_task``
    (and hence ``task append``) produces for the same input."""
    oracle = core_create_task(cfg, "Parity Append Oracle")
    expected = core_append_task(cfg, oracle.id, "same note text", section="Log")

    twin = core_create_task(cfg, "Parity Append Twin")
    dispatched = asyncio.run(
        server.app.call_tool(
            "shards_task_append",
            {"task_id": twin.id, "text": "same note text", "section": "Log"},
        )
    )
    result = _content(dispatched)

    assert result["status"] == expected.status
    assert core_get_task(cfg, oracle.id).body == core_get_task(cfg, twin.id).body


def test_parity_task_release(cfg: Config, vault: Path) -> None:
    """``shards_task_release`` lands the identical claimed->open transition
    ``core.release_task`` (and hence ``task release``) produces."""
    oracle = core_create_task(cfg, "Parity Release Oracle")
    core_claim_task(cfg, oracle.id, "test-agent")
    expected = core_release_task(cfg, oracle.id, "test-agent")

    twin = core_create_task(cfg, "Parity Release Twin")
    core_claim_task(cfg, twin.id, "test-agent")
    dispatched = asyncio.run(
        server.app.call_tool("shards_task_release", {"task_id": twin.id, "owner": "test-agent"})
    )
    result = _content(dispatched)

    assert result["status"] == expected.status == "open"
    assert result["claimed_by"] is expected.claimed_by is None


def test_parity_task_update_owner(cfg: Config, vault: Path) -> None:
    """``shards_task_update(owner=…)`` lands the identical reassignment
    ``core.update_task`` (and hence ``task update --owner``) produces."""
    oracle = core_create_task(cfg, "Parity Owner Oracle")
    expected = core_update_task(cfg, oracle.id, owner="other-agent")

    twin = core_create_task(cfg, "Parity Owner Twin")
    dispatched = asyncio.run(
        server.app.call_tool("shards_task_update", {"task_id": twin.id, "owner": "other-agent"})
    )
    result = _content(dispatched)

    assert result["owner"] == expected.owner == "other-agent"
    # Reassignment never touches claimed_by (root AGENTS.md's owner/claim split).
    assert result["claimed_by"] is None


def test_parity_task_list_available_and_priority_sort(cfg: Config, vault: Path) -> None:
    """``shards_task_list(available=True)`` returns the same ids, same
    priority-sorted order, as ``task list --available --json`` (35f7301, 3235de3)."""
    core_create_task(cfg, "Low prio available", priority="low")
    core_create_task(cfg, "High prio available", priority="high")
    claimed = core_create_task(cfg, "Not available (claimed)", priority="high")
    core_claim_task(cfg, claimed.id, "test-agent")

    cli_ids = [row["id"] for row in _cli(["--json", "task", "list", "--available"])]

    dispatched = asyncio.run(server.app.call_tool("shards_task_list", {"available": True}))
    mcp_ids = [row["id"] for row in _content(dispatched)["result"]]

    assert mcp_ids == cli_ids
    assert claimed.id not in mcp_ids


def test_parity_task_list_stale(cfg: Config, vault: Path) -> None:
    """``shards_task_list(stale=…)`` — the inverse of ``since`` — matches the CLI.

    A freshly created task is never older than a 9999-day window, so both
    surfaces must agree it is excluded (empty result)."""
    core_create_task(cfg, "Stale filter task")

    cli_ids = [row["id"] for row in _cli(["--json", "task", "list", "--stale", "9999d"])]

    dispatched = asyncio.run(server.app.call_tool("shards_task_list", {"stale": "9999d"}))
    mcp_ids = [row["id"] for row in _content(dispatched)["result"]]

    assert mcp_ids == cli_ids == []


def test_parity_task_list_status_csv(cfg: Config, vault: Path) -> None:
    """CSV ``status`` (already worked transparently) stays undocumented-but-live;
    pinned here now that the docstring calls it out explicitly."""
    open_task = core_create_task(cfg, "CSV status open")
    claimed_task = core_create_task(cfg, "CSV status claimed")
    core_claim_task(cfg, claimed_task.id, "test-agent")

    dispatched = asyncio.run(server.app.call_tool("shards_task_list", {"status": "open,claimed"}))
    mcp_ids = {row["id"] for row in _content(dispatched)["result"]}

    assert {open_task.id, claimed_task.id} <= mcp_ids


def test_parity_session_start(cfg: Config, vault: Path) -> None:
    """``shards_session_start`` returns the identical payload ``session-start
    --json`` does over the same vault state — tasks, then mentions, then
    activity, deduped by id, each entry carrying ``reason``."""
    mine = core_create_task(cfg, "My live queue task")
    core_claim_task(cfg, mine.id, "test-agent")

    # A note owned by someone else mentioning my task — the notify half (R7).
    from shards.core.notes import create_note

    create_note(
        cfg,
        "Mentions my task",
        owner="other-agent",
        body=f"see [[{mine.id}]]",
    )

    cli_entries = _cli(["session-start", "--json"])
    dispatched = asyncio.run(server.app.call_tool("shards_session_start", {}))
    mcp_entries = _content(dispatched)["result"]

    assert mcp_entries == cli_entries
    reasons_by_id = {e["id"]: e["reason"] for e in mcp_entries}
    assert reasons_by_id[mine.id] == "task"
    assert "mention" in reasons_by_id.values()


def test_parity_session_start_owner_and_team(cfg: Config, vault: Path) -> None:
    """``owner=`` swaps the effective identity; ``team=True`` widens the activity
    half only — matching ``--owner``/``--team`` on the CLI side."""
    core_create_task(cfg, "Other agent's task", owner="other-agent")

    cli_entries = _cli(["session-start", "--json", "--owner", "other-agent", "--team"])
    dispatched = asyncio.run(
        server.app.call_tool("shards_session_start", {"owner": "other-agent", "team": True})
    )
    mcp_entries = _content(dispatched)["result"]

    assert mcp_entries == cli_entries


# --------------------------------------------------------------------------- #
# agent-usability/4 — shards_health + the shards_search mode marker           #
# --------------------------------------------------------------------------- #


def test_shards_health_is_a_pure_delegate_to_core_search_health(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """No parallel MCP implementation: ``shards_health`` must call the exact
    ``core.search.search_health`` the CLI's ``--health`` flag calls, and return
    its value unmodified — so the two surfaces read off one implementation and
    cannot drift apart (a spy proves the *call*, not just a matching value)."""
    sentinel: dict[str, Any] = {
        "mode": "fallback",
        "hybrid_configured": True,
        "collection": "test-vault",
        "daemon_up": False,
        "indexed_binary_available": True,
        "reason": "daemon down",
    }
    calls: list[Config] = []

    def _spy(config: Config) -> dict[str, Any]:
        calls.append(config)
        return sentinel

    monkeypatch.setattr(server, "search_health", _spy)
    result = server.shards_health()

    assert result is sentinel
    assert len(calls) == 1


def test_shards_health_tool_takes_no_parameters(cfg: Config) -> None:
    """The registered schema has an empty ``properties`` object — the answer
    depends only on live config/environment state, never caller input."""
    props = _registered()["shards_health"].parameters["properties"]
    assert props == {}


def test_shards_health_registered_read_only(cfg: Config) -> None:
    tool = _registered()["shards_health"]
    assert tool.annotations is not None
    assert tool.annotations.readOnlyHint is True


def test_shards_health_withholds_status_daemon_reindex_init_and_delete(cfg: Config) -> None:
    """The exact withheld set this unit must not widen (binding constraint 1)."""
    names = set(_registered())
    for withheld in (
        "shards_status",
        "shards_daemon",
        "shards_daemon_start",
        "shards_daemon_stop",
        "shards_reindex",
        "shards_init",
        "shards_note_delete",
        "shards_task_delete",
    ):
        assert withheld not in names


def _seed_note_for_search(vault: Path, *, title: str) -> Path:
    """A real note under ``vault`` whose title exactly matches ``title`` — an
    exact-title-tier (score 1.0) hit under both the fallback scorer and a
    mocked ``indexed`` hit, so threshold filtering never enters into it."""
    note = core_create_note(load_config(), title)
    return vault / "notes" / f"{note.id}.md"


def test_mcp_search_marks_hits_fallback_when_indexed_unreachable(cfg: Config, vault: Path) -> None:
    """No daemon, no ``indexed`` on PATH — the real fallback path runs (nothing
    here mocks ``query_search`` itself), and the hit is marked accordingly."""
    _seed_note_for_search(vault, title="Zephyr Marker Probe Fallback")

    dispatched = asyncio.run(
        server.app.call_tool("shards_search", {"query": "Zephyr Marker Probe Fallback"})
    )
    hits = _content(dispatched)["result"]

    assert hits and all(h["mode"] == "fallback" for h in hits)


def test_mcp_search_marks_hits_indexed_when_hybrid_runs(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Daemon up + a real (mocked-subprocess) ``indexed`` hit — the real hybrid
    path runs end to end, and the hit is marked accordingly. Paired with the
    fallback test above: the *same* field takes two different real values
    depending on which engine genuinely answered, not a hard-coded string only
    ever exercised in one mode."""
    path = _seed_note_for_search(vault, title="Zephyr Marker Probe Hybrid")
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: True)
    ndjson = json.dumps({"path": str(path), "score": 0.91, "snippet": "hybrid snippet"}) + "\n"
    monkeypatch.setattr(indexed_client, "_run_indexed_search", lambda *a, **k: ndjson)

    dispatched = asyncio.run(
        server.app.call_tool("shards_search", {"query": "Zephyr Marker Probe Hybrid"})
    )
    hits = _content(dispatched)["result"]

    assert hits and all(h["mode"] == "indexed" for h in hits)


def test_mcp_search_marks_hits_fallback_on_indexed_runtime_failure(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Regression (round-1 review, Finding 1): every gate ``search_health``
    checks reports healthy (daemon up, binary on PATH, hybrid + collection
    configured) but the real ``indexed`` subprocess exits non-zero for an
    unrelated runtime reason — corrupt collection, resource exhaustion,
    whatever. ``query_search`` genuinely falls back and returns the seeded
    note via the substring scorer; the hit must be marked ``fallback``, never
    ``indexed`` — a marker that is *wrong in the confident direction* is worse
    than no marker, since it tells an agent to trust output it should hedge
    on. This was RED before the fix (mode reported "indexed", predicted from
    static gates rather than observed from the branch `query_search` actually
    took) and is GREEN after (`query_search` now returns the mode it actually
    used)."""
    path = _seed_note_for_search(vault, title="Zephyr Marker Probe Runtime Failure")
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: True)

    def _raise(*_a: object, **_k: object) -> str:
        raise subprocess.CalledProcessError(1, ["indexed"])

    monkeypatch.setattr(indexed_client, "_run_indexed_search", _raise)

    dispatched = asyncio.run(
        server.app.call_tool("shards_search", {"query": "Zephyr Marker Probe Runtime Failure"})
    )
    hits = _content(dispatched)["result"]

    # The substring fallback genuinely found the note (proves the fallback
    # really ran, not just that the mode field happens to read "fallback").
    assert {h["id"] for h in hits} == {path.stem}
    assert all(h["mode"] == "fallback" for h in hits)


def test_mcp_search_tag_pull_carries_no_mode_marker(cfg: Config, vault: Path) -> None:
    """A tag-only pull (no ``query``) never carries ``mode`` — it is served
    from the warm daemon index or an equivalent cold folder scan, a
    daemon-liveness distinction that never degrades recall, unlike the
    indexed/fallback split a real query makes (see the tool's docstring)."""
    core_create_note(load_config(), "Tag Pull Probe", tags=["probe"])

    dispatched = asyncio.run(server.app.call_tool("shards_search", {"tags": ["probe"]}))
    hits = _content(dispatched)["result"]

    assert hits
    assert all("mode" not in h for h in hits)
