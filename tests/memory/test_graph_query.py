"""cli-toolset-rework/3 — graph-query lens: ``core/context.py`` + ``shards graph``.

``graph_query`` promotes ``build_context``'s BFS-over-``related`` traversal to a
first-class, directly queryable surface (root ``.spec/tech.md`` § Implemented
surfaces; ``.spec/features/cli-toolset-rework/tech.md`` § Workstream C → C1). It
runs the *same* seed-first, cycle/diamond-deduped BFS ``build_context`` performs
— no second graph engine — and additionally records the parent→child discovery
edge each time a neighbour is first enqueued, so a single traversal yields both:

* :meth:`GraphResult.to_dict` — machine JSON: ``{"seed", "nodes", "edges"}``.
* :meth:`GraphResult.tree_lines` — a readable, indented tree (DFS over the
  discovery spanning tree the BFS already produced).

Acceptance coverage mirrors ``tests/memory/test_build_context.py``:

* **multi-hop reach** — depth-0/1/2 traversal semantics match ``build_context``.
* **cycle dedup** — ``A ↔ B`` terminates, one edge.
* **diamond dedup** — ``A → {B, C}``, ``B → D``, ``C → D``: ``D`` visited once,
  parented by whichever of ``B``/``C`` reaches it first in BFS order.
* **JSON + tree from one traversal** — both outputs are pure presentations over
  a single already-fetched :class:`GraphResult`; resolving get_note/get_task a
  second time for either output would be a bug this suite catches.
* **no hybrid-search dependency** — ``core/context.py`` imports neither
  ``shards.index`` nor ``shards.core.search``.
* **CLI** — ``shards graph`` is a leaf command; ``--json`` emits ``{seed, nodes,
  edges}``; default text is the readable tree; ``--quiet`` is ids only; unknown
  seed exits 3.
* **MCP** — ``shards_graph`` is registered read-only and delegates to
  ``core.context.graph_query``.
"""

from __future__ import annotations

import ast
import asyncio
import json
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import frontmatter
import pytest
from typer.testing import CliRunner

import shards.core.context as context_module
from shards.cli.__main__ import app
from shards.core.context import GraphResult, SeedNotFoundError, graph_query
from shards.schemas.config import Config, load_config
from shards.storage.files import note_folder, task_folder

