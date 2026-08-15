"""team-awareness/1 — Inbound derivation: ``core/context.py::inbound_ids``.

``related`` is a pure function of a node's own body, recomputed and overwritten
on every ``note``/``task`` write (``core/notes.py``, ``core/tasks.py``, from
``[[wikilinks]]``). Nothing in the vault ever walks it backward — so a mention
in someone else's note is structurally invisible from the mentioned node's own
frontmatter. :func:`~shards.core.context.inbound_ids` (and its batched sibling
:func:`~shards.core.context._inbound_index`, which :func:`~shards.core.context._bfs`
uses for a ``--direction in``/``both`` ``graph`` query) inverts it at *read*
time: ``inbound(X) = {N : X in N.related}`` — one extra vault pass, no store, no
schema change, no daemon.

This module unit-tests the derivation function directly, isolated from the BFS
(``tests/memory/test_graph_query.py`` covers the ``--direction`` wiring: cycles,
diamonds, edge orientation, ``--direction both`` union). Coverage:

* **the load-bearing case** — a target with an empty ``related`` list of its own
  is still found via a source note that links it.
* **notes and tasks, both ways** — a note can be a backlink source or target,
  and so can a task.
* **title-form links** — ``related`` already holds the *resolved* id
  (``core.wikilinks.resolve_wikilinks`` runs at write time), so a body written
  as ``[[Some Title]]`` is exercised end-to-end via the real ``create_note`` /
  ``append_note`` write path, not by hand-writing an id into frontmatter.
* **robustness** — a malformed ``.md``, a foreign file with no shards id, and a
  ``related`` entry naming a deleted id are all skipped without aborting the
  scan (the corpus-wide invariant every reader routes through
  ``storage.files.read_post`` for).
* **no false positives** — a node not mentioned anywhere returns ``[]``.
* **determinism** — multiple backlinks to the same target come back in a
  stable (sorted) order, independent of filesystem directory-scan order.
"""

from __future__ import annotations

from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import frontmatter
import pytest

from shards.core.context import inbound_ids
from shards.core.notes import append_note, create_note
from shards.schemas.config import Config, load_config
from shards.storage.files import note_folder, task_folder


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
    extra: dict[str, Any] | None = None,
) -> Path:
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
    if extra:
        meta.update(extra)
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


# --------------------------------------------------------------------------- #
# The load-bearing case                                                        #
# --------------------------------------------------------------------------- #


def test_inbound_finds_source_whose_own_body_the_target_never_touched(
    cfg: Config, vault: Path
) -> None:
    """``t-target`` has ``related: []`` — nothing in its own frontmatter points
    anywhere. A note that mentions it is still found: this is the whole point."""
    _seed_task(vault, task_id="t-target", title="Target", related=[])
    _seed_note(vault, note_id="n-mentioner", title="Reply", related=["t-target"])

    assert inbound_ids(cfg, "t-target") == ["n-mentioner"]


