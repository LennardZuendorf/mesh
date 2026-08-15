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

``--direction`` (team-awareness/1) adds backlink traversal on top of the same
BFS — see ``tests/memory/test_inbound.py`` for the ``inbound_ids``/``_inbound_index``
unit coverage; the direction cases here exercise it through ``graph_query`` and
the CLI, mirroring the cycle/diamond acceptance shape above one direction at a
time (``in``, then ``both``).
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
# core: --direction — inbound derivation wired into the BFS (team-awareness/1) #
# --------------------------------------------------------------------------- #


def test_direction_in_finds_backlink_with_no_forward_link(cfg: Config, vault: Path) -> None:
    """The load-bearing case: X's own body/frontmatter names nothing, yet a
    mention elsewhere is still found — this is the whole point of the unit."""
    _seed_task(vault, task_id="t-target", title="Target", related=[])
    _seed_note(vault, note_id="n-mentioner", title="Reply", related=["t-target"])

    out = graph_query(cfg, "t-target", depth=1, direction="in")
    assert out.ids == ["t-target", "n-mentioner"]

    # The forward query on the mentioner is unaffected by being an inbound source.
    forward = graph_query(cfg, "n-mentioner", depth=1, direction="out")
    assert forward.ids == ["n-mentioner", "t-target"]


def test_direction_in_edge_is_source_to_target(cfg: Config, vault: Path) -> None:
    """Inbound edges are emitted source→target (mentioner→mentioned), matching
    the underlying link direction — never (target, source)."""
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    out = graph_query(cfg, "n-b", depth=1, direction="in")
    assert out.edges == [("n-a", "n-b")]


def test_direction_out_is_unchanged_by_direction_default(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    default = graph_query(cfg, "n-a", depth=1)
    explicit_out = graph_query(cfg, "n-a", depth=1, direction="out")
    assert default.ids == explicit_out.ids == ["n-a", "n-b"]
    assert default.edges == explicit_out.edges == [("n-a", "n-b")]


def test_direction_both_diamond_no_duplicates(cfg: Config, vault: Path) -> None:
    """A→B (out), C→A (in): querying A with both reaches B and C, each once."""
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])
    _seed_note(vault, note_id="n-c", title="Cee", related=["n-a"])

    out = graph_query(cfg, "n-a", depth=1, direction="both")
    assert sorted(out.ids) == ["n-a", "n-b", "n-c"]
    assert len(out.ids) == 3  # each node exactly once
    assert set(out.edges) == {("n-a", "n-b"), ("n-c", "n-a")}
    assert len(out.edges) == 2  # each edge exactly once


def test_direction_both_mutual_link_is_one_edge_not_two(cfg: Config, vault: Path) -> None:
    """A mutual link (A→B and B→A) must not be double-counted under 'both'."""
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-a"])

    out = graph_query(cfg, "n-a", depth=5, direction="both")
    assert out.ids == ["n-a", "n-b"]
    assert out.edges == [("n-a", "n-b")]  # not also (n-b, n-a)


