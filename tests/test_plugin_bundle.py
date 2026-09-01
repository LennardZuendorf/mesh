"""agent-usability/8 — plugin bundle + the ``mesh`` skill.

Acceptance coverage (``.spec/features/agent-usability/plan.md`` unit 8):

* ``plugin.json`` / ``.mcp.json`` / ``hooks.json`` / ``marketplace.json`` are valid JSON and
  reference only console scripts that actually exist (``mesh``, ``mesh-mcp``, both wired in
  ``pyproject.toml``).
* ``SKILL.md`` frontmatter keys are a subset of the six-field spec ({``name``, ``description``,
  ``license``, ``compatibility``, ``metadata``, ``allowed-tools``}); ``name`` is ``mesh``.
* The skill body states all seven vault-coherence rules and carries no authorization-implying
  language — the same denylist ``tests/memory/test_instructions.py`` checks the MCP
  ``instructions`` block against (unit 1 sets the phrasing precedent this unit must match).
* The skill body does not reproduce the instructions block's own live-config section headers
  (identity / roster / vault path) — those are runtime-only by construction, so a static skill
  file can only name them, never render them.
* Exactly one skill directory ships under ``plugins/``; the developer-facing
  ``.agents/skills/spec`` and ``.claude/skills/spec`` are untouched.
* Every CLI command/flag and every ``mesh_*`` MCP tool the skill body names is checked against
  the real, live-introspected surface — a renamed or removed command/tool fails this suite
  rather than silently going stale in prose.
"""

from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path
from typing import Any

import pytest
import typer
import yaml
from typer.core import TyperGroup

from tests.memory.test_instructions import _AUTH_DENYLIST

REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGIN_ROOT = REPO_ROOT / "plugins" / "mesh"


# --------------------------------------------------------------------------- #
# Console scripts — the one source of truth for what a command may name       #
# --------------------------------------------------------------------------- #


def _console_scripts() -> dict[str, str]:
    data = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    return dict(data["project"]["scripts"])


def test_console_scripts_are_mesh_and_mesh_mcp() -> None:
    """Pin the fact the whole bundle's ``command`` fields are checked against."""
    assert _console_scripts() == {
        "mesh": "mesh.cli.__main__:app",
        "mesh-mcp": "mesh.mcp.server:main",
    }


# --------------------------------------------------------------------------- #
# JSON files parse and reference only real console scripts                    #
# --------------------------------------------------------------------------- #


def _load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def test_plugin_json_is_valid_and_named_mesh() -> None:
    data = _load_json(PLUGIN_ROOT / ".claude-plugin" / "plugin.json")
    assert data["name"] == "mesh"
    assert data["description"]
    assert data["license"] == "MIT"


def test_mcp_json_is_valid_and_references_mesh_mcp() -> None:
    data = _load_json(PLUGIN_ROOT / ".mcp.json")
    servers = data["mcpServers"]
    assert set(servers) == {"mesh"}
    command = servers["mesh"]["command"]
    assert command in _console_scripts()
    assert command == "mesh-mcp"


def test_hooks_json_is_valid_and_matches_the_existing_session_start_payload() -> None:
    """Constraint 5: the bundled hook must be the existing, already-verified payload."""
    bundled = _load_json(PLUGIN_ROOT / "hooks" / "hooks.json")
    existing = _load_json(REPO_ROOT / "hooks" / "session_start.json")
    assert bundled == existing

    command = bundled["hooks"]["SessionStart"][0]["hooks"][0]["command"]
    scripts = _console_scripts()
    invoked = command.split()[0]
    assert invoked in scripts, f"hook invokes {invoked!r}, not a real console script"


def test_marketplace_json_is_valid_and_points_at_the_plugin_dir() -> None:
    data = _load_json(REPO_ROOT / ".claude-plugin" / "marketplace.json")
    assert data["name"] == "mesh"
    assert data["owner"]["name"]
    assert len(data["plugins"]) == 1
    entry = data["plugins"][0]
    assert entry["name"] == "mesh"
    assert entry["source"] == "./plugins/mesh"
    # The relative path a "repo doubles as its own marketplace" entry promises
    # must actually resolve on disk from the marketplace root (the repo root).
    assert (REPO_ROOT / entry["source"].removeprefix("./")).is_dir()


