"""The daemon is bound to one vault — and says so on every reply.

Two layers, both proven here:

* **The socket is vault-scoped.** ``default_socket_path`` keys the socket file on
  a digest of the *resolved vault root*, so two vaults on one machine can never
  meet on one socket. A process with no resolvable config keeps the legacy
  ``shards.sock`` name rather than failing.
* **Every reply names the vault it came from.** A config-ful server stamps
  ``vault`` into its envelope and the client treats a mismatch exactly like a
  daemon-down transport failure: run the file-op fallback, never raise, never
  hand back another vault's rows. This is the belt for the socket's braces — it
  covers the stale daemon still running after a config edit, which is the one
  case a per-vault socket name cannot catch.

Invariant 1 governs both: the daemon accelerates, it never gates. A vault
mismatch therefore *degrades* a read to disk; it never turns into an error.
"""

from __future__ import annotations

import json
import socket
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest

from shards.daemon.client import DaemonClient, default_socket_path, vault_id
from shards.schemas.config import Config, load_config
from tests.daemon.conftest import running_daemon

_VAULT_SUBDIRS = (
    "notes",
    "notes/logs",
    "notes/decisions",
    "notes/references",
    "notes/.locks",
    "tasks/open",
    "tasks/done",
)


# --------------------------------------------------------------------------- #
# Fixtures & helpers                                                          #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg_b(shards_config: Path) -> Config:
    """The *ambient* config (``$SHARDS_CONFIG_PATH``) — vault B, the caller's."""
    return load_config()


def _make_vault(root: Path) -> Path:
    for sub in _VAULT_SUBDIRS:
        (root / sub).mkdir(parents=True, exist_ok=True)
    return root


def _write_config(path: Path, vault: Path) -> Config:
    path.write_text(
        "\n".join(("[core]", f'vault_path = "{vault}"', 'agent = "test-agent"', "")),
        encoding="utf-8",
    )
    return load_config(path)


def _seed_note(vault: Path, note_id: str, title: str = "Seeded") -> Path:
    when = datetime.now(UTC)
    post = frontmatter.Post("Body line.")
    post.metadata = {
        "id": note_id,
        "type": "note",
        "title": title,
        "tags": [],
        "owner": "seed-agent",
        "created": when,
        "updated": when,
        "related": [],
    }
    path = vault / "notes" / f"{note_id}.md"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


@pytest.fixture
def vault_a(tmp_path: Path) -> Path:
    """A second vault, foreign to the ambient config's vault."""
    return _make_vault(tmp_path / "vault-a")


@pytest.fixture
def cfg_a(tmp_path: Path, vault_a: Path) -> Config:
    return _write_config(tmp_path / "config-a.toml", vault_a)


def _roundtrip(path: Path, request: dict[str, object]) -> dict[str, object]:
    """Send one NDJSON request over a raw blocking socket, return the reply."""
    payload = (json.dumps(request) + "\n").encode("utf-8")
    buf = bytearray()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(5)
        sock.connect(str(path))
        sock.sendall(payload)
        while b"\n" not in buf:
            chunk = sock.recv(4096)
            if not chunk:
                break
            buf.extend(chunk)
    return json.loads(bytes(buf).split(b"\n", 1)[0])  # type: ignore[no-any-return]


# --------------------------------------------------------------------------- #
# Layer 1 — the socket file is keyed on the vault                             #
# --------------------------------------------------------------------------- #


def test_two_vaults_never_share_a_socket(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, cfg_a: Config, cfg_b: Config
) -> None:
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    assert default_socket_path(cfg_a) != default_socket_path(cfg_b)


def test_socket_name_is_stable_for_one_vault(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, vault_a: Path
) -> None:
    """Same vault, two independently loaded configs → the identical socket."""
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    first = _write_config(tmp_path / "one.toml", vault_a)
    second = _write_config(tmp_path / "two.toml", vault_a)
    assert default_socket_path(first) == default_socket_path(second)
    assert default_socket_path(first).parent == tmp_path


def test_socket_name_follows_the_ambient_config(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, cfg_b: Config
) -> None:
    """The zero-arg call resolves ``$SHARDS_CONFIG_PATH`` — same answer, no arg."""
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    assert default_socket_path() == default_socket_path(cfg_b)


def test_socket_name_degrades_when_no_config_is_resolvable(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """No config → the legacy name, never an exception out of a path helper."""
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(tmp_path / "absent.toml"))
    assert default_socket_path() == tmp_path / "shards.sock"


def test_socket_name_is_a_digest_of_the_resolved_vault(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, cfg_a: Config, vault_a: Path
) -> None:
    """A symlinked vault resolves to the same identity as its real path."""
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    link = tmp_path / "link-to-a"
    link.symlink_to(vault_a)
    via_link = _write_config(tmp_path / "link.toml", link)
    assert vault_id(via_link) == vault_id(cfg_a)
    assert default_socket_path(via_link) == default_socket_path(cfg_a)


