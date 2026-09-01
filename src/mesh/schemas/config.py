"""Configuration schema and loader.

Config lives at ``~/.mesh/config.toml`` and is overridable in tests (and for
alternate vaults) via the ``MESH_CONFIG_PATH`` environment variable. The
runtime agent identity (``[core].agent``) is in turn overridable by
``$MESH_AGENT`` — when both are present, the environment wins.

A missing config file is a validation error: ``load_config`` raises
:class:`ConfigMissingError` (code 2 == validation, per the root tech contract),
carrying the resolved path and a message naming it plus the one required key
(agent-usability/7 — see :func:`_missing_config_message`). A malformed config
raises :class:`msgspec.ValidationError`.

``ConfigMissingError`` replaces a former bare ``SystemExit(2)`` (agent-usability/5):
``SystemExit`` is a ``BaseException``, which neither the CLI boundary mapper
(:func:`mesh.cli._errors.cli_errors`) nor the MCP one (``mcp/server.py::_guarded``)
catches — both catch ``Exception`` — so on an MCP-only machine a missing config
could escape a tool call as an unhandled crash instead of a clean tool error. A
:class:`~mesh.core.errors.MeshError` subclass is a plain ``Exception``, so
both boundaries catch it exactly like any other domain exception; the CLI still
exits 2 (:class:`ConfigMissingError.code`), now via the one mapper instead of a
bespoke ``SystemExit``.
"""

from __future__ import annotations

import os
import tomllib
from pathlib import Path
from typing import Any, Final

import msgspec

from mesh.core.errors import MeshError

_ENV_CONFIG_PATH = "MESH_CONFIG_PATH"
_ENV_AGENT = "MESH_AGENT"
_DEFAULT_CONFIG_PATH = Path.home() / ".mesh" / "config.toml"
_VALIDATION_EXIT_CODE = 2


class ConfigMissingError(MeshError):
    """No config file found at the resolved path (CLI exit 2, MCP tool error).

    Carries ``cfg_path`` (the resolved path that does not exist) so both
    boundary mappers can surface it structurally, not just embedded in prose.
    The message is :func:`_missing_config_message` verbatim — one wording,
    read by the CLI's stderr line and the MCP structured payload's ``message``
    field alike.
    """

    code = _VALIDATION_EXIT_CODE

    def __init__(self, cfg_path: Path) -> None:
        self.cfg_path = cfg_path
        super().__init__(_missing_config_message(cfg_path))


def _dec_hook(target: type, value: Any) -> Any:
    """Decode a config value msgspec has no builtin for — currently ``pathlib.Path``."""
    if target is Path:
        return Path(value)
    raise NotImplementedError(f"no decoder for {target!r}")


class CoreConfig(msgspec.Struct, kw_only=True):
    """``[core]`` — vault location and default agent identity."""

    vault_path: Path
    agent: str | None = None

    def __post_init__(self) -> None:
        """Canonicalise the vault path once, here, at the loader boundary.

        Two steps, in this order:

        1. Expand a leading ``~`` — ``realpath`` does not, so ``~/vault`` would
           otherwise become a literal ``./~/vault`` under the process CWD.
        2. ``resolve()`` — absolutise and follow symlinks, so every consumer
           (the corpus walkers, the sandbox's ``safe_resolve``, the daemon's
           watcher and its scope predicates) speaks one path space. The
           watcher reports realpaths; comparing those against an unresolved
           ``vault_path`` puts a symlinked vault (external drive, cloud-synced
           folder, container bind mount, macOS ``/var`` → ``/private/var``) out
           of scope for every file touched after daemon start, emptying the
           warm index behind ``note list`` / ``task list`` / ``status``.

        ``resolve()`` is deliberately non-strict (its default): a vault that
        does not exist yet must still load — ``mesh init`` creates it lazily,
        and the missing directory is reported by ``mesh status``, not by
        refusing to parse the config.
        """
        self.vault_path = self.vault_path.expanduser().resolve()
        # A vault that does not exist yet is fine (created lazily). A vault that
        # exists and is *not a directory* never can be: every write below it would
        # fail with a bare ``ENOTDIR`` at exit 1, an internal-error code for what is
        # plainly a bad config value. Reject it here so it reads as validation.
        if self.vault_path.exists() and not self.vault_path.is_dir():
            raise ValueError(f"[core].vault_path is not a directory: {self.vault_path}")


