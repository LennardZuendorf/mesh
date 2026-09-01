"""agent-usability/7 — `mesh init`, example config, README.

Brief: `.superpowers/sdd/mesh-3track/agent-usability-7-brief.md`. Locks the
onboarding contract (`.spec/features/agent-usability/tech.md` § Onboarding):

* `init` writes a config `load_config` accepts, with every `Config` field
  populated or defaulted, honouring `$MESH_CONFIG_PATH`, and prints the
  path it wrote.
* Idempotent / non-destructive: re-running without `--force` leaves the file
  byte-identical and exits non-zero with a message naming `--force`;
  `--force` rewrites.
* The load-bearing scenario: after `init` with no prior config at all,
  `note new` / `task list` actually run and succeed — not just "a file now
  exists".
* `config.example.toml` parses through `load_config` (so it cannot rot).
* `init` never reaches the MCP tool table — root AGENTS.md keeps the
  three-verb thesis; `init` is admin, beside `daemon`/`status`/`reindex`.
* `load_config`'s missing-config message names the resolved path and the one
  required key, still at exit 2 — agent-usability/5's tool-error wording
  depends on `mesh init` being nameable here.

None of these tests write outside ``tmp_path``: the ``cfg_path`` fixture
always exports ``MESH_CONFIG_PATH``, and every ``init`` invocation is given
an explicit ``--path`` (or monkeypatches ``admin._DEFAULT_VAULT_PATH``) so the
true ``~/.mesh/vault`` default is exercised without ever touching the real
host home directory.
"""

from __future__ import annotations

import asyncio
import json
import tomllib
from pathlib import Path

import pytest
from typer.testing import CliRunner

import mesh.cli.admin as admin
import mesh.mcp.server as mcp_server
from mesh.cli.__main__ import app
from mesh.schemas.config import load_config


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


