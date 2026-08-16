"""notes/2 — ``note new``: create verb (R1).

Exercises R1 (Create): :func:`shards.core.notes.create_note` and the
``shards note new`` CLI surface. Create generates a hash ``n-`` id, routes the
file into the folder matching its ``type`` (``notes/`` for ``note``;
``notes/{logs,decisions,references}/`` for the typed variants), validates the
frontmatter against :class:`shards.schemas.note.Note`, and writes atomically with
``created == updated`` at birth. Body source precedence is ``--body`` → ``--file``
→ ``$EDITOR`` (TTY only); a headless path (``--json``/MCP or non-TTY) with neither
``--body`` nor ``--file`` refuses (exit 2) rather than launching ``$EDITOR``. The
default owner is the resolved config agent (``$SHARDS_AGENT`` override applied);
an explicit ``--owner`` outside ``[tasks].collections`` is rejected (exit 2).
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app
from shards.core.notes import create_note, find_duplicate_title, get_note
from shards.schemas.config import Config, load_config
from shards.storage.files import note_folder

# n- prefix + one-or-more Crockford base-32 digits (no I, L, O, U), 4+ long.
_ID_RE = re.compile(r"^n-[0123456789ABCDEFGHJKMNPQRSTVWXYZ]{4,}$")


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str], *, input: str | None = None):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args, input=input)


# --------------------------------------------------------------------------- #
# create_note (core)                                                          #
# --------------------------------------------------------------------------- #


def test_create_note_id_shape(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "CLID Fallback", body="Body.")
    assert note.id.startswith("n-")
    assert _ID_RE.match(note.id), note.id
    # File is named <id>.md and lives in the note folder.
    assert (note_folder("note", vault) / f"{note.id}.md").exists()


def test_create_note_routes_decision_folder(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "A Decision", note_type="decision", body="x")
    path = vault / "notes" / "decisions" / f"{note.id}.md"
    assert path.exists()
    assert note.type == "decision"


@pytest.mark.parametrize(
    ("note_type", "subdir"),
    [
        ("note", ("notes",)),
        ("log", ("notes", "logs")),
        ("decision", ("notes", "decisions")),
        ("reference", ("notes", "references")),
    ],
)
def test_create_note_folder_routing_per_type(
    cfg: Config, vault: Path, note_type: str, subdir: tuple[str, ...]
) -> None:
    note = create_note(cfg, f"Typed {note_type}", note_type=note_type, body="x")
    assert (vault.joinpath(*subdir) / f"{note.id}.md").exists()


def test_create_note_created_equals_updated(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "Fresh", body="x")
    assert note.created == note.updated


def test_create_note_default_owner_from_config(cfg: Config, vault: Path) -> None:
    # shards_config sets [core].agent = "test-agent".
    note = create_note(cfg, "Owned", body="x")
    assert note.owner == "test-agent"


def test_create_note_explicit_owner(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "Owned", owner="other-agent", body="x")
    assert note.owner == "other-agent"


def test_create_note_rejects_owner_outside_collections_in_core(cfg: Config, vault: Path) -> None:
    """The owner rule is enforced in core (not just the CLI), so MCP/daemon
    note writes get it too — and nothing is written on rejection."""
    with pytest.raises(ValueError, match="unknown owner"):
        create_note(cfg, "Ghost", owner="ghost-agent", body="x")
    assert list((vault / "notes").rglob("n-*.md")) == []


def test_create_note_tags_and_timestamps_present(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "Tagged", tags=["ndc", "flights"], body="x")
    assert note.tags == ["ndc", "flights"]
    assert note.created is not None
    assert note.updated is not None


def test_create_note_invalid_type_raises(cfg: Config, vault: Path) -> None:
    with pytest.raises(ValueError):
        create_note(cfg, "Bad", note_type="journal", body="x")


def test_create_note_writes_only_canonical_keys(cfg: Config, vault: Path) -> None:
    """Clean Markdown: create injects no machinery beyond the agreed keys."""
    note = create_note(cfg, "Clean", tags=["a"], body="just a body")
    path = note_folder("note", vault) / f"{note.id}.md"
    meta = frontmatter.loads(path.read_text(encoding="utf-8")).metadata
    assert set(meta) == {
        "id",
        "type",
        "title",
        "tags",
        "owner",
        "created",
        "updated",
        "related",
    }


def test_create_note_roundtrips_unknown_frontmatter_key(cfg: Config, vault: Path) -> None:
    """An unknown key injected into a created note survives a read-back."""
    note = create_note(cfg, "Round Trip", body="x")
    path = note_folder("note", vault) / f"{note.id}.md"
    post = frontmatter.loads(path.read_text(encoding="utf-8"))
    post.metadata["tolaria_pinned"] = True
    path.write_text(frontmatter.dumps(post), encoding="utf-8")

    view = get_note(cfg, note.id)
    dumped = view.note.model_dump()
    assert dumped.get("tolaria_pinned") is True


def test_create_note_resolves_wikilinks_into_related(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "Linker", body="see [[n-a3f2]] and [[t-99]]")
    assert note.related == ["n-a3f2", "t-99"]


# --------------------------------------------------------------------------- #
# CLI — shards note new                                                        #
# --------------------------------------------------------------------------- #


def test_cli_new_creates_file(cfg: Config, vault: Path) -> None:
    result = _invoke(["--quiet", "note", "new", "CLI Note", "--body", "hello"])
    assert result.exit_code == 0, result.output
    note_id = result.output.strip()
    assert note_id.startswith("n-")
    assert (note_folder("note", vault) / f"{note_id}.md").exists()


def test_cli_new_decision_folder_routing(cfg: Config, vault: Path) -> None:
    result = _invoke(["--quiet", "note", "new", "A Decision", "--type", "decision", "--body", "x"])
    assert result.exit_code == 0, result.output
    note_id = result.output.strip()
    assert (vault / "notes" / "decisions" / f"{note_id}.md").exists()


def test_cli_new_headless_no_body_exits_2(cfg: Config, vault: Path) -> None:
    """--json (headless) with neither --body nor --file must refuse, not open $EDITOR."""
    result = _invoke(["--json", "note", "new", "No Body"])
    assert result.exit_code == 2, result.output
    # No file created.
    assert list((vault / "notes").rglob("n-*.md")) == []


def test_cli_new_non_tty_no_body_exits_2(cfg: Config, vault: Path) -> None:
    """CliRunner stdin is not a TTY -> headless path, so no body still refuses."""
    result = _invoke(["note", "new", "No Body"])
    assert result.exit_code == 2, result.output


def test_cli_new_shards_agent_default_owner(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """$SHARDS_AGENT overrides [core].agent as the default owner."""
    monkeypatch.setenv("SHARDS_AGENT", "flights-agent")
    result = _invoke(["--quiet", "note", "new", "Agent Owned", "--body", "x"])
    assert result.exit_code == 0, result.output
    note_id = result.output.strip()
    meta = frontmatter.loads(
        (note_folder("note", vault) / f"{note_id}.md").read_text(encoding="utf-8")
    ).metadata
    assert meta["owner"] == "flights-agent"


def test_cli_new_explicit_valid_owner(cfg: Config, vault: Path) -> None:
    result = _invoke(["--quiet", "note", "new", "Owned", "--owner", "other-agent", "--body", "x"])
    assert result.exit_code == 0, result.output
    note_id = result.output.strip()
    meta = frontmatter.loads(
        (note_folder("note", vault) / f"{note_id}.md").read_text(encoding="utf-8")
    ).metadata
    assert meta["owner"] == "other-agent"


def test_cli_new_unknown_owner_exits_2(cfg: Config, vault: Path) -> None:
    """An --owner outside [tasks].collections is rejected (exit 2)."""
    result = _invoke(["note", "new", "Ghost", "--owner", "ghost-agent", "--body", "x"])
    assert result.exit_code == 2, result.output
    assert list((vault / "notes").rglob("n-*.md")) == []


def test_cli_new_body_from_file(cfg: Config, vault: Path, tmp_path: Path) -> None:
    src = tmp_path / "body.md"
    src.write_text("BODY-FROM-FILE", encoding="utf-8")
    result = _invoke(["--quiet", "note", "new", "From File", "--file", str(src)])
    assert result.exit_code == 0, result.output
    note_id = result.output.strip()
    content = (note_folder("note", vault) / f"{note_id}.md").read_text(encoding="utf-8")
    assert "BODY-FROM-FILE" in content


def test_cli_new_body_wins_over_file(cfg: Config, vault: Path, tmp_path: Path) -> None:
    """Body precedence: --body overrides --file when both are supplied."""
    src = tmp_path / "body.md"
    src.write_text("FROM-FILE", encoding="utf-8")
    result = _invoke(["--quiet", "note", "new", "Both", "--body", "FROM-BODY", "--file", str(src)])
    assert result.exit_code == 0, result.output
    note_id = result.output.strip()
    content = (note_folder("note", vault) / f"{note_id}.md").read_text(encoding="utf-8")
    assert "FROM-BODY" in content
    assert "FROM-FILE" not in content


def test_cli_new_missing_file_exits_2(cfg: Config, vault: Path, tmp_path: Path) -> None:
    missing = tmp_path / "nope.md"
    result = _invoke(["note", "new", "Missing", "--file", str(missing)])
    assert result.exit_code == 2, result.output


def test_cli_new_tags_and_timestamps(cfg: Config, vault: Path) -> None:
    result = _invoke(["--quiet", "note", "new", "Tagged", "--tags", "ndc,flights", "--body", "x"])
    assert result.exit_code == 0, result.output
    note_id = result.output.strip()
    meta = frontmatter.loads(
        (note_folder("note", vault) / f"{note_id}.md").read_text(encoding="utf-8")
    ).metadata
    assert meta["tags"] == ["ndc", "flights"]
    assert meta["created"] is not None
    assert meta["updated"] is not None
    assert meta["created"] == meta["updated"]


# --------------------------------------------------------------------------- #
# find_duplicate_title / duplicate-title warning at create (R9)                #
# --------------------------------------------------------------------------- #


def test_find_duplicate_title_exact_match(cfg: Config, vault: Path) -> None:
    first = create_note(cfg, "Japan visa requirements for Q3 trip", body="x")
    assert find_duplicate_title(cfg, "Japan visa requirements for Q3 trip") == first.id


def test_find_duplicate_title_no_match_returns_none(cfg: Config, vault: Path) -> None:
    create_note(cfg, "Existing Title", body="x")
    assert find_duplicate_title(cfg, "Unrelated Title") is None


def test_find_duplicate_title_case_and_whitespace_do_not_collide(cfg: Config, vault: Path) -> None:
    """Mirrors ``wikilinks._title_index``'s exact-match rule: a title differing
    only by case or surrounding whitespace is a *different* string, not a
    collision — the same exact-match rule the wikilink title index uses, not
    the slug resolver's normalized rule (asserted, not incidental)."""
    create_note(cfg, "Japan Visa", body="x")
    assert find_duplicate_title(cfg, "japan visa") is None
    assert find_duplicate_title(cfg, "JAPAN VISA") is None
    assert find_duplicate_title(cfg, " Japan Visa ") is None