# --------------------------------------------------------------------------- #
# Fixtures & seeding helpers (mirrors test_build_context.py)                   #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _seed_note(
    vault: Path,
    *,
    note_id: str,
    title: str = "A Note",
    related: list[str] | None = None,
    note_type: str = "note",
    owner: str = "test-agent",
    body: str = "Body line.",
) -> Path:
    """Write a shards note with an explicit ``related`` frontmatter list."""
    when = datetime.now(UTC)
    meta: dict[str, Any] = {
        "id": note_id,
        "type": note_type,
        "title": title,
        "tags": [],
        "owner": owner,
        "created": when,
        "updated": when,
        "related": list(related or []),
    }
    folder = note_folder(note_type, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{note_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


def _seed_task(
    vault: Path,
    *,
    task_id: str,
    title: str = "Seed Task",
    related: list[str] | None = None,
    status: str = "open",
    owner: str = "test-agent",
    body: str = "Task body.",
) -> Path:
    """Write a shards task with an explicit ``related`` frontmatter list."""
    when = datetime.now(UTC)
    meta: dict[str, Any] = {
        "id": task_id,
        "type": "task",
        "title": title,
        "tags": [],
        "owner": owner,
        "created": when,
        "updated": when,
        "related": list(related or []),
        "status": status,
        "priority": None,
        "claimed_by": None,
        "blocks": [],
        "blocked_by": [],
    }
    folder = task_folder(status, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


def _invoke(args: list[str]) -> Any:
    return CliRunner().invoke(app, args)


# --------------------------------------------------------------------------- #
# core: reuses build_context's traversal semantics                             #
# --------------------------------------------------------------------------- #


def test_depth_zero_returns_seed_only(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    out = graph_query(cfg, "n-a", depth=0)

    assert out.ids == ["n-a"]
    assert out.edges == []


def test_depth_one_bfs_order(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-c", title="Cee")
    _seed_note(vault, note_id="n-b2", title="Bee2", related=["n-c"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b", "n-b2"])

    out = graph_query(cfg, "n-a", depth=1)

    assert out.ids == ["n-a", "n-b", "n-b2"]
    assert out.edges == [("n-a", "n-b"), ("n-a", "n-b2")]


def test_multi_hop_reach(cfg: Config, vault: Path) -> None:
    """A multi-hop related chain (A→B→C→D) is fully reachable at depth 3."""
    _seed_note(vault, note_id="n-d", title="Dee")
    _seed_note(vault, note_id="n-c", title="Cee", related=["n-d"])
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-c"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    out = graph_query(cfg, "n-a", depth=3)

    assert out.ids == ["n-a", "n-b", "n-c", "n-d"]
    assert out.edges == [("n-a", "n-b"), ("n-b", "n-c"), ("n-c", "n-d")]


def test_multi_hop_stops_at_depth_horizon(cfg: Config, vault: Path) -> None:
    """The same chain at depth=2 does not reach the 3rd hop (n-d)."""
    _seed_note(vault, note_id="n-d", title="Dee")
    _seed_note(vault, note_id="n-c", title="Cee", related=["n-d"])
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-c"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    out = graph_query(cfg, "n-a", depth=2)

    assert out.ids == ["n-a", "n-b", "n-c"]


# --------------------------------------------------------------------------- #
# core: cycles & diamonds — dedup by id, one discovery edge each               #
# --------------------------------------------------------------------------- #


def test_cycle_is_deduplicated(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-a"])

    out = graph_query(cfg, "n-a", depth=5)

    assert out.ids == ["n-a", "n-b"]
    assert out.edges == [("n-a", "n-b")]


def test_self_reference_is_deduplicated(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-a"])

    out = graph_query(cfg, "n-a", depth=3)

    assert out.ids == ["n-a"]
    assert out.edges == []


def test_diamond_has_no_duplicates(cfg: Config, vault: Path) -> None:
    """A→{B,C}, B→D, C→D: D appears once, parented by B (first BFS discovery)."""
    _seed_note(vault, note_id="n-d", title="Dee")
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-d"])
    _seed_note(vault, note_id="n-c", title="Cee", related=["n-d"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b", "n-c"])

    out = graph_query(cfg, "n-a", depth=2)

    assert out.ids == ["n-a", "n-b", "n-c", "n-d"]
    assert out.edges == [("n-a", "n-b"), ("n-a", "n-c"), ("n-b", "n-d")]


# --------------------------------------------------------------------------- #
# core: mixed n-/t- ids and unknown seed                                       #
# --------------------------------------------------------------------------- #


def test_mixed_note_and_task_ids(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-child", title="Child Note")
    _seed_task(vault, task_id="t-child", title="Child Task")
    _seed_note(vault, note_id="n-root", title="Root", related=["n-child", "t-child"])

    out = graph_query(cfg, "n-root", depth=1)

    assert out.ids == ["n-root", "n-child", "t-child"]


def test_unknown_seed_raises(cfg: Config) -> None:
    with pytest.raises(SeedNotFoundError):
        graph_query(cfg, "n-nope", depth=1)


def test_missing_related_id_is_skipped_not_raised(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-ghost"])

    out = graph_query(cfg, "n-a", depth=1)

    assert out.ids == ["n-a"]
    assert out.edges == []


# --------------------------------------------------------------------------- #
# core: JSON + tree, both derived from ONE traversal                           #
# --------------------------------------------------------------------------- #


def test_to_dict_shape(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    out = graph_query(cfg, "n-a", depth=1)
    payload = out.to_dict()

    assert payload["seed"] == "n-a"
    assert [n["id"] for n in payload["nodes"]] == ["n-a", "n-b"]
    assert payload["edges"] == [["n-a", "n-b"]]
    json.dumps(payload)  # JSON-serialisable end to end


def test_tree_lines_reflect_discovery_structure(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-d", title="Dee")
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-d"])
    _seed_note(vault, note_id="n-c", title="Cee", related=["n-d"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b", "n-c"])

    out = graph_query(cfg, "n-a", depth=2)
    lines = out.tree_lines()

    # DFS pre-order over the discovery spanning tree: n-a, n-b, n-d (n-b's
    # child), then n-c — n-d is fully nested under n-b before n-c is visited.
    assert len(lines) == 4
    assert lines[0].startswith("n-a")
    ids_in_order = [line.split("\t", 1)[0].strip() for line in lines]
    assert ids_in_order == ["n-a", "n-b", "n-d", "n-c"]

    indent_a = len(lines[0]) - len(lines[0].lstrip())
    indent_b = len(lines[1]) - len(lines[1].lstrip())
    indent_d = len(lines[2]) - len(lines[2].lstrip())
    indent_c = len(lines[3]) - len(lines[3].lstrip())
    # n-b and n-c are both direct children of the seed: same depth.
    assert indent_b > indent_a
    assert indent_c == indent_b
    # n-d is nested under n-b (its discovery parent), one level deeper still.
    assert indent_d > indent_b


def test_json_and_tree_are_pure_presentations_no_second_traversal(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """One ``graph_query`` call resolves each node once; both outputs reuse it.

    Wraps ``get_note``/``get_task`` with counters, runs ``graph_query`` exactly
    once, then derives both ``to_dict()`` and ``tree_lines()`` from the *same*
    result. If either output secretly re-walked the graph (hit disk again), the
    counters below would climb past the single-traversal count.
    """
    _seed_note(vault, note_id="n-d", title="Dee")
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-d"])
    _seed_note(vault, note_id="n-c", title="Cee", related=["n-d"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b", "n-c"])

    calls = {"count": 0}
    real_get_note = context_module.get_note

    def _counting_get_note(config: Config, id_or_slug: str) -> Any:
        calls["count"] += 1
        return real_get_note(config, id_or_slug)

    monkeypatch.setattr(context_module, "get_note", _counting_get_note)

    result = graph_query(cfg, "n-a", depth=2)
    count_after_traversal = calls["count"]
    assert count_after_traversal == 4  # n-a, n-b, n-c, n-d each resolved once

    payload = result.to_dict()
    tree = result.tree_lines()

    assert calls["count"] == count_after_traversal, "to_dict()/tree_lines() must not re-walk"
    assert len(payload["nodes"]) == 4
    assert len(tree) == 4


def test_build_context_and_graph_query_agree_on_ids(cfg: Config, vault: Path) -> None:
    """graph_query is additive: build_context keeps returning the same entries."""
    from shards.core.context import build_context

    _seed_note(vault, note_id="n-d", title="Dee")
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-d"])
    _seed_note(vault, note_id="n-c", title="Cee", related=["n-d"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b", "n-c"])

    entries = build_context(cfg, "n-a", depth=2)
    graph = graph_query(cfg, "n-a", depth=2)

    assert [e["id"] for e in entries] == graph.ids


# --------------------------------------------------------------------------- #
# core: no hybrid-search / indexed dependency                                  #
# --------------------------------------------------------------------------- #


def test_context_module_has_no_hybrid_search_dependency() -> None:
    """C1: the graph-query path never imports the indexed/search layer."""
    source = Path(context_module.__file__).read_text(encoding="utf-8")
    tree = ast.parse(source)
    imported: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imported.update(alias.name for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module:
            imported.add(node.module)

    assert not any(name.startswith("shards.index") for name in imported)
    assert not any(name.startswith("shards.core.search") for name in imported)


# --------------------------------------------------------------------------- #
# CLI: shards graph                                                            #
# --------------------------------------------------------------------------- #


def test_cli_registered_as_leaf_command(cfg: Config) -> None:
    result = _invoke(["--help"])
    assert result.exit_code == 0, result.output
    assert "graph" in result.stdout


def test_cli_json_shape(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    result = _invoke(["graph", "n-a", "--json"])
    assert result.exit_code == 0, result.output

    payload = json.loads(result.stdout)
    assert payload["seed"] == "n-a"
    assert [n["id"] for n in payload["nodes"]] == ["n-a", "n-b"]
    assert payload["edges"] == [["n-a", "n-b"]]


def test_cli_default_output_is_a_tree(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    result = _invoke(["graph", "n-a"])
    assert result.exit_code == 0, result.output

    lines = [ln for ln in result.stdout.splitlines() if ln.strip()]
    assert len(lines) == 2
    assert lines[0].startswith("n-a")
    # n-b is indented under n-a, not flush-left like the seed.
    assert lines[1] != lines[1].lstrip()


def test_cli_depth_option(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    result = _invoke(["graph", "n-a", "--depth", "0", "--json"])
    assert result.exit_code == 0, result.output
    assert [n["id"] for n in json.loads(result.stdout)["nodes"]] == ["n-a"]


def test_cli_unknown_seed_exits_3(cfg: Config) -> None:
    result = _invoke(["graph", "n-nope", "--json"])
    assert result.exit_code == 3


def test_cli_quiet_emits_ids_only(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    result = _invoke(["--quiet", "graph", "n-a"])
    assert result.exit_code == 0, result.output
    lines = [ln for ln in result.stdout.splitlines() if ln.strip()]
    assert lines == ["n-a", "n-b"]


# --------------------------------------------------------------------------- #
# MCP: shards_graph                                                            #
# --------------------------------------------------------------------------- #


def test_mcp_tool_registered_read_only(cfg: Config) -> None:
    import shards.mcp.server as server

    tools = {tool.name: tool for tool in asyncio.run(server.app.list_tools())}
    assert "shards_graph" in tools
    assert tools["shards_graph"].annotations is not None
    assert tools["shards_graph"].annotations.readOnlyHint is True


def test_mcp_tool_delegates_to_graph_query(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    import shards.mcp.server as server

    sentinel = GraphResult(entries=[{"id": "n-seed", "type": "note", "title": "Seed", "path": "/p"}], edges=[])
    seen: dict[str, Any] = {}

    def _spy(config: Config, seed_id: str, depth: int = 1) -> GraphResult:
        seen["seed_id"], seen["depth"] = seed_id, depth
        return sentinel

    monkeypatch.setattr(server, "graph_query", _spy)

    out = server.shards_graph(seed_id="n-seed", depth=2)

    assert out == sentinel.to_dict()
    assert seen == {"seed_id": "n-seed", "depth": 2}
