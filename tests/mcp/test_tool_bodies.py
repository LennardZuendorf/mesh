"""core-hardening/6 — every mutating MCP tool body executed, on-disk effect asserted.

``mcp/server.py`` was 62% historically (most mutating tool *bodies* never
executed — only their registration metadata and, for some, their error paths
were exercised). Several later units (memory/1, team-awareness/10,
agent-usability/5) already added tool-level tests that closed a good deal of
this — re-measured at the top of this unit's work at 93%. This file closes what
remained: a handful of tool bodies whose *successful* path (not just their
not-found/config-missing/conflict paths) had never run, plus the private
``_error_kind`` fallback branch that no live domain exception can currently
reach.

Every test here drives the *registered* tool (``server.app.call_tool``, going
through ``_guarded``) and asserts the on-disk file, never the tool's return
value alone — the return value can be right while the write silently didn't
happen (or happened to the wrong file); only the file proves the mutation.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path

import frontmatter
import pytest
from fastmcp.exceptions import ToolError

import shards.mcp.server as server
from shards.core.notes import create_note
from shards.core.tasks import create_task
from shards.schemas.config import Config, load_config

_NOW = "2026-01-01T09:00:00+00:00"


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _call(name: str, params: dict[str, object]) -> dict[str, object]:
    result = asyncio.run(server.app.call_tool(name, params))
    return result.structured_content or {}  # type: ignore[no-any-return]


def _on_disk(path: Path) -> frontmatter.Post:
    return frontmatter.loads(path.read_text(encoding="utf-8"))


@pytest.fixture
def no_agent_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, vault: Path) -> Path:
    """A config with no ``[core].agent`` and no ``$SHARDS_AGENT`` override."""
    cfg_file = tmp_path / "no_agent.toml"
    cfg_file.write_text(
        "\n".join(("[core]", f'tolaria_path = "{vault}"', "", "[tasks]", "collections = []", "")),
        encoding="utf-8",
    )
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(cfg_file))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    return cfg_file


# --------------------------------------------------------------------------- #
# Read-only bodies that had never actually run (only their not-found path had) #
# --------------------------------------------------------------------------- #


def test_shards_note_list_body_returns_matching_on_disk_notes(cfg: Config, vault: Path) -> None:
    create_note(cfg, "First Listed Note", body="one")
    n2 = create_note(cfg, "Second Listed Note", body="two")

    entries = asyncio.run(server.app.call_tool("shards_note_list", {"limit": 1}))
    assert entries.structured_content is not None
    rows = entries.structured_content["result"]
    assert len(rows) == 1
    assert rows[0]["id"] == n2.id  # newest-first default sort: n2 created last
    assert (vault / "notes" / f"{n2.id}.md").exists()


def test_shards_task_get_body_returns_on_disk_frontmatter(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Gettable Task", body="task body text")

    reply = _call("shards_task_get", {"id": task.id})
    assert reply["id"] == task.id
    assert reply["title"] == "Gettable Task"
    assert reply["body"] == "task body text"

    on_disk = _on_disk(vault / "tasks" / "open" / f"{task.id}.md")
    assert on_disk.metadata["id"] == task.id


# --------------------------------------------------------------------------- #
# Mutating tools: successful path, on-disk effect asserted                    #
# --------------------------------------------------------------------------- #


def test_shards_note_new_body_writes_note_to_disk(cfg: Config, vault: Path) -> None:
    reply = _call("shards_note_new", {"title": "MCP-Created Note", "body": "mcp body"})
    note_id = reply["id"]
    on_disk = _on_disk(vault / "notes" / f"{note_id}.md")
    assert on_disk.metadata["title"] == "MCP-Created Note"
    assert on_disk.content == "mcp body"


def test_shards_note_append_body_appends_on_disk(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "Appendable Note", body="original")
    reply = _call("shards_note_append", {"target": note.id, "text": "added-via-mcp"})
    assert reply["id"] == note.id

    on_disk = _on_disk(vault / "notes" / f"{note.id}.md")
    assert "original" in on_disk.content
    assert "added-via-mcp" in on_disk.content


def test_shards_task_new_body_writes_task_to_disk(cfg: Config, vault: Path) -> None:
    reply = _call("shards_task_new", {"title": "MCP-Created Task", "body": "mcp task body"})
    task_id = reply["id"]
    on_disk = _on_disk(vault / "tasks" / "open" / f"{task_id}.md")
    assert on_disk.metadata["title"] == "MCP-Created Task"
    assert on_disk.content == "mcp task body"


def test_shards_task_append_body_appends_on_disk(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Appendable Task", body="original task body")
    reply = _call("shards_task_append", {"task_id": task.id, "text": "added-via-mcp"})
    assert reply["id"] == task.id

    on_disk = _on_disk(vault / "tasks" / "open" / f"{task.id}.md")
    assert "original task body" in on_disk.content
    assert "added-via-mcp" in on_disk.content


def test_shards_note_update_body_moves_note_and_bumps_tags(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "Updatable Note", body="x", note_type="note")
    _call("shards_note_update", {"target": note.id, "tags": "+urgent", "new_type": "decision"})

    old_path = vault / "notes" / f"{note.id}.md"
    new_path = vault / "notes" / "decisions" / f"{note.id}.md"
    assert not old_path.exists()
    assert new_path.exists()
    on_disk = _on_disk(new_path)
    assert on_disk.metadata["tags"] == ["urgent"]
    assert on_disk.metadata["type"] == "decision"


def test_shards_task_claim_body_writes_claimed_by_on_disk(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Claimable Task", body="x")
    reply = _call("shards_task_claim", {"task_id": task.id, "claimer": "mcp-agent"})
    assert reply["claimed_by"] == "mcp-agent"

    on_disk = _on_disk(vault / "tasks" / "open" / f"{task.id}.md")
    assert on_disk.metadata["claimed_by"] == "mcp-agent"
    assert on_disk.metadata["status"] == "claimed"


def test_shards_task_claim_no_agent_identity_raises_validation_error(
    no_agent_config: Path, vault: Path
) -> None:
    """locks.py-adjacent guard in server.py: no claimer and no configured agent."""
    from shards.core.tasks import create_task as _create_task

    task = _create_task(load_config(), "Claim Needs Identity", body="x")

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("shards_task_claim", {"task_id": task.id}))
    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "validation"
    assert "agent identity" in payload["message"]

    on_disk = _on_disk(vault / "tasks" / "open" / f"{task.id}.md")
    assert on_disk.metadata["claimed_by"] is None  # rejected before any write


def test_shards_task_release_body_clears_claimed_by_on_disk(cfg: Config, vault: Path) -> None:
    from shards.core.tasks import claim_task

    task = create_task(cfg, "Releasable Task", body="x")
    claim_task(cfg, task.id, "mcp-agent")

    reply = _call("shards_task_release", {"task_id": task.id, "owner": "mcp-agent"})
    assert reply["claimed_by"] is None

    on_disk = _on_disk(vault / "tasks" / "open" / f"{task.id}.md")
    assert on_disk.metadata["claimed_by"] is None
    assert on_disk.metadata["status"] == "open"


def test_shards_task_release_no_agent_identity_raises_validation_error(
    no_agent_config: Path, vault: Path
) -> None:
    from shards.core.tasks import claim_task as _claim_task
    from shards.core.tasks import create_task as _create_task

    cfg2 = load_config()
    task = _create_task(cfg2, "Release Needs Identity", body="x")
    _claim_task(cfg2, task.id, "someone")

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("shards_task_release", {"task_id": task.id}))
    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "validation"
    assert "agent identity" in payload["message"]

    on_disk = _on_disk(vault / "tasks" / "open" / f"{task.id}.md")
    assert on_disk.metadata["claimed_by"] == "someone"  # rejected before any write


def test_shards_task_finish_body_moves_task_to_done_on_disk(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Finishable Task", body="x")
    reply = _call("shards_task_finish", {"task_id": task.id, "outcome": "shipped via mcp"})
    assert reply["status"] == "done"

    assert not (vault / "tasks" / "open" / f"{task.id}.md").exists()
    done_path = vault / "tasks" / "done" / f"{task.id}.md"
    assert done_path.exists()
    on_disk = _on_disk(done_path)
    assert on_disk.metadata["status"] == "done"
    assert "shipped via mcp" in on_disk.content


def test_shards_task_update_body_writes_new_fields_on_disk(cfg: Config, vault: Path) -> None:
    task = create_task(cfg, "Updatable Task", body="x", priority="low")
    _call(
        "shards_task_update", {"task_id": task.id, "priority": "high", "title": "Renamed via MCP"}
    )

    on_disk = _on_disk(vault / "tasks" / "open" / f"{task.id}.md")
    assert on_disk.metadata["priority"] == "high"
    assert on_disk.metadata["title"] == "Renamed via MCP"


def test_shards_task_cancel_body_moves_task_to_done_cancelled_on_disk(
    cfg: Config, vault: Path
) -> None:
    task = create_task(cfg, "Cancellable Task", body="x")
    reply = _call("shards_task_cancel", {"task_id": task.id, "reason": "no longer needed"})
    assert reply["status"] == "cancelled"

    assert not (vault / "tasks" / "open" / f"{task.id}.md").exists()
    done_path = vault / "tasks" / "done" / f"{task.id}.md"
    assert done_path.exists()
    on_disk = _on_disk(done_path)
    assert on_disk.metadata["status"] == "cancelled"
    assert "no longer needed" in on_disk.content


# --------------------------------------------------------------------------- #
# The generic-ShardsError fallback kind (server.py:1181) — no live subclass    #
# reaches it today; unit-tested directly against a throwaway subclass         #
# --------------------------------------------------------------------------- #


def test_error_kind_falls_back_to_code_table_for_an_unlisted_shards_error() -> None:
    """Every concrete ``ShardsError`` subclass in the codebase is already named
    in ``_KIND_BY_TYPE`` (``NoteNotFoundError``, ``ClaimConflictError``, ...), so
    this defensive fallback — deriving a ``kind`` from the exit-code tier alone
    — is unreachable through any live call path today. It exists for a future
    subclass that forgets to register itself there; exercised directly against
    a throwaway one so the branch is proven correct without inventing a fake
    production code path."""
    from shards.core.errors import ShardsError

    class _FutureError(ShardsError):
        code = 3

    assert server._error_kind(_FutureError("x")) == "not_found"

    class _UnknownCodeError(ShardsError):
        code = 99

    assert server._error_kind(_UnknownCodeError("x")) == "error"