@pytest.mark.parametrize(
    "path",
    [
        PLUGIN_ROOT / ".claude-plugin" / "plugin.json",
        PLUGIN_ROOT / ".mcp.json",
        PLUGIN_ROOT / "hooks" / "hooks.json",
        REPO_ROOT / ".claude-plugin" / "marketplace.json",
    ],
    ids=["plugin.json", "mcp.json", "hooks.json", "marketplace.json"],
)
def test_bundle_json_files_only_name_real_console_scripts(path: Path) -> None:
    """No JSON file in the bundle may name a command outside {mesh, mesh-mcp}."""
    scripts = set(_console_scripts())
    text = path.read_text(encoding="utf-8")
    # Every occurrence of a bare "mesh" or "mesh-mcp" token as a command word
    # (not as a substring of some other identifier like "mesh_note_get") must be
    # one of the two real console scripts.
    for token in re.findall(r"\bmesh(?:-mcp)?\b", text):
        assert token in scripts, f"{path.name} names unknown command {token!r}"


# --------------------------------------------------------------------------- #
# SKILL.md frontmatter — the six-field spec subset                            #
# --------------------------------------------------------------------------- #

_SIX_FIELD_SUBSET = {"name", "description", "license", "compatibility", "metadata", "allowed-tools"}
_SKILL_PATH = PLUGIN_ROOT / "skills" / "mesh" / "SKILL.md"


def _skill_text() -> str:
    return _SKILL_PATH.read_text(encoding="utf-8")


def _skill_frontmatter() -> dict[str, Any]:
    text = _skill_text()
    match = re.match(r"^---\n(.*?)\n---\n(.*)$", text, re.DOTALL)
    assert match, "SKILL.md must open with a --- frontmatter block"
    data = yaml.safe_load(match.group(1))
    assert isinstance(data, dict)
    return data


def _skill_body() -> str:
    text = _skill_text()
    match = re.match(r"^---\n(.*?)\n---\n(.*)$", text, re.DOTALL)
    assert match
    return match.group(2)


def test_skill_file_lives_at_the_spec_layout_path() -> None:
    assert _SKILL_PATH.is_file()


def test_skill_frontmatter_is_a_subset_of_the_six_field_spec() -> None:
    fm = _skill_frontmatter()
    extra = set(fm.keys()) - _SIX_FIELD_SUBSET
    assert extra == set(), f"SKILL.md frontmatter has non-spec keys: {extra}"


def test_skill_frontmatter_name_is_mesh() -> None:
    assert _skill_frontmatter()["name"] == "mesh"


def test_skill_frontmatter_has_a_description() -> None:
    assert _skill_frontmatter()["description"].strip()


# --------------------------------------------------------------------------- #
# Seven protocol rules, cooperative phrasing                                  #
# --------------------------------------------------------------------------- #

# Each entry is a distinguishing substring for one of the brief's seven rules,
# checked case-insensitively — loose enough to survive minor copy-editing,
# tight enough that dropping a rule (or renaming it beyond recognition) fails.
_SEVEN_RULES = (
    "search before you write",
    "append rather than fork a near-duplicate",
    "tag from the existing vocabulary",
    "link when a note continues another",
    "claim before you work",
    "always finish with an outcome",
    "cancel is for tasks that shouldn't exist, not tasks you failed",
)


def test_skill_body_states_all_seven_protocol_rules() -> None:
    body = _skill_body().lower()
    missing = [rule for rule in _SEVEN_RULES if rule not in body]
    assert missing == [], f"skill body is missing rule(s): {missing}"


def test_skill_body_has_no_authorization_language() -> None:
    """Same denylist unit 1's instructions-block test uses (phrasing precedent)."""
    body = _skill_body().lower()
    hits = [term for term in _AUTH_DENYLIST if term in body]
    assert hits == [], f"authorization-implying language found: {hits}"


def test_skill_frontmatter_has_no_authorization_language() -> None:
    fm_text = _skill_text().split("---", 2)[1].lower()
    hits = [term for term in _AUTH_DENYLIST if term in fm_text]
    assert hits == [], f"authorization-implying language found in frontmatter: {hits}"


def test_skill_does_not_duplicate_instructions_block_live_config_sections() -> None:
    """The instructions block's own section headers are runtime-only; a static
    skill file can name them but must never reproduce them as its own sections."""
    body = _skill_body()
    for header in ("## Your identity", "## Valid owners", "## Vault\n"):
        assert header not in body, f"skill body duplicates live-config section {header!r}"


