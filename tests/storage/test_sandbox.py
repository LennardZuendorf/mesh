"""core-hardening/6 — sandbox escape vectors, each via the CLI and the MCP surface.

``storage/sandbox.py`` had 3 direct tests before this unit: in-sandbox accept,
base-itself accept, absolute-traversal reject, symlink-leaf reject. Untested:
the relative-candidate branch (100% of :func:`safe_resolve`'s own reachable
branches), and four escape *vectors* an id/target argument could carry, each
exercised through both public surfaces (CLI and MCP), not just the helper
directly:

1. An absolute out-of-vault path supplied as an id.
2. A symlinked directory *component* sitting mid-path (not the leaf file).
3. ``..`` traversal sequences embedded in an id/filename argument.
4. A hardlink — the one vector path-based sandboxing cannot see at all (a
   hardlinked file's canonical path *is* legitimately inside the vault), so
   what is actually verified there is substituted: that atomic replace
   (temp-file + ``os.replace``) severs the hardlink on write rather than
   corrupting the file it shares an inode with. See the module-level note on
   that test for why "rejected" does not apply to this vector the way it does
   to the other three.
"""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path

import frontmatter
import pytest
from fastmcp.exceptions import ToolError
from typer.testing import CliRunner

import shards.mcp.server as server
from shards.cli.__main__ import app
from shards.schemas.config import Config, load_config
from shards.storage.sandbox import safe_resolve

_NOW = "2026-01-01T09:00:00+00:00"


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _call_tool(name: str, params: dict[str, object]) -> dict[str, object]:
    result = asyncio.run(server.app.call_tool(name, params))
    return result.structured_content or {}  # type: ignore[no-any-return]


def _write_note_frontmatter(path: Path, note_id: str, title: str) -> None:
    meta: dict[str, object] = {
        "id": note_id,
        "type": "note",
        "title": title,
        "tags": [],
        "owner": "seed-agent",
        "created": _NOW,
        "updated": _NOW,
        "related": [],
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    post = frontmatter.Post("seed body")
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")


# --------------------------------------------------------------------------- #
# safe_resolve — the remaining reachable branch                               #
# --------------------------------------------------------------------------- #


def test_safe_resolve_accepts_relative_candidate(vault: Path) -> None:
    """The un-exercised branch: a *relative* candidate resolves against base."""
    resolved = safe_resolve(vault, Path("notes") / "n-rel.md")
    assert resolved == Path(os.path.realpath(vault / "notes" / "n-rel.md"))


# --------------------------------------------------------------------------- #
# Vector 1 — an absolute out-of-vault path supplied as an id                  #
# --------------------------------------------------------------------------- #


def test_cli_note_get_absolute_out_of_vault_id_is_not_found(cfg: Config) -> None:
    """The id is matched by *filename stem*, never treated as a path — an
    absolute path can never equal a real note's stem, so this is a clean
    not-found, never a read of the named file."""
    result = _invoke(["note", "get", "/etc/passwd"])
    assert result.exit_code == 3, result.output
    assert "root:" not in result.output  # no /etc/passwd content leaked


def test_cli_task_get_absolute_out_of_vault_id_is_not_found(cfg: Config) -> None:
    result = _invoke(["task", "get", "/etc/passwd"])
    assert result.exit_code == 3, result.output
    assert "root:" not in result.output


def test_mcp_note_get_absolute_out_of_vault_id_is_not_found(cfg: Config) -> None:
    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("shards_note_get", {"id": "/etc/passwd"}))
    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "not_found"
    assert "root:" not in payload["message"]


def test_mcp_task_get_absolute_out_of_vault_id_is_not_found(cfg: Config) -> None:
    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("shards_task_get", {"id": "/etc/passwd"}))
    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "not_found"
    assert "root:" not in payload["message"]


# --------------------------------------------------------------------------- #
# Vector 2 — a symlinked directory component sitting mid-path                 #
# --------------------------------------------------------------------------- #
#
# Not the leaf file (already covered: test_safe_resolve_rejects_symlink_escape)
# — an *ancestor directory* of the target. ``vault/notes`` itself is swapped
# for a symlink to an outside directory holding a legit-looking note; the walk
# (``Path.glob``/``rglob``) follows the symlink at the *root* it starts from
# (verified empirically — only recursive descent into interior symlinked
# subdirs is skipped), so the file is found, but its realpath resolves outside
# the vault and ``safe_resolve`` must still reject it.


def test_cli_note_get_symlinked_notes_dir_component_is_rejected(
    cfg: Config, vault: Path, tmp_path: Path
) -> None:
    outside = tmp_path / "outside_notes"
    outside.mkdir()
    _write_note_frontmatter(outside / "n-hack.md", "n-hack", "Hack")

    notes_dir = vault / "notes"
    import shutil

    shutil.rmtree(notes_dir)
    notes_dir.symlink_to(outside)

    result = _invoke(["note", "get", "n-hack"])
    assert result.exit_code != 0, result.output
    assert "Traceback" not in result.output
    assert "Hack" not in result.output  # the escaped file's content never surfaced


