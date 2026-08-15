"""cli-toolset-rework/4 — the project read-lens: ``core/lenses.py`` + ``shards project``.

The project lens is the fourth read-only view alongside ``recent-activity`` /
``build-context`` / ``graph`` (root ``.spec/tech.md`` § Contracts;
``.spec/features/cli-toolset-rework/tech.md`` § Workstream C → C2). Given a project
note id it returns the project note plus every task whose ``project`` soft link
matches — "my project and its work in one call" — as a :class:`ProjectResult`
whose ``to_dict`` yields ``{"project": <note>, "tasks": [<task>, ...]}``.

It is *daemon-independent* (nodes read straight off disk) and read-only. It ships
as a leaf lens command + a read-only ``shards_project`` MCP tool, mirroring the
``graph`` lens — NOT as a fourth write verb: this suite pins that the three verbs
(note/task/search) are unchanged.
"""

from __future__ import annotations

import asyncio
import json
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import frontmatter
import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app
from shards.core.lenses import ProjectNotFoundError, ProjectResult, project_view
from shards.schemas.config import Config, load_config
from shards.storage.files import note_folder, task_folder


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _seed_project_note(vault: Path, *, note_id: str = "n-proj", title: str = "Q3 Launch") -> Path:
    when = datetime.now(UTC)
    meta: dict[str, Any] = {
        "id": note_id,
        "type": "project",
        "title": title,
        "tags": [],
        "owner": "test-agent",
        "created": when,
        "updated": when,
        "related": [],
    }
    folder = note_folder("project", vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{note_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post("scope", **meta)), encoding="utf-8")
    return path


def _seed_task(
    vault: Path, *, task_id: str, project: str | None = None, status: str = "open"
) -> Path:
    when = datetime.now(UTC)
    meta: dict[str, Any] = {
        "id": task_id,
        "type": "task",
        "title": f"Task {task_id}",
        "tags": [],
        "owner": "test-agent",
        "created": when,
        "updated": when,
        "related": [],
        "status": status,
        "priority": None,
        "claimed_by": None,
        "blocks": [],
        "blocked_by": [],
    }
    if project is not None:
        meta["project"] = project
    folder = task_folder(status, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post("body", **meta)), encoding="utf-8")
    return path


# --------------------------------------------------------------------------- #
# core lens                                                                     #
# --------------------------------------------------------------------------- #


def test_project_view_returns_note_and_scoped_tasks(cfg: Config, vault: Path) -> None:
    _seed_project_note(vault, note_id="n-proj")
    _seed_task(vault, task_id="t-a", project="n-proj")
    _seed_task(vault, task_id="t-b", project="n-proj", status="done")
    _seed_task(vault, task_id="t-c", project="n-other")
    _seed_task(vault, task_id="t-d")  # unscoped

    result = project_view(cfg, "n-proj")

    assert isinstance(result, ProjectResult)
    assert result.project["id"] == "n-proj"
    assert result.project["type"] == "project"
    assert "path" in result.project
    assert {t["id"] for t in result.tasks} == {"t-a", "t-b"}


def test_project_view_to_dict_shape(cfg: Config, vault: Path) -> None:
    _seed_project_note(vault, note_id="n-proj")
    _seed_task(vault, task_id="t-a", project="n-proj")

    payload = project_view(cfg, "n-proj").to_dict()

    assert payload["project"]["id"] == "n-proj"
    assert [t["id"] for t in payload["tasks"]] == ["t-a"]
    json.dumps(payload)  # JSON-serialisable end to end


def test_project_view_empty_when_no_tasks(cfg: Config, vault: Path) -> None:
    _seed_project_note(vault, note_id="n-proj")
    result = project_view(cfg, "n-proj")
    assert result.tasks == []


def test_project_view_unknown_id_raises(cfg: Config, vault: Path) -> None:
    with pytest.raises(ProjectNotFoundError):
        project_view(cfg, "n-nope")


# --------------------------------------------------------------------------- #
# CLI — shards project (a leaf lens, not a verb)                                #
# --------------------------------------------------------------------------- #


def test_cli_project_registered_as_leaf(cfg: Config) -> None:
    result = _invoke(["--help"])
    assert result.exit_code == 0, result.output
    assert "project" in result.stdout


def test_cli_project_json_shape(cfg: Config, vault: Path) -> None:
    _seed_project_note(vault, note_id="n-proj")
    _seed_task(vault, task_id="t-a", project="n-proj")

    result = _invoke(["project", "n-proj", "--json"])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert payload["project"]["id"] == "n-proj"
    assert [t["id"] for t in payload["tasks"]] == ["t-a"]


def test_cli_project_quiet_emits_ids(cfg: Config, vault: Path) -> None:
    _seed_project_note(vault, note_id="n-proj")
    _seed_task(vault, task_id="t-a", project="n-proj")

    result = _invoke(["--quiet", "project", "n-proj"])
    assert result.exit_code == 0, result.output
    ids = [ln.strip() for ln in result.stdout.splitlines() if ln.strip()]
    assert ids == ["n-proj", "t-a"]


def test_cli_project_unknown_exits_3(cfg: Config) -> None:
    result = _invoke(["project", "n-nope", "--json"])
    assert result.exit_code == 3


# --------------------------------------------------------------------------- #
# MCP — shards_project registered read-only                                    #
# --------------------------------------------------------------------------- #


def test_mcp_project_tool_registered_read_only(cfg: Config) -> None:
    import shards.mcp.server as server

    tools = {tool.name: tool for tool in asyncio.run(server.app.list_tools())}
    assert "shards_project" in tools
    assert tools["shards_project"].annotations is not None
    assert tools["shards_project"].annotations.readOnlyHint is True


def test_mcp_project_tool_delegates(cfg: Config, vault: Path) -> None:
    import shards.mcp.server as server

    _seed_project_note(vault, note_id="n-proj")
    _seed_task(vault, task_id="t-a", project="n-proj")

    out = server.shards_project(project_id="n-proj")
    assert out["project"]["id"] == "n-proj"
    assert [t["id"] for t in out["tasks"]] == ["t-a"]


# --------------------------------------------------------------------------- #
# Invariant — no fourth verb                                                    #
# --------------------------------------------------------------------------- #


def test_no_new_top_level_verb() -> None:
    """The three write verbs stay note/task/search (+ daemon admin); no 4th verb."""
    import shards.cli.__main__ as main

    assert set(main._SUBAPPS) == {"note", "task", "search", "daemon"}


def test_project_is_a_leaf_lens_not_a_subapp() -> None:
    import shards.cli.__main__ as main

    assert "project" in main._LEAVES
    assert "project" not in main._SUBAPPS


def test_task_verb_gains_no_new_command() -> None:
    """--project rides existing task new/update/list; no new task subcommand.

    ``append`` is included because team-awareness/2 legitimately adds it as a
    sub-command of the existing ``task`` verb (still three top-level verbs) —
    this assertion guards against a *stray* extra subcommand, not against
    ``task`` ever growing one.
    """
    import shards.cli.task as task_cli

    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert names == {
        "new",
        "append",
        "update",
        "claim",
        "finish",
        "cancel",
        "get",
        "list",
        "delete",
    }
