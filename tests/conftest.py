"""Shared pytest fixtures for the mesh test suite.

The keystone fixtures land with notes/1: a sandboxed temp vault plus a config
pointed at it via ``MESH_CONFIG_PATH``. Every later feature inherits these, so
they stay deliberately dependency-free (pure filesystem + env) and rely on
pytest's ``tmp_path`` / ``monkeypatch`` for automatic per-test cleanup.
"""

from __future__ import annotations

import shutil
import tempfile
from collections.abc import Iterator
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


@pytest.fixture(autouse=True)
def isolate_daemon_socket(monkeypatch: pytest.MonkeyPatch) -> Iterator[None]:
    """Point the default daemon socket at a private dir for every test.

    ``DaemonClient()`` with no explicit socket resolves ``$XDG_RUNTIME_DIR`` (else
    ``$HOME/.mesh/run/``). Without this fixture any daemon listening on the real
    user socket answers the suite's reads — a developer who ran the documented
    ``mesh daemon start``, or a CI runner with a leftover daemon, gets a cascade
    of failures whose rows come from *another vault entirely*. Worse, the tests
    most affected are the ones asserting the daemon-down fallback: they would be
    cold only by accident of the environment. Autouse so the guarantee is the
    suite's default rather than each module's responsibility to remember.

    Deliberately *not* under ``tmp_path``: tests that assert on the exact contents
    of their vault directory must not see a runtime dir appear inside it. The
    short ``/tmp`` prefix also keeps the socket path clear of the 108-byte
    ``AF_UNIX`` limit.
    """
    run_dir = tempfile.mkdtemp(prefix="mesh-rt-")
    monkeypatch.setenv("XDG_RUNTIME_DIR", run_dir)
    try:
        yield
    finally:
        shutil.rmtree(run_dir, ignore_errors=True)


@pytest.fixture
def vault(tmp_path: Path) -> Path:
    """A tmp_path-backed vault with ``notes/`` and ``notes/.locks/``.

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
def mesh_config(
    vault: Path,
    config_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> Path:
    """Write a minimal config pointing at ``vault`` and export MESH_CONFIG_PATH.

    ``monkeypatch.setenv`` is undone automatically after the test, isolating the
    suite from any real ``~/.mesh/config.toml``.
    """
    config_path.write_text(
        "\n".join(
            (
                "[core]",
                f'vault_path = "{vault}"',
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
    monkeypatch.setenv("MESH_CONFIG_PATH", str(config_path))
    # Ensure a stray real agent identity never leaks into config tests.
    monkeypatch.delenv("MESH_AGENT", raising=False)
    return config_path