# --------------------------------------------------------------------------- #
# Layer 2 — the reply names its vault; a mismatch degrades to disk            #
# --------------------------------------------------------------------------- #


def test_config_ful_reply_carries_the_vault_root(socket_path: Path, cfg_a: Config) -> None:
    with running_daemon(socket_path, config=cfg_a):
        reply = _roundtrip(socket_path, {"id": "v", "method": "ping", "params": {}})
    assert reply["vault"] == vault_id(cfg_a)


def test_config_less_reply_is_unchanged(socket_path: Path) -> None:
    """A config-less server serves no vault, so it stamps none — the envelope is
    byte-identical to the documented contract."""
    with running_daemon(socket_path):
        reply = _roundtrip(socket_path, {"id": "v", "method": "ping", "params": {}})
    assert reply == {"id": "v", "ok": True, "result": {"pong": True}}


def test_note_list_never_serves_a_foreign_vaults_rows(
    socket_path: Path, cfg_a: Config, cfg_b: Config, vault_a: Path, vault: Path
) -> None:
    """A live daemon on vault A must not answer a vault-B caller's ``note list``."""
    _seed_note(vault_a, "n-alpha", title="Vault A note")
    _seed_note(vault, "n-beta", title="Vault B note")
    with running_daemon(socket_path, config=cfg_a):
        client = DaemonClient(socket_path=socket_path)
        views = client.note_list(cfg_b)
    assert [v.note.id for v in views] == ["n-beta"]
    assert all(str(v.path).startswith(str(vault)) for v in views)


def test_task_list_never_serves_a_foreign_vaults_rows(
    socket_path: Path, cfg_a: Config, cfg_b: Config, vault_a: Path, vault: Path
) -> None:
    _seed_note(vault_a, "n-alpha")
    with running_daemon(socket_path, config=cfg_a):
        client = DaemonClient(socket_path=socket_path)
        views = client.task_list(cfg_b)
    assert views == []


def test_vault_status_never_counts_a_foreign_vault(
    socket_path: Path, cfg_a: Config, cfg_b: Config, vault_a: Path, vault: Path
) -> None:
    _seed_note(vault_a, "n-alpha")
    _seed_note(vault_a, "n-gamma")
    with running_daemon(socket_path, config=cfg_a):
        report = DaemonClient(socket_path=socket_path).vault_status(cfg_b)
    assert report["notes"] == 0  # vault B is empty; vault A holds two notes


def test_activity_recent_never_leaks_foreign_paths(
    socket_path: Path, cfg_a: Config, cfg_b: Config, vault_a: Path, vault: Path
) -> None:
    """``activity.recent`` has an empty fallback-code set — a vault mismatch must
    still degrade to the on-disk scan rather than leak A's absolute paths."""
    _seed_note(vault_a, "n-alpha")
    _seed_note(vault, "n-beta")
    with running_daemon(socket_path, config=cfg_a):
        result = DaemonClient(socket_path=socket_path).activity_recent(cfg_b)
    entries = result["entries"]
    assert [e["id"] for e in entries] == ["n-beta"]
    assert all(not str(e["path"]).startswith(str(vault_a)) for e in entries)


def test_tag_pull_never_serves_a_foreign_vaults_hits(
    socket_path: Path, cfg_a: Config, cfg_b: Config, vault_a: Path, vault: Path
) -> None:
    _seed_note(vault_a, "n-alpha")
    with running_daemon(socket_path, config=cfg_a):
        hits = DaemonClient(socket_path=socket_path).tag_pull(cfg_b, limit=10)
    assert [h.id for h in hits] == []


def test_a_foreign_vault_daemon_does_not_count_as_up(
    socket_path: Path, cfg_a: Config, cfg_b: Config
) -> None:
    """The liveness gate is per-vault: a daemon serving A is *down* for B."""
    with running_daemon(socket_path, config=cfg_a):
        assert DaemonClient(socket_path=socket_path, config=cfg_b).is_up() is False
        assert DaemonClient(socket_path=socket_path, config=cfg_a).is_up() is True


def test_matching_vault_still_serves_from_the_warm_index(
    socket_path: Path, cfg_b: Config, vault: Path
) -> None:
    """The guard must not cost the happy path: same vault → still warm-served."""
    _seed_note(vault, "n-beta")
    with running_daemon(socket_path, config=cfg_b):
        client = DaemonClient(socket_path=socket_path, config=cfg_b)
        assert client.is_up() is True
        assert [v.note.id for v in client.note_list(cfg_b)] == ["n-beta"]