def test_mcp_note_get_symlinked_notes_dir_component_is_rejected(
    cfg: Config, vault: Path, tmp_path: Path
) -> None:
    outside = tmp_path / "outside_notes_mcp"
    outside.mkdir()
    _write_note_frontmatter(outside / "n-hack2.md", "n-hack2", "Hack2")

    notes_dir = vault / "notes"
    import shutil

    shutil.rmtree(notes_dir)
    notes_dir.symlink_to(outside)

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(server.app.call_tool("shards_note_get", {"id": "n-hack2"}))
    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "validation"  # safe_resolve's ValueError, not a leak
    assert "Hack2" not in json.dumps(payload)


# --------------------------------------------------------------------------- #
# Vector 3 — ``..`` traversal sequences embedded in an id/filename argument    #
# --------------------------------------------------------------------------- #


def test_cli_note_append_dotdot_id_is_not_found(cfg: Config, vault: Path) -> None:
    """A traversal-shaped id is still just a filename-stem lookup — never a
    path walk — so it is not found, and nothing outside the vault is touched."""
    outside_secret = vault.parent / "secret.md"
    outside_secret.write_text("SECRET", encoding="utf-8")

    result = _invoke(["note", "append", "../../secret", "pwned"])

    assert result.exit_code == 3, result.output
    assert outside_secret.read_text(encoding="utf-8") == "SECRET"  # untouched


def test_mcp_task_append_dotdot_id_is_not_found(cfg: Config, vault: Path) -> None:
    outside_secret = vault.parent / "secret_task.md"
    outside_secret.write_text("SECRET", encoding="utf-8")

    with pytest.raises(ToolError) as exc_info:
        asyncio.run(
            server.app.call_tool(
                "shards_task_append", {"task_id": "../../secret_task", "text": "pwned"}
            )
        )
    payload = json.loads(str(exc_info.value))
    assert payload["kind"] == "not_found"
    assert outside_secret.read_text(encoding="utf-8") == "SECRET"


# --------------------------------------------------------------------------- #
# Vector 4 — a hardlink: not rejectable by path, but not exploitable either   #
# --------------------------------------------------------------------------- #
#
# A hardlink has no separate "target" for realpath to resolve through — the
# vault-side path *is* the file, canonically, so ``safe_resolve`` has no path
# signal to reject and correctly does not try. What actually protects the
# outside file the inode is shared with is unrelated to sandboxing: every
# shards write goes through ``atomic_write`` (temp file + ``os.replace``),
# which always severs a hardlink on write rather than mutating the shared
# inode in place. This substitutes "rejected" with the property that is
# actually true and actually load-bearing here — reported as a substitution
# per the unit's instructions, not a silent reinterpretation.


def test_cli_note_append_through_hardlink_never_mutates_the_outside_file(
    cfg: Config, vault: Path, tmp_path: Path
) -> None:
    outside = tmp_path / "outside_hardlink_source.md"
    _write_note_frontmatter(outside, "n-hlink", "Hardlinked")
    original_outside_content = outside.read_text(encoding="utf-8")

    inside = vault / "notes" / "n-hlink.md"
    os.link(outside, inside)
    assert outside.stat().st_ino == inside.stat().st_ino  # genuinely the same inode

    result = _invoke(["note", "append", "n-hlink", "APPENDED-VIA-CLI"])
    assert result.exit_code == 0, result.output

    assert "APPENDED-VIA-CLI" in inside.read_text(encoding="utf-8")
    # The write replaced the vault-side directory entry (temp + os.replace),
    # severing the hardlink -- the outside file is untouched, not corrupted.
    assert outside.read_text(encoding="utf-8") == original_outside_content
    assert "APPENDED-VIA-CLI" not in outside.read_text(encoding="utf-8")
    assert outside.stat().st_ino != inside.stat().st_ino  # no longer shared


def test_mcp_note_append_through_hardlink_never_mutates_the_outside_file(
    cfg: Config, vault: Path, tmp_path: Path
) -> None:
    outside = tmp_path / "outside_hardlink_source_mcp.md"
    _write_note_frontmatter(outside, "n-hlink-mcp", "HardlinkedMCP")
    original_outside_content = outside.read_text(encoding="utf-8")

    inside = vault / "notes" / "n-hlink-mcp.md"
    os.link(outside, inside)
    assert outside.stat().st_ino == inside.stat().st_ino

    reply = _call_tool("shards_note_append", {"target": "n-hlink-mcp", "text": "APPENDED-VIA-MCP"})
    assert reply  # the tool call succeeded (no ToolError raised)

    assert "APPENDED-VIA-MCP" in inside.read_text(encoding="utf-8")
    assert outside.read_text(encoding="utf-8") == original_outside_content
    assert "APPENDED-VIA-MCP" not in outside.read_text(encoding="utf-8")
    assert outside.stat().st_ino != inside.stat().st_ino
