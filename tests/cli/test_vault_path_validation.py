"""A misconfigured vault root reads as a config error, not an internal one.

Exit codes are a contract (`.spec/tech.md`: 1 error · 2 validation). A
`vault_path` pointing at a regular file used to report a healthy empty vault at
exit 0, then fail the first write with a bare `ENOTDIR` at exit 1 — an
internal-error code for a plainly bad config value.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from mesh.schemas.config import load_config


def _config_at(config_path: Path, vault: Path) -> None:
    config_path.write_text(
        f'[core]\nvault_path = "{vault}"\nagent = "a"\n',
        encoding="utf-8",
    )


def test_a_file_as_vault_root_is_a_validation_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    a_file = tmp_path / "notes.md"
    a_file.write_text("i am a file", encoding="utf-8")
    config_path = tmp_path / "config.toml"
    _config_at(config_path, a_file)
    monkeypatch.setenv("MESH_CONFIG_PATH", str(config_path))

    with pytest.raises(Exception, match="not a directory"):
        load_config()


def test_a_vault_that_does_not_exist_yet_still_loads(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Lazy creation is `init`'s documented behaviour — do not break it."""
    missing = tmp_path / "not-created-yet"
    config_path = tmp_path / "config.toml"
    _config_at(config_path, missing)
    monkeypatch.setenv("MESH_CONFIG_PATH", str(config_path))

    assert load_config().core.vault_path == missing.resolve()


def test_a_symlinked_vault_resolves_to_its_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The canonicalisation the daemon's scope predicates depend on."""
    real = tmp_path / "real-vault"
    real.mkdir()
    link = tmp_path / "link-vault"
    link.symlink_to(real)
    config_path = tmp_path / "config.toml"
    _config_at(config_path, link)
    monkeypatch.setenv("MESH_CONFIG_PATH", str(config_path))

    assert load_config().core.vault_path == real.resolve()
