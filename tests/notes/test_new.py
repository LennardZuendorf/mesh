"""notes/2 — ``note new``: create verb (R1).

Exercises R1 (Create): :func:`brain.core.notes.create_note` and the
``brain note new`` CLI surface. Create generates a hash ``n-`` id, routes the
file into the folder matching its ``type`` (``notes/`` for ``note``;
``notes/{logs,decisions,references}/`` for the typed variants), validates the
frontmatter against :class:`brain.schemas.note.Note`, and writes atomically with
``created == updated`` at birth. Body source precedence is ``--body`` → ``--file``
→ ``$EDITOR`` (TTY only); a headless path (``--json``/MCP or non-TTY) with neither
``--body`` nor ``--file`` refuses (exit 2) rather than launching ``$EDITOR``. The
default owner is the resolved config agent (``$BRAIN_AGENT`` override applied);
an explicit ``--owner`` outside ``[tasks].collections`` is rejected (exit 2).
"""

from __future__ import annotations

import re
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

from brain.cli.__main__ import app
from brain.core.notes import create_note, get_note
from brain.schemas.config import Config, load_config
from brain.storage.files import note_folder

# n- prefix + one-or-more Crockford base-32 digits (no I, L, O, U), 4+ long.
_ID_RE = re.compile(r"^n-[0123456789ABCDEFGHJKMNPQRSTVWXYZ]{4,}$")


@pytest.fixture
def cfg(brain_config: Path) -> Config:
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
    # brain_config sets [core].agent = "test-agent".
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
# CLI — brain note new                                                        #
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


def test_cli_new_brain_agent_default_owner(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """$BRAIN_AGENT overrides [core].agent as the default owner."""
    monkeypatch.setenv("BRAIN_AGENT", "flights-agent")
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
    result = _invoke(
        ["--quiet", "note", "new", "Both", "--body", "FROM-BODY", "--file", str(src)]
    )
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
