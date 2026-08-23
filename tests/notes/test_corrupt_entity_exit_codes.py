"""One broken file yields one exit code, whatever verb reaches it.

`msgspec.ValidationError` is a `ValueError`, so an entity whose frontmatter fails
the schema used to fall into the CLI's generic validation branch (exit 2) unless a
handler happened to re-map it — and only `get` did. A script branching on exit
codes could not classify the file: 3 from `get`, 2 from `append`/`update`.

`delete` is the deliberate exception: removing a corrupt file is the repair path
and must not require the file to parse first.
"""

from __future__ import annotations

from pathlib import Path

import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app

_CORRUPT_NOTE = "---\nid: n-bad\ntype: note\ntitle: Broken\n---\n\nbody\n"
_CORRUPT_TASK = "---\nid: t-bad\ntype: task\nstatus: open\ntitle: Broken\n---\n\nbody\n"

_NOT_FOUND = 3


@pytest.fixture
def corrupt_vault(shards_config: Path, vault: Path) -> Path:
    """A vault holding one note and one task that are missing required fields."""
    (vault / "notes").mkdir(parents=True, exist_ok=True)
    (vault / "tasks" / "open").mkdir(parents=True, exist_ok=True)
    (vault / "notes" / "n-bad.md").write_text(_CORRUPT_NOTE, encoding="utf-8")
    (vault / "tasks" / "open" / "t-bad.md").write_text(_CORRUPT_TASK, encoding="utf-8")
    return vault


@pytest.mark.parametrize(
    "argv",
    [
        ["note", "get", "n-bad"],
        ["note", "append", "n-bad", "more text"],
        ["note", "update", "n-bad", "--tags", "x"],
        ["note", "update", "n-bad", "--type", "log"],
        ["graph", "n-bad"],
        ["build-context", "n-bad"],
    ],
)
def test_every_note_verb_reports_a_corrupt_note_as_not_found(
    corrupt_vault: Path, argv: list[str]
) -> None:
    result = CliRunner().invoke(app, argv)
    assert result.exit_code == _NOT_FOUND, f"{argv} -> {result.exit_code}: {result.output}"


@pytest.mark.parametrize(
    "argv",
    [
        ["task", "get", "t-bad"],
        ["task", "append", "t-bad", "more text"],
        ["task", "claim", "t-bad"],
        ["task", "release", "t-bad"],
        ["task", "finish", "t-bad"],
        ["task", "cancel", "t-bad"],
        ["task", "update", "t-bad", "--priority", "high"],
    ],
)
def test_every_task_verb_reports_a_corrupt_task_as_not_found(
    corrupt_vault: Path, argv: list[str]
) -> None:
    result = CliRunner().invoke(app, argv)
    assert result.exit_code == _NOT_FOUND, f"{argv} -> {result.exit_code}: {result.output}"


def test_delete_can_still_remove_a_corrupt_note(corrupt_vault: Path) -> None:
    """Deliberate exception: delete is how you repair a file nothing can parse."""
    result = CliRunner().invoke(app, ["note", "delete", "n-bad", "--force"])

    assert result.exit_code == 0
    assert not (corrupt_vault / "notes" / "n-bad.md").exists()


def test_delete_can_still_remove_a_corrupt_task(corrupt_vault: Path) -> None:
    result = CliRunner().invoke(app, ["task", "delete", "t-bad", "--force"])

    assert result.exit_code == 0


def test_listing_skips_a_corrupt_entity_silently(corrupt_vault: Path) -> None:
    """The whole-corpus invariant: a foreign or corrupt file never breaks a scan."""
    assert CliRunner().invoke(app, ["note", "list"]).exit_code == 0
    assert CliRunner().invoke(app, ["task", "list"]).exit_code == 0
    assert CliRunner().invoke(app, ["status"]).exit_code == 0
