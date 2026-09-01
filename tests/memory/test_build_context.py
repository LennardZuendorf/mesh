"""memory/3 — build-context lens: ``core/context.py`` + ``mesh build-context``.

``build_context`` is a read-only, daemon-independent BFS over the ``related``
id graph, starting at a seed id and stopping at ``--depth`` hops (``depth=1`` =
seed + its direct ``related`` entries). It resolves both ``n-`` (notes/) and
``t-`` (tasks/) ids via :func:`mesh.core.notes.get_note` /
:func:`mesh.core.tasks.get_task`, and returns a JSON-serialisable list of the
standard note/task frontmatter shape plus ``path``, in BFS traversal order with
the seed first and every id visited at most once.

Acceptance coverage:

* **depth-0** — seed only, even when the seed has ``related`` entries.
* **depth-1 BFS** — seed followed by its direct ``related`` entries, in order.
* **cycle dedup** — ``A → B → A`` yields ``[A, B]`` (no infinite loop, no dup).
* **diamond** — ``A → {B, C}``, ``B → D``, ``C → D`` yields ``[A, B, C, D]``
  (``D`` once, resolved once).
* **unknown seed** — raises :class:`SeedNotFoundError`; the CLI maps it to exit 3.
* **mixed ids** — a note whose ``related`` spans an ``n-`` note and a ``t-`` task
  resolves both.
* **entry shape** — every entry carries the frontmatter keys plus ``path``.
* **CLI** — ``mesh build-context`` is a leaf command; ``--json`` emits the array;
  ``--depth`` controls the horizon; an unknown seed exits 3.

Order assertions are on *lists*, not sets, so a broken traversal that happens to
visit the right id set still fails.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import frontmatter
import pytest
from typer.testing import CliRunner

from mesh.cli.__main__ import app
from mesh.core.context import SeedNotFoundError, build_context
from mesh.schemas.config import Config, load_config
from mesh.storage.files import note_folder, task_folder

# --------------------------------------------------------------------------- #
# Fixtures & seeding helpers                                                   #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg(mesh_config: Path) -> Config:
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
    """Write a mesh note with an explicit ``related`` frontmatter list."""
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
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
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
    """Write a mesh task with an explicit ``related`` frontmatter list."""
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
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def _ids(entries: list[dict[str, Any]]) -> list[str]:
    return [e["id"] for e in entries]


def _invoke(args: list[str]) -> Any:
    return CliRunner().invoke(app, args)


# --------------------------------------------------------------------------- #
# core: depth semantics                                                        #
# --------------------------------------------------------------------------- #


def test_depth_zero_returns_seed_only(cfg: Config, vault: Path) -> None:
    """depth=0 stops at the seed even when it has related entries."""
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    out = build_context(cfg, "n-a", depth=0)

    assert _ids(out) == ["n-a"]


def test_depth_one_bfs_order(cfg: Config, vault: Path) -> None:
    """depth=1 = seed + its direct related, in related order, seed first."""
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-c", title="Cee")
    # n-c is a grand-child via n-b; it must NOT appear at depth 1.
    _seed_note(vault, note_id="n-b2", title="Bee2", related=["n-c"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b", "n-b2"])

    out = build_context(cfg, "n-a", depth=1)

    assert _ids(out) == ["n-a", "n-b", "n-b2"]


def test_depth_two_reaches_grandchildren(cfg: Config, vault: Path) -> None:
    """depth=2 expands one more hop past the direct related."""
    _seed_note(vault, note_id="n-d", title="Dee")
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-d"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    out = build_context(cfg, "n-a", depth=2)

    assert _ids(out) == ["n-a", "n-b", "n-d"]


# --------------------------------------------------------------------------- #
# core: cycles & diamonds — dedup by id                                        #
# --------------------------------------------------------------------------- #


def test_cycle_is_deduplicated(cfg: Config, vault: Path) -> None:
    """A ↔ B cycle terminates and lists each id once."""
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-a"])

    out = build_context(cfg, "n-a", depth=5)

    assert _ids(out) == ["n-a", "n-b"]


def test_self_reference_is_deduplicated(cfg: Config, vault: Path) -> None:
    """A note that links its own id resolves to itself exactly once."""
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-a"])

    out = build_context(cfg, "n-a", depth=3)

    assert _ids(out) == ["n-a"]


def test_diamond_has_no_duplicates(cfg: Config, vault: Path) -> None:
    """A→{B,C}, B→D, C→D: D appears once, BFS order A,B,C,D."""
    _seed_note(vault, note_id="n-d", title="Dee")
    _seed_note(vault, note_id="n-b", title="Bee", related=["n-d"])
    _seed_note(vault, note_id="n-c", title="Cee", related=["n-d"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b", "n-c"])

    out = build_context(cfg, "n-a", depth=2)

    assert _ids(out) == ["n-a", "n-b", "n-c", "n-d"]


# --------------------------------------------------------------------------- #
# core: mixed n-/t- ids and entry shape                                        #
# --------------------------------------------------------------------------- #


def test_mixed_note_and_task_ids(cfg: Config, vault: Path) -> None:
    """A note seed whose related spans an n- note and a t- task resolves both."""
    _seed_note(vault, note_id="n-child", title="Child Note")
    _seed_task(vault, task_id="t-child", title="Child Task")
    _seed_note(vault, note_id="n-root", title="Root", related=["n-child", "t-child"])

    out = build_context(cfg, "n-root", depth=1)

    assert _ids(out) == ["n-root", "n-child", "t-child"]
    by_id = {e["id"]: e for e in out}
    assert by_id["n-child"]["type"] == "note"
    assert by_id["t-child"]["type"] == "task"
    # Task entries carry the task-only lifecycle fields.
    assert by_id["t-child"]["status"] == "open"


def test_task_seed_resolves(cfg: Config, vault: Path) -> None:
    """A t- seed is resolved via get_task and can expand into notes."""
    _seed_note(vault, note_id="n-x", title="Ex")
    _seed_task(vault, task_id="t-root", title="Root Task", related=["n-x"])

    out = build_context(cfg, "t-root", depth=1)

    assert _ids(out) == ["t-root", "n-x"]


def test_entry_shape_has_frontmatter_plus_path(cfg: Config, vault: Path) -> None:
    """Every entry is the frontmatter shape plus a ``path`` key."""
    path = _seed_note(vault, note_id="n-a", title="Ay")

    out = build_context(cfg, "n-a", depth=0)

    assert len(out) == 1
    entry = out[0]
    for key in ("id", "type", "title", "tags", "owner", "created", "updated", "related", "path"):
        assert key in entry, key
    assert entry["path"] == str(path)
    # JSON-serialisable end to end.
    json.dumps(out)


# --------------------------------------------------------------------------- #
# core: unknown seed                                                           #
# --------------------------------------------------------------------------- #


def test_unknown_seed_raises(cfg: Config) -> None:
    with pytest.raises(SeedNotFoundError):
        build_context(cfg, "n-nope", depth=1)


def test_unknown_task_seed_raises(cfg: Config) -> None:
    with pytest.raises(SeedNotFoundError):
        build_context(cfg, "t-nope", depth=1)


def test_missing_related_id_is_skipped_not_raised(cfg: Config, vault: Path) -> None:
    """A dangling related id (points at no file) is skipped, not fatal."""
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-ghost"])

    out = build_context(cfg, "n-a", depth=1)

    assert _ids(out) == ["n-a"]


# --------------------------------------------------------------------------- #
# CLI: mesh build-context                                                     #
# --------------------------------------------------------------------------- #


def test_cli_registered_as_leaf_command(cfg: Config) -> None:
    result = _invoke(["--help"])
    assert result.exit_code == 0, result.output
    assert "build-context" in result.stdout


def test_cli_json_bfs_array(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    result = _invoke(["build-context", "n-a", "--json"])
    assert result.exit_code == 0, result.output

    arr = json.loads(result.stdout)
    assert isinstance(arr, list)
    assert [e["id"] for e in arr] == ["n-a", "n-b"]
    for e in arr:
        assert "path" in e


def test_cli_depth_option(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    result = _invoke(["build-context", "n-a", "--depth", "0", "--json"])
    assert result.exit_code == 0, result.output
    assert [e["id"] for e in json.loads(result.stdout)] == ["n-a"]


def test_cli_unknown_seed_exits_3(cfg: Config) -> None:
    result = _invoke(["build-context", "n-nope", "--json"])
    assert result.exit_code == 3


def test_cli_quiet_emits_ids_only(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bee")
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-b"])

    result = _invoke(["--quiet", "build-context", "n-a"])
    assert result.exit_code == 0, result.output
    lines = [ln for ln in result.stdout.splitlines() if ln.strip()]
    assert lines == ["n-a", "n-b"]
