"""cli-toolset-rework/4 — ``type: project`` note as a supported convention.

Projects ship as a *convention*, not a fourth verb: a ``type: project`` note is a
note like any other (root ``.spec/tech.md`` § Contracts → Folders;
``.spec/features/cli-toolset-rework/tech.md`` § Workstream C → C2). This unit pins
the two on-disk contracts the convention needs:

* the ``NoteType`` enum gains ``"project"`` (validated through ``model_validate``);
* ``storage/files.py`` routes ``type: project`` to ``notes/projects/`` alongside
  ``logs``/``decisions``/``references``, so ``note new --type project`` and the
  watcher reconcile both land the file in the right subfolder.

No new command is introduced — creation flows through the existing ``note`` verb.
"""

from __future__ import annotations

from datetime import UTC, datetime
from pathlib import Path
from typing import get_args

import frontmatter
import pytest
from typer.testing import CliRunner

from mesh.cli.__main__ import app
from mesh.core.notes import create_note, get_note
from mesh.index.reconcile import reconcile_path
from mesh.schemas.config import Config, load_config
from mesh.schemas.note import Note, NoteType
from mesh.storage.files import note_folder


@pytest.fixture
def cfg(mesh_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _now() -> datetime:
    return datetime(2026, 7, 3, 12, 0, 0, tzinfo=UTC)


# --------------------------------------------------------------------------- #
# NoteType enum                                                                #
# --------------------------------------------------------------------------- #


def test_note_type_enum_includes_project() -> None:
    assert "project" in get_args(NoteType)


def test_note_model_validates_project_type() -> None:
    note = Note.model_validate(
        {
            "id": "n-proj",
            "type": "project",
            "title": "Q3 Launch",
            "created": _now(),
            "updated": _now(),
        }
    )
    assert note.type == "project"


# --------------------------------------------------------------------------- #
# note_folder routing                                                          #
# --------------------------------------------------------------------------- #


def test_note_folder_routes_project_to_projects_dir(tmp_path: Path) -> None:
    base = tmp_path / "vault"
    assert note_folder("project", base) == base / "notes" / "projects"


def test_create_note_project_routes_to_projects_folder(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "Q3 Launch", note_type="project", body="scope")
    assert note.type == "project"
    assert (vault / "notes" / "projects" / f"{note.id}.md").exists()


def test_cli_note_new_project_type(cfg: Config, vault: Path) -> None:
    result = _invoke(["--quiet", "note", "new", "Q3 Launch", "--type", "project", "--body", "x"])
    assert result.exit_code == 0, result.output
    note_id = result.output.strip()
    assert (vault / "notes" / "projects" / f"{note_id}.md").exists()


def test_project_note_readable_through_note_verb(cfg: Config, vault: Path) -> None:
    note = create_note(cfg, "Q3 Launch", note_type="project", body="scope")
    view = get_note(cfg, note.id)
    assert view.note.type == "project"


# --------------------------------------------------------------------------- #
# reconcile — a project note routes into notes/projects/ like other types      #
# --------------------------------------------------------------------------- #


def test_reconcile_moves_misplaced_project_note(cfg: Config, vault: Path) -> None:
    # A project note wrongly sitting in notes/ root must move to notes/projects/.
    when = _now()
    meta: dict[str, object] = {
        "id": "n-proj",
        "type": "project",
        "title": "Q3 Launch",
        "tags": [],
        "owner": "seed-agent",
        "created": when,
        "updated": when,
        "related": [],
    }
    path = vault / "notes" / "n-proj.md"
    post = frontmatter.Post("body")
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")

    final = reconcile_path(cfg, path)

    assert final == (vault / "notes" / "projects" / "n-proj.md").resolve()
    assert final.exists()
    assert not path.exists()
