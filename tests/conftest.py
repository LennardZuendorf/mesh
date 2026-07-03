"""Shared pytest fixtures for the brain test suite.

The keystone fixtures land with notes/1 (a sandboxed temp vault + a config
pointed at it via ``BRAIN_CONFIG_PATH``). This scaffold provides the import
surface so an empty ``uv run pytest`` is green before any feature exists.
"""

from __future__ import annotations

import pytest


@pytest.fixture()
def anchor() -> bool:
    """Placeholder fixture proving the suite collects. Removed once notes/1 lands."""
    return True