def test_find_duplicate_title_ignores_tasks(cfg: Config, vault: Path) -> None:
    """Same-kind only: a task with the same title is invisible to the note check."""
    from shards.core.tasks import create_task

    create_task(cfg, "Shared Title")
    assert find_duplicate_title(cfg, "Shared Title") is None


def test_create_note_duplicate_title_still_succeeds(cfg: Config, vault: Path) -> None:
    """Non-blocking: creating a second note with an existing title still creates
    it (exit 0 equivalent at the core layer — no exception, id returned, file on
    disk) rather than refusing."""
    first = create_note(cfg, "Japan visa requirements for Q3 trip", body="x")
    second = create_note(cfg, "Japan visa requirements for Q3 trip", body="y")
    assert second.id != first.id
    assert (note_folder("note", vault) / f"{second.id}.md").exists()
    assert (note_folder("note", vault) / f"{first.id}.md").exists()


# --------------------------------------------------------------------------- #
# CLI — duplicate-title warning (R9)                                          #
# --------------------------------------------------------------------------- #


def test_cli_new_duplicate_title_warns_and_still_creates(cfg: Config, vault: Path) -> None:
    """Load-bearing: the create SUCCEEDS (exit 0, id on stdout, file on disk)
    *and* a warning naming the prior id lands on stderr."""
    first = _invoke(
        ["--quiet", "note", "new", "Japan visa requirements for Q3 trip", "--body", "x"]
    )
    assert first.exit_code == 0, first.output
    first_id = first.output.strip()

    second = _invoke(["note", "new", "Japan visa requirements for Q3 trip", "--body", "y"])
    assert second.exit_code == 0, second.output
    assert first_id in second.stderr
    assert "duplicate title" in second.stderr
    second_id = second.output.strip().split()[-1]
    assert (note_folder("note", vault) / f"{second_id}.md").exists()


