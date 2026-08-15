"""core-hardening/4 — fallback threshold applies only when explicitly set.

`index/fallback.py::search_fallback` scores every corpus file by a fixed,
pinned tier matrix (title exact 1.0 / title substring 0.8 / tag contains 0.6 /
body substring 0.4 — root tech.md § Implemented surfaces) and drops anything
below the effective threshold. Before this fix, the *default* `[search].threshold`
(0.65, `schemas/config.py:52`) silently dropped the whole body tier at default
configuration — the documented first-run state with no `indexed` binary — so
`search "eTA"` returned `[]` for a note whose only match was in its body.

The fix (root tech.md § B5) changes the threshold-*application* rule, not the
matrix: the threshold filters only when a caller set it explicitly — via
`--threshold` on the CLI, or an explicit `[search].threshold` key in
`config.toml`. With no explicit value the fallback uses its own floor, the
lowest matrix tier (0.4), so every tier is reachable out of the box.

These tests exercise the real `shards search` CLI over a real temp vault with
no `indexed` binary on PATH and no daemon running — the substring fallback is
the only path available, exactly the "fresh install" scenario the defect
report describes. Nothing here patches the scorer.
"""

from __future__ import annotations

import json
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app
from shards.index.fallback import search_fallback
from shards.schemas.config import Config, load_config

_NOTICE = "search: using substring fallback (indexed unavailable)"


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _seed_body_only_hit(vault: Path) -> None:
    """A note whose title/tags don't match but whose body contains 'eTA'."""
    meta = {
        "id": "n-visa",
        "type": "note",
        "title": "Travel Notes",
        "tags": ["travel"],
        "owner": "seed-agent",
        "created": "2026-06-01T00:00:00+00:00",
        "updated": "2026-06-01T00:00:00+00:00",
        "related": [],
    }
    body = "Remember to apply for the eTA before the flight."
    folder = vault / "notes"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "n-visa.md").write_text(
        frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8"
    )


# --------------------------------------------------------------------------- #
# A config.toml with no [search].threshold key at all — the fresh-install case #
# --------------------------------------------------------------------------- #


@pytest.fixture
def default_threshold_config(
    vault: Path, config_path: Path, monkeypatch: pytest.MonkeyPatch
) -> Path:
    """Config pointed at ``vault`` whose ``[search]`` section omits ``threshold``."""
    config_path.write_text(
        "\n".join(
            (
                "[core]",
                f'tolaria_path = "{vault}"',
                'agent = "test-agent"',
                "",
                "[search]",
                'collection = "test-vault"',
                "hybrid = true",
                "",
                "[tasks]",
                'collections = ["test-agent"]',
                "",
            )
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(config_path))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    return config_path


@pytest.fixture
def cfg_default_threshold(default_threshold_config: Path) -> Config:
    return load_config()


# --------------------------------------------------------------------------- #
# Scenario 1 — default config, no indexed: body match is returned              #
# --------------------------------------------------------------------------- #


def test_default_config_no_indexed_returns_body_hit(
    default_threshold_config: Path, vault: Path
) -> None:
    """The exact defect repro: `search "eTA"` on a fresh install now returns the hit."""
    _seed_body_only_hit(vault)
    result = _invoke(["search", "eTA"])
    assert result.exit_code == 0, result.output
    hits = json.loads(result.stdout)
    assert {h["id"] for h in hits} == {"n-visa"}
    assert hits[0]["score"] == 0.4
    assert _NOTICE in result.stderr  # confirms this ran the substring fallback


def test_search_fallback_default_returns_body_hit(
    cfg_default_threshold: Config, vault: Path
) -> None:
    _seed_body_only_hit(vault)
    results = search_fallback(cfg_default_threshold, "eTA")
    assert {r.id for r in results} == {"n-visa"}
    assert results[0].score == 0.4


def test_config_threshold_not_explicit_by_default(cfg_default_threshold: Config) -> None:
    assert cfg_default_threshold.search.threshold_explicit() is False


# --------------------------------------------------------------------------- #
# Scenario 2 — --threshold on the CLI still filters, even over a default config #
# --------------------------------------------------------------------------- #


def test_cli_threshold_flag_still_excludes_body_hit(
    default_threshold_config: Path, vault: Path
) -> None:
    _seed_body_only_hit(vault)
    result = _invoke(["search", "eTA", "--threshold", "0.7"])
    assert result.exit_code == 0, result.output
    assert json.loads(result.stdout) == []


def test_cli_threshold_flag_lower_than_floor_still_included(
    default_threshold_config: Path, vault: Path
) -> None:
    _seed_body_only_hit(vault)
    result = _invoke(["search", "eTA", "--threshold", "0.1"])
    assert {h["id"] for h in json.loads(result.stdout)} == {"n-visa"}


# --------------------------------------------------------------------------- #
# Scenario 3 — an explicit config.toml threshold behaves exactly as today      #
# --------------------------------------------------------------------------- #


def test_explicit_config_threshold_065_behaves_as_today(shards_config: Path, vault: Path) -> None:
    # `shards_config` (tests/conftest.py) writes an explicit [search].threshold = 0.65.
    cfg = load_config()
    assert cfg.search.threshold_explicit() is True
    _seed_body_only_hit(vault)
    result = _invoke(["search", "eTA"])
    assert result.exit_code == 0, result.output
    assert json.loads(result.stdout) == []  # explicit 0.65 still excludes the 0.4 body hit


# --------------------------------------------------------------------------- #
# Scenario 4 — the tier matrix itself is unchanged                             #
# --------------------------------------------------------------------------- #


def test_tier_matrix_unchanged(cfg_default_threshold: Config, vault: Path) -> None:
    """Direct scoring check: all four tiers keep their pinned values."""
    meta_base = {
        "type": "note",
        "owner": "seed-agent",
        "created": "2026-06-01T00:00:00+00:00",
        "updated": "2026-06-01T00:00:00+00:00",
        "related": [],
    }
    folder = vault / "notes"
    folder.mkdir(parents=True, exist_ok=True)

    def _write(entry_id: str, title: str, tags: list[str], body: str) -> None:
        meta = {"id": entry_id, "title": title, "tags": tags, **meta_base}
        (folder / f"{entry_id}.md").write_text(
            frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8"
        )

    _write("n-exact", "Matrix Probe", [], "irrelevant body")
    _write("n-sub", "Prefixed Matrix Probe Suffix", [], "irrelevant body")
    _write("n-tag", "Untitled", ["matrix probe"], "irrelevant body")
    _write("n-body", "Untitled", [], "the matrix probe hides in the body")

    # threshold=None (no explicit value): floor is the lowest tier, so every
    # hit — including the body-only one — is returned; score confirms the tier.
    by_id = {r.id: r.score for r in search_fallback(cfg_default_threshold, "matrix probe")}
    assert by_id == {"n-exact": 1.0, "n-sub": 0.8, "n-tag": 0.6, "n-body": 0.4}