@pytest.fixture
def cfg_path(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Point $MESH_CONFIG_PATH at a not-yet-existing file under tmp_path."""
    path = tmp_path / "config.toml"
    monkeypatch.setenv("MESH_CONFIG_PATH", str(path))
    monkeypatch.delenv("MESH_AGENT", raising=False)
    return path


# --------------------------------------------------------------------------- #
# Writes a loadable config                                                    #
# --------------------------------------------------------------------------- #


def test_init_writes_config_load_config_accepts(tmp_path: Path, cfg_path: Path) -> None:
    vault = tmp_path / "vault"
    result = _invoke(["init", "--path", str(vault), "--agent", "test-agent"])
    assert result.exit_code == 0, result.output
    assert cfg_path.is_file()

    cfg = load_config(cfg_path)  # must not raise
    assert cfg.core.vault_path == vault
    assert cfg.core.agent == "test-agent"
    assert cfg.search.hybrid is True
    assert cfg.search.threshold == pytest.approx(0.65)
    assert cfg.tasks.collections == []


def test_init_prints_the_path_it_wrote(tmp_path: Path, cfg_path: Path) -> None:
    result = _invoke(["init", "--path", str(tmp_path / "vault")])
    assert result.exit_code == 0
    assert str(cfg_path) in result.output


def test_init_json_output_reports_path_and_agent(tmp_path: Path, cfg_path: Path) -> None:
    # init is admin (out of the R6 either-side flag contract, root AGENTS.md §6)
    # — --json is global-only here, like the rest of daemon/status/reindex.
    result = _invoke(["--json", "init", "--path", str(tmp_path / "vault"), "--agent", "a"])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.output)
    assert payload["path"] == str(cfg_path)
    assert payload["agent"] == "a"


def test_init_no_flags_still_populates_every_field(
    tmp_path: Path, cfg_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Real defaults, not real $HOME: patch the module constant, not env HOME."""
    monkeypatch.setattr(admin, "_DEFAULT_VAULT_PATH", tmp_path / "default-vault")
    result = _invoke(["init"])
    assert result.exit_code == 0, result.output

    cfg = load_config(cfg_path)
    assert cfg.core.vault_path == tmp_path / "default-vault"
    assert cfg.core.agent  # populated, not None/empty
    assert cfg.search.hybrid is True
    assert cfg.search.threshold == pytest.approx(0.65)
    assert cfg.tasks.collections == []


def test_init_honours_mesh_agent_env_as_default(
    tmp_path: Path, cfg_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MESH_AGENT", "env-agent")
    result = _invoke(["init", "--path", str(tmp_path / "vault")])
    assert result.exit_code == 0, result.output
    cfg = load_config(cfg_path)
    assert cfg.core.agent == "env-agent"


def test_init_honours_collections_roster(tmp_path: Path, cfg_path: Path) -> None:
    result = _invoke(
        ["init", "--path", str(tmp_path / "vault"), "--collections", "agent-a, agent-b"]
    )
    assert result.exit_code == 0, result.output
    cfg = load_config(cfg_path)
    assert cfg.tasks.collections == ["agent-a", "agent-b"]


def test_init_creates_the_vault_directory(tmp_path: Path, cfg_path: Path) -> None:
    vault = tmp_path / "fresh-vault"
    assert not vault.exists()
    result = _invoke(["init", "--path", str(vault)])
    assert result.exit_code == 0, result.output
    assert vault.is_dir()


def test_init_expands_tilde_in_path(
    tmp_path: Path, cfg_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A literal `~/vault` must resolve under the (faked) home, never become a
    # relative `./~/vault` — the same defect .spec/lessons.md already records
    # for CoreConfig itself; `init` must not reintroduce it on the write side.
    monkeypatch.setenv("HOME", str(tmp_path))
    result = _invoke(["init", "--path", "~/myvault"])
    assert result.exit_code == 0, result.output
    cfg = load_config(cfg_path)
    assert cfg.core.vault_path == tmp_path / "myvault"
    assert cfg.core.vault_path.is_dir()
    assert not (Path.cwd() / "~").exists()


# --------------------------------------------------------------------------- #
# Idempotent / non-destructive                                                #
# --------------------------------------------------------------------------- #


def test_rerun_without_force_leaves_file_byte_identical_and_exits_nonzero(
    tmp_path: Path, cfg_path: Path
) -> None:
    vault = tmp_path / "vault"
    first = _invoke(["init", "--path", str(vault), "--agent", "agent-a"])
    assert first.exit_code == 0, first.output
    original_bytes = cfg_path.read_bytes()

    second = _invoke(["init", "--path", str(vault), "--agent", "agent-b"])
    assert second.exit_code != 0
    assert "force" in second.output.lower()
    assert str(cfg_path) in second.output

    assert cfg_path.read_bytes() == original_bytes
    assert load_config(cfg_path).core.agent == "agent-a"  # untouched, not agent-b


def test_force_rewrites(tmp_path: Path, cfg_path: Path) -> None:
    vault = tmp_path / "vault"
    first = _invoke(["init", "--path", str(vault), "--agent", "agent-a"])
    assert first.exit_code == 0, first.output

    second = _invoke(["init", "--path", str(vault), "--agent", "agent-b", "--force"])
    assert second.exit_code == 0, second.output
    assert load_config(cfg_path).core.agent == "agent-b"


def test_init_twice_with_force_each_time_is_safe(tmp_path: Path, cfg_path: Path) -> None:
    vault = tmp_path / "vault"
    for _ in range(2):
        result = _invoke(["init", "--path", str(vault), "--agent", "agent-a", "--force"])
        assert result.exit_code == 0, result.output
    cfg = load_config(cfg_path)
    assert cfg.core.agent == "agent-a"


# --------------------------------------------------------------------------- #
# The load-bearing scenario: a genuine working install, not just files        #
# --------------------------------------------------------------------------- #


def test_first_run_end_to_end_note_and_task_actually_work(tmp_path: Path, cfg_path: Path) -> None:
    """No prior config at all -> init -> note new / task list actually run."""
    assert not cfg_path.exists()
    vault = tmp_path / "vault"

    init_result = _invoke(["init", "--path", str(vault), "--agent", "e2e-agent"])
    assert init_result.exit_code == 0, init_result.output

    note_result = _invoke(["note", "new", "hello world", "--type", "note", "--body", "hi"])
    assert note_result.exit_code == 0, note_result.output

    task_result = _invoke(["task", "new", "do a thing", "--body", "details"])
    assert task_result.exit_code == 0, task_result.output

    note_list = _invoke(["note", "list"])
    assert note_list.exit_code == 0, note_list.output
    assert "hello world" in note_list.output

    task_list = _invoke(["task", "list"])
    assert task_list.exit_code == 0, task_list.output
    assert "do a thing" in task_list.output


# --------------------------------------------------------------------------- #
# The missing-config message                                                  #
# --------------------------------------------------------------------------- #


def test_missing_config_message_names_path_and_requirement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    missing = tmp_path / "nope" / "config.toml"
    monkeypatch.setenv("MESH_CONFIG_PATH", str(missing))
    monkeypatch.delenv("MESH_AGENT", raising=False)

    result = _invoke(["note", "list"])

    assert result.exit_code == 2
    assert str(missing) in result.output
    assert "mesh init" in result.output
    assert "vault_path" in result.output


# --------------------------------------------------------------------------- #
# config.example.toml cannot rot                                              #
# --------------------------------------------------------------------------- #


def test_example_config_loads_and_documents_every_key() -> None:
    example = Path(__file__).resolve().parents[2] / "config.example.toml"
    assert example.is_file()

    cfg = load_config(example)
    assert cfg.core.vault_path
    assert cfg.core.agent
    assert cfg.search.collection
    assert cfg.search.hybrid is True
    assert cfg.search.threshold == pytest.approx(0.65)
    assert cfg.tasks.collections


# --------------------------------------------------------------------------- #
# init stays off the MCP surface                                              #
# --------------------------------------------------------------------------- #


def test_init_absent_from_mcp_tool_table() -> None:
    tools = asyncio.run(mcp_server.app.list_tools())
    names = {tool.name for tool in tools}
    assert "mesh_init" not in names
    assert not any("init" in name for name in names)


# --------------------------------------------------------------------------- #
# [search].threshold is omitted unless the caller asks for it                  #
# --------------------------------------------------------------------------- #


def test_init_omits_threshold_unless_passed(tmp_path: Path, cfg_path: Path) -> None:
    """A generated config must leave `[search].threshold` unset.

    The substring fallback applies `threshold` only when it is *explicit*
    (`SearchConfig.threshold_explicit`), so writing the schema default into
    every generated config would re-arm the very cutoff that fix removed: the
    body tier (0.4) and tag tier (0.6) both sit below 0.65 and become
    unreachable on a fresh install.
    """
    result = _invoke(["init", "--path", str(tmp_path / "vault")])
    assert result.exit_code == 0, result.output

    with cfg_path.open("rb") as fh:
        assert "threshold" not in tomllib.load(fh)["search"]
    cfg = load_config(cfg_path)
    assert cfg.search.threshold_explicit() is False
    assert cfg.search.threshold == pytest.approx(0.65)  # schema default, not written


def test_init_writes_threshold_when_explicitly_passed(tmp_path: Path, cfg_path: Path) -> None:
    result = _invoke(["init", "--path", str(tmp_path / "vault"), "--threshold", "0.8"])
    assert result.exit_code == 0, result.output

    cfg = load_config(cfg_path)
    assert cfg.search.threshold_explicit() is True
    assert cfg.search.threshold == pytest.approx(0.8)


def test_example_config_leaves_threshold_unset() -> None:
    """The committed reference must model the same fresh-install shape as `init`."""
    example = Path(__file__).resolve().parents[2] / "config.example.toml"
    assert load_config(example).search.threshold_explicit() is False
