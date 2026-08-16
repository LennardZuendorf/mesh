"""``search --health`` — surfacing indexed reachability vs. substring fallback.

Silent degradation (root ``tech.md`` § Risks — "``indexed`` drift") means a
caller can't tell whether a result set came from the hybrid ``indexed`` engine
or the substring fallback without reading stderr notices closely. ``--health``
makes the four gates that decide the recall path (``[search].hybrid``,
``[search].collection``, daemon liveness, ``indexed`` binary presence)
explicit in one JSON payload, both via :func:`shards.core.search.search_health`
directly and through the ``shards search --health`` CLI flag — and it must
degrade cleanly (never raise) even when ``indexed`` is entirely absent.

agent-usability/4 adds the MCP mirror, ``shards_health`` (``shards/mcp/server.py``)
— the CLI-only flag above had no MCP surface at all before this unit, so an
MCP agent had no way to check recall-path reachability short of a degraded
search and a stderr line it never sees. The tests below drive the *registered*
tool (``server.app.call_tool``), across the same four gates the CLI-level
tests above exercise, and assert byte-for-byte payload parity against the real
``shards search --health`` CLI invocation over the same mocked gate state —
so the two surfaces cannot silently diverge.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

import pytest
from typer.testing import CliRunner

import shards.mcp.server as server
from shards.cli.__main__ import app
from shards.core.search import search_health
from shards.index import indexed_client
from shards.schemas.config import Config, load_config


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _write_config(
    *,
    vault: Path,
    config_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    hybrid: bool,
    collection: str | None,
) -> None:
    """Write a real ``config.toml`` and export ``SHARDS_CONFIG_PATH`` — needed
    (rather than mutating an already-loaded ``Config``) because both the CLI
    and the MCP tool call ``load_config()`` fresh from disk on every
    invocation; an in-memory mutation of a separately loaded object is never
    seen by either surface."""
    collection_line = f'collection = "{collection}"' if collection is not None else ""
    config_path.write_text(
        "\n".join(
            (
                "[core]",
                f'tolaria_path = "{vault}"',
                'agent = "test-agent"',
                "",
                "[search]",
                collection_line,
                f"hybrid = {str(hybrid).lower()}",
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


# --------------------------------------------------------------------------- #
# MCP: shards_health (agent-usability/4)                                      #
# --------------------------------------------------------------------------- #


def _mcp_health() -> dict[str, Any]:
    """Dispatch the real *registered* ``shards_health`` tool (not the bare
    module function), mirroring how an MCP client actually calls it."""
    dispatched = asyncio.run(server.app.call_tool("shards_health", {}))
    result: dict[str, Any] = dispatched.structured_content
    return result


@pytest.mark.parametrize(
    ("gate_mocks", "expected_mode", "reason_substring"),
    [
        pytest.param(
            {"daemon_up": True, "indexed_available": True}, "indexed", None, id="every-gate-open"
        ),
        pytest.param(
            {"daemon_up": False, "indexed_available": True}, "fallback", "daemon", id="daemon-down"
        ),
        pytest.param(
            {"daemon_up": True, "indexed_available": False},
            "fallback",
            "indexed",
            id="binary-absent",
        ),
    ],
)
def test_mcp_health_mirrors_core_search_health_gates(
    cfg: Config,
    monkeypatch: pytest.MonkeyPatch,
    gate_mocks: dict[str, bool],
    expected_mode: str,
    reason_substring: str | None,
) -> None:
    """``shards_health`` reports the same ``mode``/gates ``search_health``
    does for daemon-liveness and indexed-binary gate combinations — mirroring
    the CLI-level table above, now through the registered MCP tool."""
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: gate_mocks["daemon_up"])
    monkeypatch.setattr(
        indexed_client, "indexed_available", lambda: gate_mocks["indexed_available"]
    )

    payload = _mcp_health()

    assert payload["mode"] == expected_mode
    assert payload["daemon_up"] is gate_mocks["daemon_up"]
    assert payload["indexed_binary_available"] is gate_mocks["indexed_available"]
    if reason_substring is not None:
        assert reason_substring in payload["reason"]
    else:
        assert "reason" not in payload


def test_mcp_health_reports_fallback_when_hybrid_disabled(
    vault: Path, config_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The two gates the daemon/binary table above doesn't cover — hybrid
    disabled and no collection configured — mirroring the core-level tests."""
    _write_config(
        vault=vault,
        config_path=config_path,
        monkeypatch=monkeypatch,
        hybrid=False,
        collection="test-vault",
    )
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: True)

    payload = _mcp_health()

    assert payload["mode"] == "fallback"
    assert "hybrid" in payload["reason"]


def test_mcp_health_reports_fallback_when_no_collection_configured(
    vault: Path, config_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_config(
        vault=vault, config_path=config_path, monkeypatch=monkeypatch, hybrid=True, collection=None
    )
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    monkeypatch.setattr(indexed_client, "indexed_available", lambda: True)

    payload = _mcp_health()

    assert payload["mode"] == "fallback"
    assert payload["collection"] is None
    assert "collection" in payload["reason"]


@pytest.mark.parametrize(
    "gate_mocks",
    [
        pytest.param({"daemon_up": True, "indexed_available": True}, id="indexed-mode"),
        pytest.param({"daemon_up": False, "indexed_available": False}, id="fallback-mode"),
    ],
)
def test_mcp_health_matches_cli_health_byte_for_byte(
    shards_config: Path, monkeypatch: pytest.MonkeyPatch, gate_mocks: dict[str, bool]
) -> None:
    """The load-bearing anti-drift assertion: over the *identical* mocked gate
    state, the registered MCP tool and the real ``shards search --health`` CLI
    invocation return the exact same JSON payload — both surfaces are reading
    off the one ``core.search.search_health`` implementation, so neither can
    silently diverge from the other."""
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: gate_mocks["daemon_up"])
    monkeypatch.setattr(
        indexed_client, "indexed_available", lambda: gate_mocks["indexed_available"]
    )

    cli_result = _invoke(["search", "--health"])
    assert cli_result.exit_code == 0, cli_result.output
    cli_payload = json.loads(cli_result.stdout)

    mcp_payload = _mcp_health()

    assert mcp_payload == cli_payload


def test_mcp_health_never_raises_when_indexed_entirely_absent(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    # No mocking of indexed_available at all — the real shutil.which probe,
    # same as the core-level equivalent above.
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: False)
    payload = _mcp_health()
    assert payload["mode"] == "fallback"
    assert isinstance(payload["indexed_binary_available"], bool)


def test_mcp_health_is_a_pure_delegate_not_a_reimplementation(
    cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Structural anti-drift proof: patch ``core.search.search_health`` itself
    (the name ``server.py`` imports) and confirm the registered tool returns
    exactly what it returned — ``shards_health`` has no gate logic of its own
    to drift from the CLI's."""
    sentinel = {
        "mode": "indexed",
        "hybrid_configured": True,
        "collection": "c",
        "daemon_up": True,
        "indexed_binary_available": True,
    }
    monkeypatch.setattr(server, "search_health", lambda config: sentinel)

    assert _mcp_health() == sentinel
