"""``search --health`` — surfacing indexed reachability vs. substring fallback.

Silent degradation (root ``tech.md`` § Risks — "``indexed`` drift") means a
caller can't tell whether a result set came from the hybrid ``indexed`` engine
or the substring fallback without reading stderr notices closely. ``--health``
makes the four gates that decide the recall path (``[search].hybrid``,
``[search].collection``, daemon liveness, ``indexed`` binary presence)
explicit in one JSON payload, both via :func:`shards.core.search.search_health`
directly and through the ``shards search --health`` CLI flag — and it must
degrade cleanly (never raise) even when ``indexed`` is entirely absent.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app
from shards.core.search import search_health
from shards.index import indexed_client
from shards.schemas.config import Config, load_config


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


# --------------------------------------------------------------------------- #
# search_health(): the core signal                                            #
# --------------------------------------------------------------------------- #


def test_health_reports_indexed_mode_when_every_gate_is_open(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: True)
    report = search_health(cfg)
    assert report["mode"] == "indexed"
    assert report["hybrid_configured"] is True
    assert report["collection"] == "test-vault"
    assert report["daemon_up"] is True
    assert report["indexed_binary_available"] is True
    assert "reason" not in report


def test_health_reports_fallback_when_hybrid_disabled(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    cfg.search.hybrid = False
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: True)
    report = search_health(cfg)
    assert report["mode"] == "fallback"
    assert "hybrid" in report["reason"]


def test_health_reports_fallback_when_no_collection_configured(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    cfg.search.collection = None
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: True)
    report = search_health(cfg)
    assert report["mode"] == "fallback"
    assert report["collection"] is None
    assert "collection" in report["reason"]


def test_health_reports_fallback_when_daemon_down(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: False)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: True)
    report = search_health(cfg)
    assert report["mode"] == "fallback"
    assert report["daemon_up"] is False
    assert "daemon" in report["reason"]


def test_health_reports_fallback_when_indexed_binary_missing(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The gap this closes: indexed absent must degrade cleanly, never raise, and
    # be distinguishable in the report from a merely-down daemon.
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: False)
    report = search_health(cfg)
    assert report["mode"] == "fallback"
    assert report["indexed_binary_available"] is False
    assert "indexed" in report["reason"]


def test_health_never_raises_when_indexed_entirely_absent(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    # No mocking of indexed_available at all — exercise the real shutil.which
    # probe against a binary name that (almost certainly) isn't on PATH here,
    # and confirm the daemon-down default also degrades without raising.
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: False)
    report = search_health(cfg)
    assert report["mode"] == "fallback"
    assert isinstance(report["indexed_binary_available"], bool)


# --------------------------------------------------------------------------- #
# CLI: shards search --health                                                 #
# --------------------------------------------------------------------------- #


def test_cli_search_health_reports_indexed_reachable(
    shards_config: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: True)
    result = _invoke(["search", "--health"])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert payload["mode"] == "indexed"


def test_cli_search_health_reports_fallback_distinctly(
    shards_config: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: False)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: False)
    result = _invoke(["search", "--health"])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert payload["mode"] == "fallback"
    assert payload["daemon_up"] is False
    assert payload["indexed_binary_available"] is False


def test_cli_search_health_ignores_query_and_never_performs_a_search(
    shards_config: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def _boom(*_a: object, **_k: object) -> None:
        raise AssertionError("--health must not run a query")

    # Patched at the ``cli.search`` import site (where the call is actually
    # made), not ``core.search`` — a ``from ... import`` binds a separate name.
    monkeypatch.setattr("shards.cli.search.query_search", _boom)
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: True)
    result = _invoke(["search", "some query", "--health"])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert payload["mode"] == "indexed"
