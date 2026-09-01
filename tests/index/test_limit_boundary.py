"""`--limit 0` means none, not all.

Both selectors read `if limit >= 0: return results[:limit]`, with a negative limit
documented as unbounded. A mutation audit found that flipping `>= 0` to `> 0` —
which silently turns "give me nothing" into "give me everything" — survived the
entire suite in both files: the negative case is covered everywhere, zero never
was. `limit=0` is reachable from `--limit 0` and from the MCP `limit` parameter.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from mesh.core.notes import create_note
from mesh.index.fallback import search_fallback
from mesh.index.tagpull import tagpull
from mesh.schemas.config import Config


def _seed(cfg: Config) -> None:
    create_note(cfg, "Zero Boundary Alpha", body="alpha body", tags=["boundary"])
    create_note(cfg, "Zero Boundary Beta", body="beta body", tags=["boundary"])


def test_substring_fallback_honours_a_zero_limit(cfg: Config, vault: Path) -> None:
    _seed(cfg)
    assert search_fallback(cfg, "Boundary", limit=-1), "precondition: the query matches"

    assert search_fallback(cfg, "Boundary", limit=0) == []


def test_tag_pull_honours_a_zero_limit(cfg: Config, vault: Path) -> None:
    _seed(cfg)
    assert tagpull(cfg, ["boundary"], limit=-1), "precondition: the tag matches"

    assert tagpull(cfg, ["boundary"], limit=0) == []


def test_a_negative_limit_stays_unbounded(cfg: Config, vault: Path) -> None:
    """The documented sentinel, pinned alongside its neighbour."""
    _seed(cfg)

    assert len(search_fallback(cfg, "Boundary", limit=-1)) == 2
    assert len(tagpull(cfg, ["boundary"], limit=-1)) == 2


@pytest.fixture
def cfg(mesh_config: Path) -> Config:
    """The loaded config for the tmp vault the shared fixture just wrote."""
    from mesh.schemas.config import load_config

    return load_config()