def test_direction_both_self_reference_is_deduplicated(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-a"])

    out = graph_query(cfg, "n-a", depth=3, direction="both")
    assert out.ids == ["n-a"]
    assert out.edges == []


def test_direction_in_tree_lines_nest_by_discovery_not_link_direction(
    cfg: Config, vault: Path
) -> None:
    """tree_lines() nests the mentioner under the seed it was discovered from,
    even though the link-direction edge (in ``to_dict()``) points the other way."""
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    out = graph_query(cfg, "n-b", depth=1, direction="in")
    lines = out.tree_lines()
    assert len(lines) == 2
    assert lines[0].startswith("n-b")
    assert lines[1].split("\t", 1)[0].strip() == "n-a"
    assert lines[1] != lines[1].lstrip()  # nested under the seed
    # But the JSON/edge contract still reports the true link direction.
    assert out.to_dict()["edges"] == [["n-a", "n-b"]]


def test_direction_invalid_raises_value_error(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", title="Ay")
    with pytest.raises(ValueError, match="direction"):
        graph_query(cfg, "n-a", depth=1, direction="sideways")


@pytest.mark.parametrize("direction", ["out", "in", "both"])
def test_direction_unknown_seed_raises_in_every_direction(cfg: Config, direction: str) -> None:
    with pytest.raises(SeedNotFoundError):
        graph_query(cfg, "n-nope", depth=1, direction=direction)


def test_direction_in_diamond_collapses_shared_source(cfg: Config, vault: Path) -> None:
    """D→d, B→D and C→D (out), A→{B,C} (out): inbound(D, depth=2) reaches B, C,
    then A once each — the mirror image of test_diamond_has_no_duplicates."""
    _seed_note(vault, note_id="n-d", title="Dee")
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-d"])
    _seed_note(vault, note_id="n-c", title="Cee", related=["n-d"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b", "n-c"])

    out = graph_query(cfg, "n-d", depth=2, direction="in")
    assert out.ids == ["n-d", "n-b", "n-c", "n-a"]
    assert set(out.edges) == {("n-b", "n-d"), ("n-c", "n-d"), ("n-a", "n-b")}
    assert len(out.edges) == 3  # n-a discovered once, not twice


def test_direction_in_covers_notes_and_tasks_both_ways(cfg: Config, vault: Path) -> None:
    """Inbound derivation works with a task mentioning a note and vice versa."""
    _seed_note(vault, note_id="n-target", title="Target Note")
    _seed_task(vault, task_id="t-mentioner", title="Mentioning Task", related=["n-target"])

    out = graph_query(cfg, "n-target", depth=1, direction="in")
    assert out.ids == ["n-target", "t-mentioner"]


def test_direction_in_does_not_walk_vault_at_depth_zero(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """depth=0 never expands the seed, so the whole-vault inbound pass is skipped."""
    _seed_note(vault, note_id="n-a", title="Ay")

    def _boom(config: Config) -> dict[str, list[str]]:
        raise AssertionError("inbound index built despite depth=0")

    monkeypatch.setattr(context_module, "_inbound_index", _boom)
    out = graph_query(cfg, "n-a", depth=0, direction="in")
    assert out.ids == ["n-a"]


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


def test_cli_direction_in_finds_backlink(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    result = _invoke(["graph", "n-b", "--direction", "in", "--json"])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert [n["id"] for n in payload["nodes"]] == ["n-b", "n-a"]
    assert payload["edges"] == [["n-a", "n-b"]]


def test_cli_direction_both_union(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])
    _seed_note(vault, note_id="n-c", title="Cee", related=["n-a"])

    result = _invoke(["graph", "n-a", "--direction", "both", "--json"])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert sorted(n["id"] for n in payload["nodes"]) == ["n-a", "n-b", "n-c"]


def test_cli_direction_defaults_to_out(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    result = _invoke(["graph", "n-a", "--json"])
    assert result.exit_code == 0, result.output
    assert [n["id"] for n in json.loads(result.stdout)["nodes"]] == ["n-a", "n-b"]


def test_cli_direction_invalid_exits_2(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", title="Ay")

    result = _invoke(["graph", "n-a", "--direction", "sideways"])
    assert result.exit_code == 2


def test_cli_direction_in_unknown_seed_exits_3(cfg: Config) -> None:
    result = _invoke(["graph", "n-nope", "--direction", "in"])
    assert result.exit_code == 3


# --------------------------------------------------------------------------- #
# Daemon-up vs daemon-down parity — no degradation path, so identical either way #
# --------------------------------------------------------------------------- #


def test_direction_in_identical_daemon_up_and_down(
    cfg: Config, vault: Path, tmp_path: Path
) -> None:
    """Inbound derivation never touches the daemon; a running daemon must not
    change the answer (constraint: "the daemon never gates")."""
    import shutil
    import tempfile

    from tests.daemon.conftest import running_daemon

    _seed_task(vault, task_id="t-target", title="Target", related=[])
    _seed_note(vault, note_id="n-mentioner", title="Reply", related=["t-target"])

    cold = graph_query(cfg, "t-target", depth=2, direction="both").to_dict()

    sock_dir = Path(tempfile.mkdtemp(prefix="brn-inbound-", dir="/tmp"))
    try:
        socket_path = sock_dir / "d.sock"
        with running_daemon(socket_path, config=cfg):
            warm = graph_query(cfg, "t-target", depth=2, direction="both").to_dict()
    finally:
        shutil.rmtree(sock_dir, ignore_errors=True)

    assert warm == cold


# --------------------------------------------------------------------------- #
# MCP: shards_graph                                                            #
# --------------------------------------------------------------------------- #


def test_mcp_tool_registered_read_only(cfg: Config) -> None:
    import shards.mcp.server as server

    tools = {tool.name: tool for tool in asyncio.run(server.app.list_tools())}
    assert "shards_graph" in tools
    assert tools["shards_graph"].annotations is not None
    assert tools["shards_graph"].annotations.readOnlyHint is True


def test_mcp_tool_delegates_to_graph_query(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    import shards.mcp.server as server

    sentinel = GraphResult(
        entries=[{"id": "n-seed", "type": "note", "title": "Seed", "path": "/p"}],
        edges=[],
        tree_edges=[],
    )
    seen: dict[str, Any] = {}

    def _spy(config: Config, seed_id: str, depth: int = 1, direction: str = "out") -> GraphResult:
        seen["seed_id"], seen["depth"], seen["direction"] = seed_id, depth, direction
        return sentinel

    monkeypatch.setattr(server, "graph_query", _spy)

    out = server.shards_graph(seed_id="n-seed", depth=2)

    assert out == sentinel.to_dict()
    assert seen == {"seed_id": "n-seed", "depth": 2, "direction": "out"}


def test_mcp_tool_direction_passthrough(cfg: Config, vault: Path) -> None:
    """End-to-end (unmocked): ``shards_graph(direction="in")`` finds a backlink."""
    import shards.mcp.server as server

    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    payload = server.shards_graph(seed_id="n-b", depth=1, direction="in")

    assert [n["id"] for n in payload["nodes"]] == ["n-b", "n-a"]
    assert payload["edges"] == [["n-a", "n-b"]]
