"""core-hardening/4 — timestamp drift: every emitted timestamp is ``...Z``.

Model dumps (``schemas/note.py::_iso_z``) already rendered UTC as a ``Z`` suffix;
``cli/_output.py::emit_mutation`` and ``core/search.py::hit_dict`` instead emitted
raw ``datetime.isoformat()`` (``+00:00``). This file locks the fix in at both the
unit level (the local ``_iso_z`` helpers) and through the CLI JSON surfaces that
call them.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime
from pathlib import Path

from typer.testing import CliRunner

from shards.cli.__main__ import app
from shards.cli._output import _iso_z as output_iso_z
from shards.core.search import _iso_z as search_iso_z

_AWARE = datetime(2026, 6, 1, 12, 30, 0, tzinfo=UTC)
_NAIVE = datetime(2026, 6, 1, 12, 30, 0)


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


# --------------------------------------------------------------------------- #
# Unit — the local _iso_z helpers                                              #
# --------------------------------------------------------------------------- #


def test_output_iso_z_renders_z_suffix() -> None:
    text = output_iso_z(_AWARE)
    assert text.endswith("Z")
    assert "+00:00" not in text


def test_search_iso_z_renders_z_suffix() -> None:
    text = search_iso_z(_AWARE)
    assert text.endswith("Z")
    assert "+00:00" not in text


def test_output_iso_z_and_search_iso_z_agree() -> None:
    assert output_iso_z(_AWARE) == search_iso_z(_AWARE)


def test_iso_z_leaves_naive_datetime_unchanged() -> None:
    # No offset to swap — matches schemas/note.py's _iso_z convention exactly.
    assert output_iso_z(_NAIVE) == _NAIVE.isoformat()


# --------------------------------------------------------------------------- #
# CLI integration — emit_mutation (cli/_output.py) via `--json`                #
# --------------------------------------------------------------------------- #


def test_cli_json_note_new_updated_is_z_suffixed(shards_config: Path, vault: Path) -> None:
    result = _invoke(["--json", "note", "new", "Timestamp Check", "--body", "x"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["updated"].endswith("Z")
    assert "+00:00" not in obj["updated"]


def test_cli_json_task_new_updated_is_z_suffixed(shards_config: Path, vault: Path) -> None:
    result = _invoke(["--json", "task", "new", "Timestamp Check"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["updated"].endswith("Z")
    assert "+00:00" not in obj["updated"]


# --------------------------------------------------------------------------- #
# CLI integration — search hit_dict (core/search.py) via `--json`              #
# --------------------------------------------------------------------------- #


def test_cli_search_hit_updated_is_z_suffixed(shards_config: Path, vault: Path) -> None:
    new_result = _invoke(["--quiet", "note", "new", "Findable Note", "--body", "x"])
    assert new_result.exit_code == 0, new_result.output
    result = _invoke(["search", "Findable Note"])
    assert result.exit_code == 0, result.output
    hits = json.loads(result.stdout)
    assert hits and "updated" in hits[0]
    assert hits[0]["updated"].endswith("Z")
    assert "+00:00" not in hits[0]["updated"]