def test_cli_new_unique_title_emits_no_warning(cfg: Config, vault: Path) -> None:
    result = _invoke(["note", "new", "A Wholly Unique Title", "--body", "x"])
    assert result.exit_code == 0, result.output
    assert result.stderr == ""


def test_cli_new_duplicate_title_quiet_suppresses_warning(cfg: Config, vault: Path) -> None:
    _invoke(["--quiet", "note", "new", "Repeat Title", "--body", "x"])
    second = _invoke(["--quiet", "note", "new", "Repeat Title", "--body", "y"])
    assert second.exit_code == 0, second.output
    assert second.stderr == ""


def test_cli_new_duplicate_title_json_never_carries_warning(cfg: Config, vault: Path) -> None:
    """``--json`` payload never carries the advisory text; the warning still
    reaches stderr (only ``--quiet`` suppresses it)."""
    _invoke(["--quiet", "note", "new", "JSON Dup", "--body", "x"])
    second = _invoke(["--json", "note", "new", "JSON Dup", "--body", "y"])
    assert second.exit_code == 0, second.output
    obj = json.loads(second.stdout)
    assert "warning" not in json.dumps(obj)
    assert "duplicate title" in second.stderr


def test_cli_new_note_task_same_title_no_warning(cfg: Config, vault: Path) -> None:
    """A note and a task sharing a title do not warn (same-kind only, R9)."""
    task_result = _invoke(["--quiet", "task", "new", "Cross-Kind Title"])
    assert task_result.exit_code == 0, task_result.output

    note_result = _invoke(["note", "new", "Cross-Kind Title", "--body", "x"])
    assert note_result.exit_code == 0, note_result.output
    assert note_result.stderr == ""
