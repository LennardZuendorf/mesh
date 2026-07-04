"""search/3 — Freshness: keeping the ``indexed`` collection current.

This unit wires the ``indexed`` client's re-index calls to the vault's lifecycle:

* :func:`~brain.index.indexed_client.incremental_update` shells ``indexed index
  update <path> --collection <c>`` for a single changed file, and — crucially —
  **swallows every failure** so a missing binary or a non-zero exit can never
  crash the watchdog observer thread that calls it.
* :func:`~brain.index.indexed_client.full_rebuild` shells ``indexed index create
  <vault> --collection <c>``; :func:`~brain.index.indexed_client.reindex` is its
  alias and the delegate ``brain reindex`` calls.
* :class:`~brain.daemon.server.DaemonServer`, when started with a vault config,
  registers ``incremental_update`` on the module-level watcher change-hook so
  *every* create/modify/move/delete re-indexes just that file. Registration is
  explicit and config-bound — never a module-import side-effect.

The real ``indexed`` binary is **never** shelled here: every subprocess is faked
at ``indexed_client.subprocess.run`` (or the ``incremental_update`` name in the
daemon namespace). The change-hook registry is module-level, so an autouse
fixture clears it before and after each test to prevent leakage.
"""

from __future__ import annotations

import asyncio
import subprocess
import tempfile
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from typer.testing import CliRunner

from brain.cli.__main__ import app
from brain.daemon import server as server_mod
from brain.index import indexed_client
from brain.index.watch import clear_change_hooks, on_vault_change
from brain.schemas.config import Config, load_config

# --------------------------------------------------------------------------- #
# Fixtures                                                                     #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg(brain_config: Path) -> Config:
    return load_config()


@pytest.fixture(autouse=True)
def _clean_hooks() -> Iterator[None]:
    """Isolate every test from the module-level change-hook registry (no leakage)."""
    clear_change_hooks()
    try:
        yield
    finally:
        clear_change_hooks()


@pytest.fixture
def sock_dir() -> Iterator[Path]:
    """A short-lived ``/tmp`` dir for unix sockets (AF_UNIX path-length limit)."""
    path = Path(tempfile.mkdtemp(prefix="brn-fresh-", dir="/tmp"))
    try:
        yield path
    finally:
        for child in path.glob("*"):
            child.unlink(missing_ok=True)
        path.rmdir()


@pytest.fixture
def socket_path(sock_dir: Path) -> Path:
    return sock_dir / "d.sock"


