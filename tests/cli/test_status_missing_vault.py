"""Round-2 review — a mistyped `[core].vault_path` must be visible in `status`.

Nothing checked that the configured vault existed: `shards status` on a typoed
path reported `notes: 0`, `freshness: (no vault files)` and exit 0, and the
next `note new` silently materialised a whole parallel vault at the typo via
`atomic_write`'s `mkdir(parents=True)`.

Lazy creation is `shards init`'s documented behaviour, so this is a
*visibility* fix and not a hard failure: `status` names the vault and marks it
missing, on both the human and the `--json` surface, and writes still work.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app
from tests.cli._vault_config import point_at


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


@pytest.fixture
def missing_vault(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """A config whose vault path is a typo — nothing exists there."""
    vault = tmp_path / "typoed-vault"
    point_at(monkeypatch, tmp_path / "config.toml", vault)
    return vault


def test_status_reports_a_missing_vault(missing_vault: Path) -> None:
    result = _invoke(["status"])
    assert result.exit_code == 0, result.output
    assert f"vault: {missing_vault} (does not exist)" in result.stdout


def test_status_json_reports_a_missing_vault(missing_vault: Path) -> None:
    result = _invoke(["--json", "status"])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert payload["vault"] == {"path": str(missing_vault), "exists": False}


def test_status_names_a_present_vault_without_the_marker(
    tmp_path: Path, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    point_at(monkeypatch, tmp_path / "config.toml", vault)
    result = _invoke(["status"])
    assert result.exit_code == 0, result.output
    assert f"vault: {vault}" in result.stdout
    assert "does not exist" not in result.stdout


def test_missing_vault_does_not_block_writes(missing_vault: Path) -> None:
    """Lazy creation is `init`'s documented behaviour — status warns, nothing fails."""
    result = _invoke(["note", "new", "Lazy Vault", "--body", "x"])
    assert result.exit_code == 0, result.output
    assert missing_vault.is_dir()