# --------------------------------------------------------------------------- #
# Exactly one skill under plugins/; developer skill set untouched             #
# --------------------------------------------------------------------------- #


def test_exactly_one_skill_directory_under_plugins() -> None:
    skill_dirs = sorted((REPO_ROOT / "plugins").glob("*/skills/*/"))
    assert [str(p.relative_to(REPO_ROOT)) for p in skill_dirs] == ["plugins/mesh/skills/mesh"]


def test_developer_skill_set_is_untouched() -> None:
    assert (REPO_ROOT / ".agents" / "skills" / "spec" / "SKILL.md").is_file()
    symlink = REPO_ROOT / ".claude" / "skills" / "spec"
    assert symlink.is_symlink()
    assert symlink.resolve() == (REPO_ROOT / ".agents" / "skills" / "spec").resolve()


# --------------------------------------------------------------------------- #
# Every command/tool name the skill mentions actually exists                  #
# --------------------------------------------------------------------------- #


def _cli_note_task_subcommands() -> dict[str, set[str]]:
    from mesh.cli.note import note_app
    from mesh.cli.task import task_app

    return {
        "note": {c.name for c in note_app.registered_commands if c.name is not None},
        "task": {c.name for c in task_app.registered_commands if c.name is not None},
    }


def _cli_top_level_names() -> set[str]:
    from mesh.cli.__main__ import _LEAVES, _SUBAPPS

    return set(_SUBAPPS) | set(_LEAVES)


def _cli_flags(*, group: str, command: str) -> set[str]:
    """Every ``--flag`` a real CLI command accepts, via click introspection.

    Typer 0.26 vendors its own click fork under ``typer._click`` (the ``app``'s
    resolved command tree is built from *that* ``Command``/``Context``, not the
    ``click`` package on PyPI) — so introspection here goes through
    ``typer.core.TyperGroup`` / ``typer._click.core.Context``, the same vendored
    types ``cli/__main__.py`` itself is annotated against.
    """
    from typer._click.core import Context as ClickContext

    from mesh.cli.__main__ import app

    click_app = typer.main.get_command(app)
    assert isinstance(click_app, TyperGroup)

    ctx = ClickContext(click_app)
    if group == "":
        target = click_app.get_command(ctx, command)
    else:
        sub = click_app.get_command(ctx, group)
        assert sub is not None
        assert isinstance(sub, TyperGroup)
        sub_ctx = ClickContext(sub, parent=ctx)
        target = sub.get_command(sub_ctx, command)
    assert target is not None, f"no such CLI command: {group} {command}".strip()
    opts: set[str] = set()
    for param in target.params:
        opts.update(getattr(param, "opts", []))
    return opts


def _mcp_tool_names() -> set[str]:
    import asyncio

    import mesh.mcp.server as server

    tools = asyncio.run(server.app.list_tools())
    return {tool.name for tool in tools}


def test_skill_mentions_only_real_note_and_task_subcommands() -> None:
    body = _skill_body()
    live = _cli_note_task_subcommands()
    for verb, block in (("note", live["note"]), ("task", live["task"])):
        match = re.search(rf"`{verb} \{{([a-z,]+)\}}`", body)
        assert match, f"skill body has no `{verb} {{...}}` command listing"
        named = set(match.group(1).split(","))
        assert named <= block, f"{verb} lists unknown subcommand(s): {named - block}"
        assert named, f"{verb} listing is empty"


def test_skill_mentions_only_real_top_level_leaf_commands() -> None:
    body = _skill_body()
    live = _cli_top_level_names()
    for name in ("recent-activity", "build-context", "graph", "project", "session-start", "init"):
        assert name in body, f"skill body never mentions {name!r}"
        assert name in live, f"{name!r} is not a real mesh command"
    assert "search" in body
    assert "search" in live


def test_skill_mentions_only_real_cli_flags() -> None:
    body = _skill_body()
    task_list_flags = _cli_flags(group="task", command="list")
    for flag in ("--stale", "--available", "--status", "--owner", "--mine", "--tags"):
        assert flag in body
        assert flag in task_list_flags, f"{flag} is not a real `task list` flag"

    graph_flags = _cli_flags(group="", command="graph")
    assert "--direction" in body
    assert "--direction" in graph_flags

    session_start_flags = _cli_flags(group="", command="session-start")
    assert "--team" in body
    assert "--team" in session_start_flags

    task_finish_flags = _cli_flags(group="task", command="finish")
    assert "--outcome" in body
    assert "--outcome" in task_finish_flags