def _fake_run_recorder(recorded: list[list[str]]) -> Any:
    """A ``subprocess.run`` stand-in that records argv and reports success."""

    def fake_run(args: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        recorded.append(args)
        return subprocess.CompletedProcess(args, 0, stdout="", stderr="")

    return fake_run


# --------------------------------------------------------------------------- #
# incremental_update — argv, swallowing, no-op                                 #
# --------------------------------------------------------------------------- #


def test_incremental_update_shells_index_update(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    recorded: list[list[str]] = []
    monkeypatch.setattr(indexed_client.subprocess, "run", _fake_run_recorder(recorded))
    target = vault / "notes" / "n-fresh.md"
    indexed_client.incremental_update(cfg, target)
    assert recorded == [["indexed", "index", "update", str(target), "--collection", "test-vault"]]


def test_incremental_update_swallows_missing_binary(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A missing ``indexed`` binary must NEVER crash the watcher thread.
    def _raise(*_args: object, **_kwargs: object) -> subprocess.CompletedProcess[str]:
        raise FileNotFoundError("indexed")

    monkeypatch.setattr(indexed_client.subprocess, "run", _raise)
    # Returns cleanly (no exception) — the swallow is the whole point of this unit.
    indexed_client.incremental_update(cfg, vault / "notes" / "n-x.md")


def test_incremental_update_swallows_nonzero_exit(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def _raise(*_args: object, **_kwargs: object) -> subprocess.CompletedProcess[str]:
        raise subprocess.CalledProcessError(1, ["indexed"])

    monkeypatch.setattr(indexed_client.subprocess, "run", _raise)
    indexed_client.incremental_update(cfg, vault / "notes" / "n-x.md")


def test_incremental_update_swallows_generic_oserror(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def _raise(*_args: object, **_kwargs: object) -> subprocess.CompletedProcess[str]:
        raise OSError("permission denied")

    monkeypatch.setattr(indexed_client.subprocess, "run", _raise)
    indexed_client.incremental_update(cfg, vault / "notes" / "n-x.md")


def test_incremental_update_noop_without_collection(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cfg.search.collection = None

    def _boom(*_args: object, **_kwargs: object) -> subprocess.CompletedProcess[str]:
        raise AssertionError("no subprocess when collection is None")

    monkeypatch.setattr(indexed_client.subprocess, "run", _boom)
    indexed_client.incremental_update(cfg, vault / "notes" / "n-x.md")


# --------------------------------------------------------------------------- #
# full_rebuild / reindex — argv, delegation, no-op                             #
# --------------------------------------------------------------------------- #


def test_full_rebuild_shells_index_create(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    recorded: list[list[str]] = []
    monkeypatch.setattr(indexed_client.subprocess, "run", _fake_run_recorder(recorded))
    indexed_client.full_rebuild(cfg)
    assert recorded == [["indexed", "index", "create", str(vault), "--collection", "test-vault"]]


def test_reindex_delegates_to_full_rebuild(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[Config] = []
    monkeypatch.setattr(indexed_client, "full_rebuild", lambda config: calls.append(config))
    indexed_client.reindex(cfg)
    assert calls == [cfg]


def test_full_rebuild_noop_without_collection(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    cfg.search.collection = None

    def _boom(*_args: object, **_kwargs: object) -> subprocess.CompletedProcess[str]:
        raise AssertionError("no subprocess when collection is None")

    monkeypatch.setattr(indexed_client.subprocess, "run", _boom)
    indexed_client.full_rebuild(cfg)  # silent no-op


# --------------------------------------------------------------------------- #
# No module-import side-effect: registration is explicit + config-bound        #
# --------------------------------------------------------------------------- #


def test_no_hook_registered_at_import(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Importing ``indexed_client`` must not have registered any change hook.

    The registry is cleared by the autouse fixture; firing a vault change now must
    reach ``incremental_update`` zero times, proving registration is an explicit
    step (not an import-time side-effect).
    """
    calls: list[Path] = []
    monkeypatch.setattr(indexed_client, "incremental_update", lambda c, p: calls.append(p))
    on_vault_change(vault / "notes" / "n-x.md")
    assert calls == []


def test_register_hook_is_explicit_and_config_bound(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """After an explicit ``register_hook``, a vault change drives incremental_update."""
    calls: list[tuple[Config, Path]] = []
    monkeypatch.setattr(
        indexed_client, "incremental_update", lambda config, path: calls.append((config, path))
    )
    indexed_client.register_hook(cfg)
    changed = vault / "notes" / "n-x.md"
    on_vault_change(changed)
    assert calls == [(cfg, changed)]


# --------------------------------------------------------------------------- #
# Daemon wiring: start() registers incremental_update on the watcher hook       #
# --------------------------------------------------------------------------- #


def _run(loop: asyncio.AbstractEventLoop, coro: Any) -> Any:
    return loop.run_until_complete(coro)


def test_daemon_start_registers_incremental_update_hook(
    cfg: Config, vault: Path, socket_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A config-ful ``DaemonServer.start`` wires ``incremental_update`` to the hook.

    The lambda resolves ``incremental_update`` in the ``brain.daemon.server``
    namespace, so patch it there (``raising=False`` keeps the RED clean before the
    wiring exists). Firing ``on_vault_change`` then reaches the recorder with the
    startup config, proving every watcher event re-indexes that path.
    """
    calls: list[tuple[Config, Path]] = []
    monkeypatch.setattr(
        server_mod,
        "incremental_update",
        lambda config, path: calls.append((config, path)),
        raising=False,
    )
    server = server_mod.DaemonServer(socket_path, config=cfg)
    loop = asyncio.new_event_loop()
    try:
        _run(loop, server.start())
        changed = vault / "notes" / "n-daemon.md"
        on_vault_change(changed)  # what the watchdog thread calls after each event
        assert (cfg, changed) in calls
    finally:
        _run(loop, server.stop())
        loop.close()


def test_daemon_without_config_registers_no_hook(
    cfg: Config, vault: Path, socket_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A config-less daemon (transport-only) must not register the freshness hook."""
    calls: list[Path] = []
    monkeypatch.setattr(
        server_mod,
        "incremental_update",
        lambda config, path: calls.append(path),
        raising=False,
    )
    server = server_mod.DaemonServer(socket_path, config=None)
    loop = asyncio.new_event_loop()
    try:
        _run(loop, server.start())
        on_vault_change(vault / "notes" / "n-x.md")
        assert calls == []
    finally:
        _run(loop, server.stop())
        loop.close()


# --------------------------------------------------------------------------- #
# brain reindex — delegates to indexed_client.reindex == full_rebuild          #
# --------------------------------------------------------------------------- #


def test_brain_reindex_cli_calls_full_rebuild(
    brain_config: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """``brain reindex`` reaches ``indexed_client.reindex`` → ``full_rebuild``."""
    calls: list[Config] = []
    monkeypatch.setattr(indexed_client, "full_rebuild", lambda config: calls.append(config))
    result = CliRunner().invoke(app, ["reindex"])
    assert result.exit_code == 0, result.output
    assert len(calls) == 1
    assert isinstance(calls[0], Config)
