"""Configuration schema and loader.

Config lives at ``~/.shards/config.toml`` and is overridable in tests (and for
alternate vaults) via the ``SHARDS_CONFIG_PATH`` environment variable. The
runtime agent identity (``[core].agent``) is in turn overridable by
``$SHARDS_AGENT`` — when both are present, the environment wins.

A missing config file is a validation error: ``load_config`` raises
``SystemExit(2)`` (exit code 2 == validation, per the root tech contract), after
printing a stderr line naming the resolved path and the one required key
(agent-usability/7 — see :func:`_missing_config_message`). A malformed config
raises :class:`msgspec.ValidationError`.
"""

from __future__ import annotations

import os
import sys
import tomllib
from pathlib import Path
from typing import Any

import msgspec

_ENV_CONFIG_PATH = "SHARDS_CONFIG_PATH"
_ENV_AGENT = "SHARDS_AGENT"
_DEFAULT_CONFIG_PATH = Path.home() / ".shards" / "config.toml"
_VALIDATION_EXIT_CODE = 2


def _dec_hook(target: type, value: Any) -> Any:
    """Decode a config value msgspec has no builtin for — currently ``pathlib.Path``."""
    if target is Path:
        return Path(value)
    raise NotImplementedError(f"no decoder for {target!r}")


class CoreConfig(msgspec.Struct, kw_only=True):
    """``[core]`` — vault location and default agent identity."""

    tolaria_path: Path
    agent: str | None = None

    def __post_init__(self) -> None:
        """Expand a leading ``~`` — ``realpath`` does not, so ``~/vault`` would
        otherwise become a literal ``./~/vault`` under the process CWD."""
        self.tolaria_path = self.tolaria_path.expanduser()


class SearchConfig(msgspec.Struct, kw_only=True):
    """``[search]`` — how ``shards search`` talks to ``indexed``.

    ``_explicit`` records which ``[search]`` keys were actually present in the
    raw TOML mapping. msgspec ``Struct``s expose no ``fields_set`` /
    ``__pydantic_fields_set__`` equivalent, so :func:`load_config` populates
    this by inspecting the parsed mapping directly (the same place the
    ``[core].path`` alias is already resolved) rather than the decoded
    ``Config``. It exists solely to answer :meth:`threshold_explicit` — the
    substring fallback (root tech.md § B5) must apply ``threshold`` only when
    a caller set it explicitly, never on the decoded default. Not a general
    field-provenance mechanism: nothing else reads it.
    """

    collection: str | None = None
    hybrid: bool = True
    threshold: float = 0.65
    _explicit: frozenset[str] = msgspec.field(default_factory=frozenset)

    def threshold_explicit(self) -> bool:
        """Whether ``[search].threshold`` was set explicitly in ``config.toml``."""
        return "threshold" in self._explicit


class TasksConfig(msgspec.Struct, kw_only=True):
    """``[tasks]`` — valid agent identities for ``--owner`` validation."""

    collections: list[str] = msgspec.field(default_factory=list)


class Config(msgspec.Struct, kw_only=True):
    """Top-level shards configuration parsed from ``config.toml``."""

    core: CoreConfig
    search: SearchConfig = msgspec.field(default_factory=SearchConfig)
    tasks: TasksConfig = msgspec.field(default_factory=TasksConfig)

    @property
    def agent(self) -> str | None:
        """Resolved agent identity (``$SHARDS_AGENT`` override applied at load)."""
        return self.core.agent


def _missing_config_message(cfg_path: Path) -> str:
    """The stderr line printed when ``cfg_path`` does not exist (agent-usability/7).

    Names the exact resolved path (not just "the config") and the one key
    ``load_config`` cannot default its way around, so a first-run agent or
    operator is pointed at both the fix (``shards init``) and the shape it
    produces, instead of a bare exit 2. This is a message only — the exit
    code and exception type are the pre-existing, documented contract
    (agent-usability/5 replaces ``SystemExit`` with a typed, catchable error;
    that restructuring is out of scope here).
    """
    return (
        f"shards: no config found at {cfg_path}\n"
        "run `shards init` to create one (honours $SHARDS_CONFIG_PATH), "
        "or point $SHARDS_CONFIG_PATH at an existing config.\n"
        "required: [core].tolaria_path (path to your Tolaria vault folder); "
        "[core].agent, [search], and [tasks] are optional and default."
    )


def resolve_config_path() -> Path:
    """Config path: ``$SHARDS_CONFIG_PATH`` if set, else ``~/.shards/config.toml``."""
    override = os.environ.get(_ENV_CONFIG_PATH)
    if override:
        return Path(override).expanduser()
    return _DEFAULT_CONFIG_PATH


def load_config(path: Path | None = None) -> Config:
    """Load and validate the shards config.

    ``path`` defaults to :func:`resolve_config_path`. A missing file raises
    ``SystemExit(2)``. ``$SHARDS_AGENT`` overrides ``[core].agent`` when set. The
    root tech contract spells the vault key ``[core].path``; the field name is
    ``tolaria_path``, so both are accepted on input.
    """
    cfg_path = path if path is not None else resolve_config_path()
    if not cfg_path.is_file():
        print(_missing_config_message(cfg_path), file=sys.stderr)
        raise SystemExit(_VALIDATION_EXIT_CODE)

    with cfg_path.open("rb") as fh:
        data = tomllib.load(fh)

    # Record which ``[search]`` keys the raw TOML actually set, before decoding
    # loses that information — msgspec has no ``fields_set`` to ask afterward.
    raw_search = data.get("search")
    explicit_search_keys: frozenset[str] = (
        frozenset(str(key) for key in raw_search) if isinstance(raw_search, dict) else frozenset()
    )

    core = data.get("core")
    if isinstance(core, dict):
        core = dict(core)
        # Accept the ``[core].path`` spelling as an alias for ``tolaria_path``.
        if "path" in core and "tolaria_path" not in core:
            core["tolaria_path"] = core.pop("path")
        agent_override = os.environ.get(_ENV_AGENT)
        if agent_override:
            core["agent"] = agent_override
        data = {**data, "core": core}

    config = msgspec.convert(data, Config, dec_hook=_dec_hook)
    config.search._explicit = explicit_search_keys
    return config
