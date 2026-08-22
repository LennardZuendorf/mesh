"""Shared helper: write a config for an arbitrary vault path and export it.

The vault-root defect tests each need a config pointing somewhere the ``vault``
fixture deliberately cannot go — a path that does not exist, a regular file, a
vault with no ``[search].collection``. One writer serves all three rather than
each module hand-rolling the same five TOML lines.
"""

from __future__ import annotations

from pathlib import Path

import pytest


def point_at(
    monkeypatch: pytest.MonkeyPatch,
    cfg_path: Path,
    vault: Path,
    *,
    collection: str | None = None,
) -> None:
    """Write a config for ``vault`` at ``cfg_path`` and export $SHARDS_CONFIG_PATH."""
    lines = ["[core]", f'vault_path = "{vault}"', 'agent = "test-agent"', "", "[search]"]
    if collection is not None:
        lines.append(f'collection = "{collection}"')
    lines.append("hybrid = true")
    cfg_path.write_text("\n".join([*lines, ""]), encoding="utf-8")
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(cfg_path))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