def test_inbound_empty_when_nothing_points_at_target(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-lonely", title="Lonely")
    _seed_note(vault, note_id="n-other", title="Other", related=["n-lonely"])

    assert inbound_ids(cfg, "n-other") == []


def test_inbound_multiple_sources_sorted_deterministically(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-target", title="Target")
    _seed_note(vault, note_id="n-z", title="Zee", related=["n-target"])
    _seed_note(vault, note_id="n-a", title="Ay", related=["n-target"])
    _seed_note(vault, note_id="n-m", title="Em", related=["n-target"])

    assert inbound_ids(cfg, "n-target") == ["n-a", "n-m", "n-z"]


# --------------------------------------------------------------------------- #
# Notes and tasks, both as backlink source and target                          #
# --------------------------------------------------------------------------- #


def test_inbound_task_mentions_note(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-target", title="Target Note")
    _seed_task(vault, task_id="t-mentioner", title="Mentioning Task", related=["n-target"])

    assert inbound_ids(cfg, "n-target") == ["t-mentioner"]


def test_inbound_note_mentions_task(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-target", title="Target Task")
    _seed_note(vault, note_id="n-mentioner", title="Mentioning Note", related=["t-target"])

    assert inbound_ids(cfg, "t-target") == ["n-mentioner"]


def test_inbound_task_mentions_task(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-target", title="Target Task")
    _seed_task(vault, task_id="t-mentioner", title="Mentioning Task", related=["t-target"])

    assert inbound_ids(cfg, "t-target") == ["t-mentioner"]


def test_inbound_a_done_task_still_counts_as_a_source(cfg: Config, vault: Path) -> None:
    """``task_rows`` walks both ``tasks/open/`` and ``tasks/done/`` — a finished
    task's mention of a note must still be delivered."""
    _seed_note(vault, note_id="n-target", title="Target")
    _seed_task(vault, task_id="t-done", title="Finished", related=["n-target"], status="done")

    assert inbound_ids(cfg, "n-target") == ["t-done"]


# --------------------------------------------------------------------------- #
# Title-form links — real write path, not hand-authored frontmatter            #
# --------------------------------------------------------------------------- #


def test_inbound_covers_a_title_form_mention(cfg: Config, vault: Path) -> None:
    """``related`` already holds the *resolved* id by the time it hits disk
    (``core.wikilinks.resolve_wikilinks`` runs at write time) — so a mention
    written as ``[[Title]]`` is exactly as inbound-discoverable as ``[[n-id]]``.
    Goes through the real ``create_note``/``append_note`` write path."""
    target = create_note(cfg, "Target Note")

    create_note(cfg, "Reply", body=f"see [[{target.title}]] for context")

    sources = inbound_ids(cfg, target.id)
    assert len(sources) == 1
    assert sources[0].startswith("n-")


def test_inbound_reflects_an_append_not_just_the_original_body(cfg: Config, vault: Path) -> None:
    """``append_note`` recomputes ``related`` from the *whole* amended body, so a
    mention added after creation is deliverable too — this is what makes a
    reply written after the fact still notify."""
    target = create_note(cfg, "Target Note")
    mentioner = create_note(cfg, "Reply", body="no mention yet")
    assert inbound_ids(cfg, target.id) == []

    append_note(cfg, mentioner.id, f"actually, see [[{target.id}]]")

    assert inbound_ids(cfg, target.id) == [mentioner.id]


# --------------------------------------------------------------------------- #
# Robustness: skip silently, never abort the scan                              #
# --------------------------------------------------------------------------- #


def test_inbound_skips_malformed_frontmatter(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-target", title="Target")
    _seed_note(vault, note_id="n-good", title="Good", related=["n-target"])

    bad = vault / "notes" / "n-bad.md"
    bad.write_text("---\ntitle: [unterminated\n---\nbody", encoding="utf-8")

    assert inbound_ids(cfg, "n-target") == ["n-good"]


def test_inbound_skips_a_foreign_file_with_no_shards_id(cfg: Config, vault: Path) -> None:
    """A coexisting Tolaria/foreign ``.md`` — even one that happens to carry a
    ``related``-shaped key — is not a valid source (no shards id)."""
    _seed_note(vault, note_id="n-target", title="Target")

    foreign = vault / "notes" / "not-a-shard.md"
    foreign_meta = {"title": "Foreign", "related": ["n-target"]}
    foreign.write_text(
        frontmatter.dumps(frontmatter.Post("foreign body", **foreign_meta)), encoding="utf-8"
    )

    assert inbound_ids(cfg, "n-target") == []


def test_inbound_tolerates_a_related_entry_naming_a_deleted_id(cfg: Config, vault: Path) -> None:
    """A source's ``related`` list can carry a dangling id alongside the target
    — the scan must not choke on it, and the target entry still resolves."""
    _seed_note(vault, note_id="n-target", title="Target")
    _seed_note(
        vault,
        note_id="n-source",
        title="Source",
        related=["n-target", "n-deleted-ghost"],
    )

    assert inbound_ids(cfg, "n-target") == ["n-source"]


def test_inbound_tolerates_non_list_related_field(cfg: Config, vault: Path) -> None:
    """A corrupted ``related`` (wrong type, not a list) is treated as empty,
    never raised."""
    _seed_note(vault, note_id="n-target", title="Target")
    _seed_note(vault, note_id="n-weird", title="Weird", extra={"related": "n-target"})

    assert inbound_ids(cfg, "n-target") == []


def test_inbound_survives_an_empty_vault(cfg: Config) -> None:
    assert inbound_ids(cfg, "n-anything") == []
