"""A fresh `shards init` install must still recall on body and tag text.

Regression for the round-2 review finding: the substring fallback was changed
to apply `[search].threshold` only when a caller set it explicitly, but
`shards init` wrote `threshold = 0.65` into every config it generated — which
made it explicit again and restored the pre-fix cutoff. The fallback's body
tier scores 0.4 and its tag tier 0.6, both below 0.65, so body-only and
tag-only matches were unreachable on every fresh install.

Everything here goes through `shards init` deliberately: a hand-written config
fixture cannot catch a defect that lives in the generator itself.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app

# A nonsense token, so a hit can only come from the tier under test — never
# from the title-exact (1.0) or title-substring (0.8) tiers above the cutoff.
_TOKEN = "zqxfoo"


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


@pytest.fixture
def fresh_install(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """No config at all -> `shards init` -> a working vault. Returns the vault."""
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(tmp_path / "config.toml"))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    vault = tmp_path / "vault"
    result = _invoke(["init", "--path", str(vault), "--agent", "fresh-agent"])
    assert result.exit_code == 0, result.output
    return vault


def test_body_only_match_is_returned_after_init(fresh_install: Path) -> None:
    new_result = _invoke(
        ["--quiet", "note", "new", "Unrelated Title", "--body", f"a {_TOKEN} b"]
    )
    assert new_result.exit_code == 0, new_result.output
    note_id = new_result.stdout.strip()

    result = _invoke(["search", _TOKEN])
    assert result.exit_code == 0, result.output
    hits = json.loads(result.stdout)
    assert [hit["id"] for hit in hits] == [note_id]


def test_tag_only_match_is_returned_after_init(fresh_install: Path) -> None:
    new_result = _invoke(
        ["--quiet", "note", "new", "Unrelated Title", "--tags", _TOKEN, "--body", "x"]
    )
    assert new_result.exit_code == 0, new_result.output
    note_id = new_result.stdout.strip()

    result = _invoke(["search", _TOKEN])
    assert result.exit_code == 0, result.output
    hits = json.loads(result.stdout)
    assert [hit["id"] for hit in hits] == [note_id]
