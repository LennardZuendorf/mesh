"""core-hardening/1 — safe reader symmetry (R1).

``storage.files.read_post`` is the project's single safe reader: it maps both
``OSError`` (a vanished/unreadable file) and ``yaml.YAMLError`` (malformed
frontmatter) to a silent skip. Before this unit, the task-side write and list
paths hand-rolled ``frontmatter.loads(path.read_text())`` and caught only
``yaml.YAMLError``, so a file that raised ``OSError`` on read tracebacked out
of ``task list``, ``session-start``, and ``status`` while ``note list``
(already routed through ``read_post``) survived the identical vault.

These tests pin the fix at the CLI boundary: identical skip behaviour across
every reader, parametrised over the note/task symmetry the regression broke.
``.spec/lessons.md``'s "Foreign-file tolerance must be symmetric across every
reader" rule is exactly what this test suite exists to keep honest.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from typer.testing import CliRunner

from mesh.cli.__main__ import app
from mesh.schemas.config import Config, load_config

pytestmark = pytest.mark.usefixtures("mesh_config")


@pytest.fixture
def cfg(mesh_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _make_os_error_file(path: Path) -> None:
    """Force ``OSError`` on read: a directory sitting where a ``.md`` file is expected.

    Portable across CI regardless of user/root: permission bits are unreliable
    when the suite runs as root (root can read a chmod-000 file), but a
    directory always raises ``IsADirectoryError`` (an ``OSError`` subclass) on
    ``Path.read_text()`` — the same failure mode a vanished-mid-scan or
    permission-denied file produces in production.
    """
    path.mkdir(parents=True)


# --------------------------------------------------------------------------- #
# Symmetry — task list and note list skip an OSError-raising file identically #
# --------------------------------------------------------------------------- #


@pytest.mark.parametrize(
    ("verb", "good_title", "bad_subdir"),
    [
        ("task", "Good Task", ("tasks", "open")),
        ("note", "Good Note", ("notes",)),
    ],
)
def test_list_skips_os_error_file(
    cfg: Config,
    vault: Path,
    verb: str,
    good_title: str,
    bad_subdir: tuple[str, ...],
) -> None:
    """``<verb> list`` exits 0 and keeps every readable entity when a sibling raises OSError."""
    created = _invoke(["--quiet", verb, "new", good_title, "--body", "seed body"])
    assert created.exit_code == 0, created.output
    good_id = created.output.strip()

    _make_os_error_file(vault.joinpath(*bad_subdir, "z-broken.md"))

    result = _invoke(["--quiet", verb, "list"])
    assert result.exit_code == 0, result.output
    assert result.output.split() == [good_id]


# --------------------------------------------------------------------------- #
# task list / session-start / status all survive an OSError-raising task file #
# --------------------------------------------------------------------------- #


def test_task_list_survives_os_error_file(cfg: Config, vault: Path) -> None:
    created = _invoke(["--quiet", "task", "new", "Good Task"])
    good_id = created.output.strip()
    _make_os_error_file(vault / "tasks" / "open" / "z-broken.md")

    result = _invoke(["--json", "task", "list"])
    assert result.exit_code == 0, result.output
    ids = [row["id"] for row in json.loads(result.output)]
    assert ids == [good_id]


def test_session_start_survives_os_error_file(cfg: Config, vault: Path) -> None:
    created = _invoke(["--quiet", "task", "new", "Good Task"])
    good_id = created.output.strip()
    _make_os_error_file(vault / "tasks" / "open" / "z-broken.md")

    result = _invoke(["session-start", "--json"])
    assert result.exit_code == 0, result.output
    ids = [row["id"] for row in json.loads(result.output)]
    assert good_id in ids


def test_status_survives_os_error_file(cfg: Config, vault: Path) -> None:
    _invoke(["--quiet", "task", "new", "Good Task"])
    _make_os_error_file(vault / "tasks" / "open" / "z-broken.md")

    result = _invoke(["--json", "status"])
    assert result.exit_code == 0, result.output
    report = json.loads(result.output)
    # The unreadable sibling is skipped, not counted and not fatal.
    assert report["tasks_total"] == 1