#: Legacy ``[core]`` spellings of the vault key, accepted on input forever.
#: ``path`` was the root-spec spelling; ``tolaria_path`` predates the rename to
#: a tool-neutral name. Order is precedence: the first present wins, and an
#: explicit ``vault_path`` beats both.
_VAULT_KEY_ALIASES: Final = ("path", "tolaria_path")


class SearchConfig(msgspec.Struct, kw_only=True):
    """``[search]`` — how ``mesh search`` talks to ``indexed``.

    ``_explicit`` records which ``[search]`` keys were actually present in the
    raw TOML mapping. msgspec ``Struct``s expose no ``fields_set`` /
    ``__pydantic_fields_set__`` equivalent, so :func:`load_config` populates
    this by inspecting the parsed mapping directly (the same place the
    ``[core]`` vault key — ``vault_path``, plus its legacy aliases ``path``
    and ``tolaria_path`` — is already resolved) rather than the decoded
    ``Config``. It exists solely
    to answer :meth:`threshold_explicit` — the
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
    """Top-level mesh configuration parsed from ``config.toml``."""

    core: CoreConfig
    search: SearchConfig = msgspec.field(default_factory=SearchConfig)
    tasks: TasksConfig = msgspec.field(default_factory=TasksConfig)

    @property
    def agent(self) -> str | None:
        """Resolved agent identity (``$MESH_AGENT`` override applied at load)."""
        return self.core.agent


def _missing_config_message(cfg_path: Path) -> str:
    """The stderr line printed when ``cfg_path`` does not exist (agent-usability/7).

    Names the exact resolved path (not just "the config") and the one key
    ``load_config`` cannot default its way around, so a first-run agent or
    operator is pointed at both the fix (``mesh init``) and the shape it
    produces, instead of a bare exit 2. This is the message only — the
    wording is unchanged by agent-usability/5's restructuring of *how* it
    reaches the caller: :class:`ConfigMissingError` carries this string
    verbatim, read by the CLI's stderr line and the MCP structured payload's
    ``message`` field alike.
    """
    return (
        f"mesh: no config found at {cfg_path}\n"
        "run `mesh init` to create one (honours $MESH_CONFIG_PATH), "
        "or point $MESH_CONFIG_PATH at an existing config.\n"
        "required: [core].vault_path (path to your Markdown vault folder); "
        "[core].agent, [search], and [tasks] are optional and default."
    )


def resolve_config_path() -> Path:
    """Config path: ``$MESH_CONFIG_PATH`` if set, else ``~/.mesh/config.toml``."""
    override = os.environ.get(_ENV_CONFIG_PATH)
    if override:
        return Path(override).expanduser()
    return _DEFAULT_CONFIG_PATH


def load_config(path: Path | None = None) -> Config:
    """Load and validate the mesh config.

    ``path`` defaults to :func:`resolve_config_path`. A missing file raises
    :class:`ConfigMissingError` (code 2). ``$MESH_AGENT`` overrides
    ``[core].agent`` when set. The canonical spelling is ``[core].vault_path``;
    ``[core].path`` and ``[core].tolaria_path`` are accepted as legacy aliases.
    """
    cfg_path = path if path is not None else resolve_config_path()
    if not cfg_path.is_file():
        raise ConfigMissingError(cfg_path)

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
        # Accept every legacy spelling of the vault key; canonical always wins.
        for alias in _VAULT_KEY_ALIASES:
            if alias in core and "vault_path" not in core:
                core["vault_path"] = core.pop(alias)
        core.pop("path", None)
        core.pop("tolaria_path", None)
        agent_override = os.environ.get(_ENV_AGENT)
        if agent_override:
            core["agent"] = agent_override
        data = {**data, "core": core}

    config = msgspec.convert(data, Config, dec_hook=_dec_hook)
    config.search._explicit = explicit_search_keys
    return config