def test_skill_tags_help_matches_the_real_merge_semantics() -> None:
    """The most-changed, most-likely-to-drift claim: bare `--tags` merges, not replaces."""
    from mesh.core.notes import TAG_SPEC_SEMANTICS

    body = _skill_body()
    assert "merges" in body.lower()
    assert "adds" in TAG_SPEC_SEMANTICS.lower()
    # The delta/replace forms named in the skill must be the ones the real
    # semantics string actually documents, not a stale prior contract.
    assert "+x,-y" in body and "+x,-y" in TAG_SPEC_SEMANTICS
    assert "=x,y" in body and "=x,y" in TAG_SPEC_SEMANTICS


def test_tag_paragraph_correctly_distinguishes_delta_from_replace() -> None:
    """Regression (review round 1, finding 1): "=x,y — the only form that drops
    anything" was false and self-contradictory — the delta form's own `-y` also
    drops (removes) whatever it names. The corrected wording must state delta's
    removal explicitly and scope replace's danger to tags the caller *didn't*
    name, and the old blanket claim must never reappear.
    """
    body = _skill_body()
    lowered = body.lower()
    assert "only form that drops anything" not in lowered
    assert "removes exactly the tags you name" in lowered
    assert "left out of the new list is discarded" in lowered


def test_allowed_tools_are_all_real_registered_mcp_tools() -> None:
    fm = _skill_frontmatter()
    allowed = fm["allowed-tools"]
    assert isinstance(allowed, list) and allowed, "allowed-tools must be a non-empty list"
    live_tools = _mcp_tool_names()
    for entry in allowed:
        assert entry.startswith("mcp__mesh__"), f"unexpected allowed-tools entry: {entry}"
        tool_name = entry.removeprefix("mcp__mesh__")
        assert tool_name in live_tools, f"allowed-tools names a non-existent tool: {tool_name}"


def test_allowed_tools_excludes_every_destructive_tool() -> None:
    """Constraint 4: allowed-tools must never pre-approve a destructive verb.

    ``mesh_task_cancel`` is the only MCP tool registered with ``destructiveHint``
    (server.py's own ``_DESTRUCTIVE`` annotation group); the two hard-unlink delete
    verbs (``note delete`` / ``task delete``) are never MCP tools at all, so they
    cannot appear here regardless — :func:`test_delete_verbs_do_not_exist_as_mcp_tools`
    pins that separately.
    """
    import asyncio

    import mesh.mcp.server as server

    tools = asyncio.run(server.app.list_tools())
    destructive = {t.name for t in tools if (t.annotations and t.annotations.destructiveHint)}
    assert destructive == {"mesh_task_cancel"}

    fm = _skill_frontmatter()
    allowed = {e.removeprefix("mcp__mesh__") for e in fm["allowed-tools"]}
    assert allowed.isdisjoint(destructive), (
        f"allowed-tools pre-approves destructive tool(s): {allowed & destructive}"
    )


def test_delete_verbs_do_not_exist_as_mcp_tools() -> None:
    live_tools = _mcp_tool_names()
    assert "mesh_note_delete" not in live_tools
    assert "mesh_task_delete" not in live_tools


def test_skill_mentions_mesh_health_and_it_is_a_real_tool() -> None:
    body = _skill_body()
    assert "mesh_health" in body
    assert "mesh_health" in _mcp_tool_names()


def test_skill_mentions_mesh_session_start_and_it_is_a_real_tool() -> None:
    body = _skill_body()
    assert "mesh_session_start" in body
    assert "mesh_session_start" in _mcp_tool_names()


def test_bundle_states_no_notes_application_prerequisite() -> None:
    """Install-time metadata must not read as 'you need a particular notes app'."""
    bundle = [
        REPO_ROOT / ".claude-plugin" / "marketplace.json",
        PLUGIN_ROOT / ".claude-plugin" / "plugin.json",
        _SKILL_PATH,
    ]

    for path in bundle:
        text = path.read_text(encoding="utf-8").lower()
        assert "tolaria" not in text, path
        assert "obsidian" not in text, path
