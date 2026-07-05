"""Configuration schema and loader.

Config lives at ``~/.brain/config.toml`` and is overridable in tests (and for
alternate vaults) via the ``BRAIN_CONFIG_PATH`` environment variable. The
runtime agent identity (``[core].agent``) is in turn overridable by
``$BRAIN_AGENT`` — when both are present, the environment wins.

A missing config file is a validation error: ``load_config`` raises
``SystemExit(2)`` (exit code 2 == validation, per the root tech contract).
"""

from __future__ import annotations

import os
import tomllib
from pathlib import Path

from pydantic import AliasChoices, BaseModel, ConfigDict, Field, field_validator

_ENV_CONFIG_PATH = "BRAIN_CONFIG_PATH"
_ENV_AGENT = "BRAIN_AGENT"
_DEFAULT_CONFIG_PATH = Path.home() / ".brain" / "config.toml"
_VALIDATION_EXIT_CODE = 2


class CoreConfig(BaseModel):
    """``[core]`` — vault location and default agent identity."""

    model_config = ConfigDict(populate_by_name=True)

    # Field name is ``tolaria_path``; the root tech contract spells the TOML key
    # ``path``, so accept both on input.
    tolaria_path: Path = Field(
        validation_alias=AliasChoices("tolaria_path", "path"),
    )
    agent: str | None = None

    @field_validator("tolaria_path", mode="after")
    @classmethod
    def _expand_user(cls, value: Path) -> Path:
        """Expand a leading ``~`` — ``realpath`` does not, so ``~/vault`` would
        otherwise become a literal ``./~/vault`` under the process CWD."""
        return value.expanduser()


class SearchConfig(BaseModel):
    """``[search]`` — how ``brain search`` talks to ``indexed``."""

    collection: str | None = None
    hybrid: bool = True
    threshold: float = 0.65


class TasksConfig(BaseModel):
    """``[tasks]`` — valid agent identities for ``--owner`` validation."""

    collections: list[str] = Field(default_factory=list)


class Config(BaseModel):
    """Top-level brain configuration parsed from ``config.toml``."""

    core: CoreConfig
    search: SearchConfig = Field(default_factory=SearchConfig)
    tasks: TasksConfig = Field(default_factory=TasksConfig)

    @property
    def agent(self) -> str | None:
        """Resolved agent identity (``$BRAIN_AGENT`` override applied at load)."""
        return self.core.agent


def resolve_config_path() -> Path:
    """Config path: ``$BRAIN_CONFIG_PATH`` if set, else ``~/.brain/config.toml``."""
    override = os.environ.get(_ENV_CONFIG_PATH)
    if override:
        return Path(override).expanduser()
    return _DEFAULT_CONFIG_PATH


def load_config(path: Path | None = None) -> Config:
    """Load and validate the brain config.

    ``path`` defaults to :func:`resolve_config_path`. A missing file raises
    ``SystemExit(2)``. ``$BRAIN_AGENT`` overrides ``[core].agent`` when set.
    """
    cfg_path = path if path is not None else resolve_config_path()
    if not cfg_path.is_file():
        raise SystemExit(_VALIDATION_EXIT_CODE)

    with cfg_path.open("rb") as fh:
        data = tomllib.load(fh)

    agent_override = os.environ.get(_ENV_AGENT)
    if agent_override:
        core = data.setdefault("core", {})
        if isinstance(core, dict):
            core["agent"] = agent_override

    return Config.model_validate(data)
