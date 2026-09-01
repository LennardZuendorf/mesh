"""agent-usability/1 — MCP ``instructions`` block.

Acceptance coverage (``.spec/features/agent-usability/plan.md`` unit 1):

* **Config-driven, not static** — two different configs render different identity/
  roster/vault text (a constant string would pass a weaker test).
* **Degradation** — no config, no agent, and an empty roster each still render, with
  a named ``shards init`` statement, and never raise.
* **Cooperative phrasing** — no permission/authorization language, checked against a
  denylist, across a config, a no-agent config, and no config at all.
* **Budget** — every rendered variant (including a large roster) stays <= 2048 bytes.
* **The server receives it** — ``shards.mcp.server``'s constructed ``app.instructions``
  matches ``build_instructions`` for the same config, in a fresh interpreter (so the
  guarded import-time load is exercised for real, not mocked), both with a real
  config file and with none.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

from shards.mcp.instructions import BUDGET_BYTES, build_instructions
from shards.schemas.config import Config, CoreConfig, SearchConfig, TasksConfig

# Terms that would imply owner/claimed_by is a verified, enforced authorization
# boundary rather than the cooperative convention it actually is (root AGENTS.md
# §6). None of these may appear anywhere in the rendered block, in any config
# state — this is the same denylist agent-usability/8's skill body is checked
# against, since unit 1 sets the phrasing precedent.
_AUTH_DENYLIST = (
    "permission",
    "permit",
    "authoriz",
    "unauthoriz",
    "access denied",
    "forbidden",
    "not allowed",
    "enforc",
    "restrict",
    "privilege",
    "grant",
    "credential",
)


def _config(
    *,
    agent: str | None = "flights-agent",
    collections: list[str] | None = None,
    vault_path: str = "/home/agent/vault",
    hybrid: bool = True,
    collection: str | None = "shards-vault",
) -> Config:
    return Config(
        core=CoreConfig(vault_path=Path(vault_path), agent=agent),
        search=SearchConfig(collection=collection, hybrid=hybrid),
        tasks=TasksConfig(
            collections=collections if collections is not None else ["flights-agent"]
        ),
    )


# --------------------------------------------------------------------------- #
# Config-driven content                                                       #
# --------------------------------------------------------------------------- #


def test_block_names_resolved_identity_and_roster() -> None:
    config = _config(
        agent="flights-agent",
        collections=["flights-agent", "notes-agent"],
        vault_path="/home/agent/vault",
    )
    block = build_instructions(config)
    assert "flights-agent" in block
    assert "notes-agent" in block
    assert "/home/agent/vault" in block


def test_block_varies_with_config_not_a_constant_string() -> None:
    """Two distinct configs must render distinct identity/roster/vault text —
    proves the block is actually built from ``config``, not hard-coded prose."""
    first = build_instructions(
        _config(agent="flights-agent", collections=["flights-agent"], vault_path="/vault/a")
    )
    second = build_instructions(
        _config(
            agent="notes-agent",
            collections=["notes-agent", "cowork-agent"],
            vault_path="/vault/b",
        )
    )
    assert first != second
    assert "flights-agent" in first
    assert "flights-agent" not in second
    assert "notes-agent" in second
    assert "cowork-agent" in second
    assert "cowork-agent" not in first
    assert "/vault/a" in first
    assert "/vault/b" in second


def test_recall_section_does_not_promise_hybrid_when_disabled() -> None:
    """Constraint 5: never claim ranked hybrid recall when it cannot be verified."""
    hybrid_off = build_instructions(_config(hybrid=False))
    recall_off = hybrid_off.split("## Recall", 1)[1].split("##", 1)[0].lower()
    assert "never calls indexed" in recall_off
    assert "substring fallback only" in recall_off

    hybrid_on = build_instructions(_config(hybrid=True))
    recall_on = hybrid_on.split("## Recall", 1)[1].split("##", 1)[0]
    # Even when hybrid *is* configured, the block must not promise it actually
    # ran — indexed/the daemon can still be down at call time. As of
    # agent-usability/4 a caller *can* check (shards_health, or a hit's mode
    # field) — the block must point there rather than re-claim the old "no way
    # to tell" gap.
    assert "degrades" in recall_on.lower() or "silently" in recall_on.lower()
    assert "shards_health" in recall_on


# --------------------------------------------------------------------------- #
# Degradation — no config / no agent / empty roster                          #
# --------------------------------------------------------------------------- #


def test_no_config_renders_degraded_block_and_names_shards_init() -> None:
    block = build_instructions(None)
    assert block  # renders, does not raise
    assert "shards init" in block
    assert "flights-agent" not in block


def test_no_agent_identity_renders_degraded_statement() -> None:
    config = _config(agent=None)
    block = build_instructions(config)
    assert "shards init" in block
    assert "## Your identity" in block


def test_empty_roster_renders_degraded_statement() -> None:
    config = _config(collections=[])
    block = build_instructions(config)
    assert "## Valid owners" in block
    section = block.split("## Valid owners", 1)[1].split("##", 1)[0]
    assert "no roster" in section.lower() or "empty" in section.lower()


@pytest.mark.parametrize(
    "config",
    [None, _config(agent=None, collections=[])],
    ids=["no-config", "partial-config"],
)
def test_degraded_variants_still_orient_an_agent(config: Config | None) -> None:
    """A degraded block must still be *useful*, not merely non-throwing.

    Asserting only "does not raise" would pass on an empty string — which is the
    one output that fails the block's whole purpose, since a client with no config
    is exactly the reader who most needs telling what to do next.
    """
    text = build_instructions(config)

    assert text.strip(), "a degraded instructions block must not be empty"
    assert "shards" in text.lower()
    assert "shards init" in text, "a config-less agent must be told how to fix it"


# --------------------------------------------------------------------------- #
# Cooperative phrasing (no authorization language)                            #
# --------------------------------------------------------------------------- #


@pytest.mark.parametrize(
    "config",
    [
        None,
        _config(agent=None, collections=[]),
        _config(agent="flights-agent", collections=["flights-agent", "notes-agent"]),
    ],
    ids=["no-config", "partial-config", "full-config"],
)
def test_no_authorization_language(config: Config | None) -> None:
    block = build_instructions(config).lower()
    hits = [term for term in _AUTH_DENYLIST if term in block]
    assert hits == [], f"authorization-implying language found: {hits}"


# --------------------------------------------------------------------------- #
# Size budget                                                                 #
# --------------------------------------------------------------------------- #


def test_instructions_name_no_notes_application() -> None:
    """The block an agent reads must not imply a particular notes app is required."""
    block = build_instructions(_config(vault_path="/home/agent/vault"))

    assert "tolaria" not in block.lower()
    assert "obsidian" not in block.lower()


def test_budget_constant_is_2048_bytes() -> None:
    assert BUDGET_BYTES == 2048


@pytest.mark.parametrize(
    "config",
    [
        None,
        _config(agent=None, collections=[]),
        _config(agent="flights-agent", collections=["flights-agent", "notes-agent"]),
        _config(collections=[f"agent-{i}" for i in range(25)]),  # a large roster
    ],
    ids=["no-config", "partial-config", "full-config", "large-roster"],
)
def test_rendered_block_stays_under_budget(config: Config | None) -> None:
    block = build_instructions(config)
    size = len(block.encode("utf-8"))
    assert size <= BUDGET_BYTES, (
        f"instructions block is {size} bytes, over the {BUDGET_BYTES} budget"
    )


# --------------------------------------------------------------------------- #
# The server actually receives the block                                     #
# --------------------------------------------------------------------------- #

_PROBE = (
    "import sys\n"
    "import shards.mcp.server as server\n"
    "sys.stdout.write(server.app.instructions or '')\n"
)


def _run_probe(env: dict[str, str]) -> str:
    result = subprocess.run(
        [sys.executable, "-c", _PROBE],
        capture_output=True,
        text=True,
        env=env,
        check=True,
    )
    return result.stdout


def test_server_app_instructions_matches_build_instructions_with_real_config(
    shards_config: Path,
) -> None:
    from shards.schemas.config import load_config

    env = dict(os.environ)
    env["SHARDS_CONFIG_PATH"] = str(shards_config)
    stdout = _run_probe(env)
    expected = build_instructions(load_config(shards_config))
    assert stdout == expected
    assert "test-agent" in stdout


def test_server_app_instructions_degrades_and_still_starts_without_config(
    tmp_path: Path,
) -> None:
    """A missing config must not stop the server from starting (product R1,
    scenario 2) — the subprocess exiting 0 *and* returning the degraded block
    both matter here."""
    missing = tmp_path / "does-not-exist" / "config.toml"
    env = dict(os.environ)
    env["SHARDS_CONFIG_PATH"] = str(missing)
    env.pop("SHARDS_AGENT", None)
    stdout = _run_probe(env)
    assert stdout == build_instructions(None)
    assert "shards init" in stdout
