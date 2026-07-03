"""Scaffold smoke test — the CLI imports and reports its version."""

from __future__ import annotations

from typer.testing import CliRunner

from brain import __version__
from brain.cli.__main__ import app


def test_version_matches_package() -> None:
    result = CliRunner().invoke(app, ["--version"])
    assert result.exit_code == 0
    assert __version__ in result.stdout
