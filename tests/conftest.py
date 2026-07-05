"""Shared pytest fixtures for the shards test suite.

The keystone fixtures land with notes/1: a sandboxed temp vault plus a config
pointed at it via ``SHARDS_CONFIG_PATH``. Every later feature inherits these, so
they stay deliberately dependency-free (pure filesystem + env) and rely on
pytest's ``tmp_path`` / ``monkeypatch`` for automatic per-test cleanup.
"""

from __future__ import annotations

from pathlib import Path

import pytest

# Vault skeleton created by the ``vault`` fixture. ``notes/`` and
# ``notes/.locks/`` are required by the notes/1 acceptance criteria; the rest are
# cheap and let later units write without re-creating folders.
_VAULT_SUBDIRS = (
    "notes",
    "notes/logs",
    "notes/decisions",
    "notes/references",
    "notes/.locks",
    "tasks/open",
    "tasks/done",
)


@pytest.fixture
def vault(tmp_path: Path) -> Path:
    """A tmp_path-backed Tolaria vault with ``notes/`` and ``notes/.locks/``.

    tmp_path is torn down by pytest after each test, so no explicit cleanup is
    needed.
    """
    root = tmp_path / "vault"
    for sub in _VAULT_SUBDIRS:
        (root / sub).mkdir(parents=True, exist_ok=True)
    return root


@pytest.fixture
def config_path(tmp_path: Path) -> Path:
    """Filesystem location the ``config.toml`` will be written to."""
    return tmp_path / "config.toml"


@pytest.fixture
def shards_config(
    vault: Path,
    config_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> Path:
    """Write a minimal config pointing at ``vault`` and export SHARDS_CONFIG_PATH.

    ``monkeypatch.setenv`` is undone automatically after the test, isolating the
    suite from any real ``~/.shards/config.toml``.
    """
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
                "threshold = 0.65",
                "",
                "[tasks]",
                'collections = ["test-agent", "other-agent"]',
                "",
            )
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(config_path))
    # Ensure a stray real agent identity never leaks into config tests.
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    return config_path
