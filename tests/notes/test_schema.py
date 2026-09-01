"""notes/1 — config, Note schema, and id generation.

Covers the shared primitives every later feature inherits: the msgspec Config
model (with MESH_CONFIG_PATH / MESH_AGENT overrides), the Note frontmatter
schema (unknown-key round-trip), and the deterministic hash-id generator.
"""

from __future__ import annotations

from datetime import UTC, datetime
from pathlib import Path

import pytest

from mesh.core.ids import generate_note_id, generate_task_id
from mesh.schemas.config import Config, ConfigMissingError, load_config
from mesh.schemas.note import Note

# --------------------------------------------------------------------------- #
# Config                                                                        #
# --------------------------------------------------------------------------- #


def test_config_reads_all_sections(mesh_config: Path, vault: Path) -> None:
    cfg = load_config()
    assert isinstance(cfg, Config)
    assert cfg.core.vault_path == vault
    assert cfg.core.agent == "test-agent"
    assert cfg.search.collection == "test-vault"
    assert cfg.search.hybrid is True
    assert cfg.search.threshold == pytest.approx(0.65)
    assert cfg.tasks.collections == ["test-agent", "other-agent"]


def test_search_threshold_explicit_when_set_in_toml(mesh_config: Path, vault: Path) -> None:
    # mesh_config writes an explicit [search].threshold = 0.65.
    cfg = load_config()
    assert cfg.search.threshold_explicit() is True


def test_search_threshold_not_explicit_when_absent(config_path: Path, vault: Path) -> None:
    config_path.write_text(
        f'[core]\nvault_path = "{vault}"\n\n[search]\ncollection = "v"\n', encoding="utf-8"
    )
    cfg = load_config(config_path)
    assert cfg.search.threshold_explicit() is False
    assert cfg.search.threshold == pytest.approx(0.65)  # decoded default, unaffected


def test_search_threshold_not_explicit_with_no_search_section(
    config_path: Path, vault: Path
) -> None:
    config_path.write_text(f'[core]\nvault_path = "{vault}"\n', encoding="utf-8")
    cfg = load_config(config_path)
    assert cfg.search.threshold_explicit() is False


def test_config_accepts_path_alias(config_path: Path, vault: Path) -> None:
    # Root tech.md spells the key `[core].path`; the model must accept it too.
    config_path.write_text(f'[core]\npath = "{vault}"\nagent = "aliased"\n', encoding="utf-8")
    cfg = load_config(config_path)
    assert cfg.core.vault_path == vault
    assert cfg.core.agent == "aliased"


def test_config_expands_tilde_in_path(config_path: Path) -> None:
    # `realpath` does not expand `~`; a literal `~/vault` would otherwise become
    # a `./~/vault` dir under CWD. The field validator must expand it.
    config_path.write_text('[core]\npath = "~/vault"\n', encoding="utf-8")
    cfg = load_config(config_path)
    assert cfg.core.vault_path == Path.home() / "vault"


def test_legacy_tolaria_path_key_still_loads(tmp_path: Path) -> None:
    """The pre-rename spelling must keep working — no config edit is required."""
    cfg_file = tmp_path / "config.toml"
    cfg_file.write_text(f'[core]\ntolaria_path = "{tmp_path / "vault"}"\n', encoding="utf-8")

    cfg = load_config(cfg_file)

    assert cfg.core.vault_path == tmp_path / "vault"


def test_canonical_vault_path_wins_over_legacy_aliases(tmp_path: Path) -> None:
    """An explicit canonical key beats both aliases; two spellings is not an error."""
    cfg_file = tmp_path / "config.toml"
    cfg_file.write_text(
        f"[core]\n"
        f'vault_path = "{tmp_path / "canonical"}"\n'
        f'tolaria_path = "{tmp_path / "legacy"}"\n'
        f'path = "{tmp_path / "alias"}"\n',
        encoding="utf-8",
    )

    cfg = load_config(cfg_file)

    assert cfg.core.vault_path == tmp_path / "canonical"


def test_legacy_alias_expands_tilde(tmp_path: Path) -> None:
    """`~` expansion is a property of the field, not of the spelling used to reach it."""
    cfg_file = tmp_path / "config.toml"
    cfg_file.write_text('[core]\ntolaria_path = "~/vault"\n', encoding="utf-8")

    cfg = load_config(cfg_file)

    assert cfg.core.vault_path == Path.home() / "vault"


def test_missing_config_raises_config_missing_error(tmp_path: Path) -> None:
    # agent-usability/5: load_config now raises a typed ConfigMissingError
    # (a MeshError, plain Exception) instead of SystemExit(2) — the old
    # BaseException could walk past both the CLI and MCP boundary mappers,
    # which catch Exception. The CLI still exits 2 (ConfigMissingError.code),
    # now via cli_errors() like every other domain exception.
    missing = tmp_path / "does-not-exist.toml"
    with pytest.raises(ConfigMissingError) as exc:
        load_config(missing)
    assert exc.value.code == 2
    assert exc.value.cfg_path == missing


def test_mesh_config_path_override_is_authoritative(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Point MESH_CONFIG_PATH at a nonexistent file. If the override were ignored
    # and it silently fell back to ~/.mesh/config.toml this would not raise.
    missing = tmp_path / "nope.toml"
    monkeypatch.setenv("MESH_CONFIG_PATH", str(missing))
    with pytest.raises(ConfigMissingError) as exc:
        load_config()
    assert exc.value.code == 2
    assert exc.value.cfg_path == missing


def test_mesh_config_path_isolates_from_home(mesh_config: Path, vault: Path) -> None:
    # The fixture set MESH_CONFIG_PATH; load_config must read *that* file, not
    # any real ~/.mesh/config.toml on the host running the suite.
    cfg = load_config()
    assert cfg.core.vault_path == vault


def test_mesh_agent_env_overrides_config_agent(
    mesh_config: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MESH_AGENT", "env-agent")
    cfg = load_config()
    assert cfg.core.agent == "env-agent"
    assert cfg.agent == "env-agent"


def test_config_agent_used_when_env_absent(mesh_config: Path) -> None:
    cfg = load_config()
    assert cfg.agent == "test-agent"


# --------------------------------------------------------------------------- #
# Vault-path canonicalisation                                                   #
# --------------------------------------------------------------------------- #

_VAULT_KEY_SPELLINGS = ("vault_path", "path", "tolaria_path")


def _write_core(cfg_file: Path, key: str, value: object) -> None:
    cfg_file.write_text(f'[core]\n{key} = "{value}"\n', encoding="utf-8")


@pytest.mark.parametrize("key", _VAULT_KEY_SPELLINGS)
def test_vault_path_resolves_through_a_symlink(tmp_path: Path, key: str) -> None:
    """A symlinked vault must canonicalise to its target, on every spelling.

    The daemon's watcher indexes realpaths while the scope predicates compare
    against the configured path: an unresolved symlink puts the two in different
    path spaces, and every file touched after daemon start silently falls out of
    scope (`note list` / `task list` / `status` go empty while the daemon is up).
    """
    real = tmp_path / "real-vault"
    real.mkdir()
    link = tmp_path / "linked-vault"
    link.symlink_to(real)

    cfg_file = tmp_path / "config.toml"
    _write_core(cfg_file, key, link)

    assert load_config(cfg_file).core.vault_path == real


def test_nonexistent_vault_path_still_loads(tmp_path: Path) -> None:
    """`mesh init` creates the vault lazily — resolution must not require it."""
    missing = tmp_path / "not-yet" / "vault"
    cfg_file = tmp_path / "config.toml"
    _write_core(cfg_file, "vault_path", missing)

    assert load_config(cfg_file).core.vault_path == missing


def test_nonexistent_vault_path_under_a_symlink_resolves(tmp_path: Path) -> None:
    """Canonicalisation applies to the existing prefix of a not-yet-created vault."""
    real = tmp_path / "real-parent"
    real.mkdir()
    link = tmp_path / "linked-parent"
    link.symlink_to(real)

    cfg_file = tmp_path / "config.toml"
    _write_core(cfg_file, "vault_path", link / "vault")

    assert load_config(cfg_file).core.vault_path == real / "vault"


def test_tilde_expands_before_resolution(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """`~` still expands — and the expansion is then canonicalised, not literalised."""
    real_home = tmp_path / "real-home"
    (real_home / "vault").mkdir(parents=True)
    linked_home = tmp_path / "linked-home"
    linked_home.symlink_to(real_home)
    monkeypatch.setenv("HOME", str(linked_home))

    cfg_file = tmp_path / "config.toml"
    _write_core(cfg_file, "vault_path", "~/vault")

    assert load_config(cfg_file).core.vault_path == real_home / "vault"


@pytest.mark.parametrize("key", _VAULT_KEY_SPELLINGS)
def test_relative_vault_path_becomes_absolute(tmp_path: Path, key: str) -> None:
    """One path space means absolute: a relative spelling is anchored, not kept."""
    cfg_file = tmp_path / "config.toml"
    _write_core(cfg_file, key, "some/vault")

    resolved = load_config(cfg_file).core.vault_path
    assert resolved.is_absolute()
    assert resolved == (Path.cwd() / "some" / "vault").resolve()


# --------------------------------------------------------------------------- #
# Note schema                                                                   #
# --------------------------------------------------------------------------- #


def _now() -> datetime:
    return datetime(2026, 7, 3, 12, 0, 0, tzinfo=UTC)


def test_note_carries_exact_fields() -> None:
    note = Note(
        id="n-a3f2",
        type="decision",
        title="CLID fallback",
        tags=["ndc", "flights"],
        owner="flights-agent",
        created=_now(),
        updated=_now(),
        related=["n-b1c2", "t-9xyz"],
    )
    assert note.id == "n-a3f2"
    assert note.type == "decision"
    assert note.title == "CLID fallback"
    assert note.tags == ["ndc", "flights"]
    assert note.owner == "flights-agent"
    assert note.created == _now()
    assert note.updated == _now()
    assert note.related == ["n-b1c2", "t-9xyz"]


def test_note_type_rejects_unknown_value() -> None:
    # msgspec validates on ``model_validate`` (the disk/frontmatter entry point),
    # not on direct construction — mirroring how production reads notes.
    from msgspec import ValidationError

    with pytest.raises(ValidationError):
        Note.model_validate(
            {"id": "n-a3f2", "type": "journal", "title": "x", "created": _now(), "updated": _now()}
        )


def test_note_defaults_are_sane() -> None:
    note = Note(id="n-z", title="bare", created=_now(), updated=_now())
    assert note.type == "note"
    assert note.tags == []
    assert note.related == []
    assert note.owner is None


def test_note_unknown_keys_round_trip_unchanged() -> None:
    payload = {
        "id": "n-a3f2",
        "type": "note",
        "title": "has extras",
        "tags": [],
        "owner": None,
        "created": _now(),
        "updated": _now(),
        "related": [],
        # Keys mesh does not own must survive a load/dump cycle untouched.
        "othertool_pinned": True,
        "custom_ref": "PROJ-123",
    }
    note = Note.model_validate(payload)
    dumped = note.model_dump()
    assert dumped["othertool_pinned"] is True
    assert dumped["custom_ref"] == "PROJ-123"


# --------------------------------------------------------------------------- #
# IDs                                                                           #
# --------------------------------------------------------------------------- #


def test_note_id_shape() -> None:
    nid = generate_note_id("2026-07-03T12:00:00Z", "CLID fallback")
    assert nid.startswith("n-")
    body = nid[2:]
    assert len(body) >= 4
    crockford = set("0123456789ABCDEFGHJKMNPQRSTVWXYZ")
    assert set(body) <= crockford


def test_task_id_prefix_differs() -> None:
    tid = generate_task_id("2026-07-03T12:00:00Z", "ship it")
    assert tid.startswith("t-")
    body = tid[2:]
    assert len(body) >= 4


def test_id_is_deterministic() -> None:
    a = generate_note_id("2026-07-03T12:00:00Z", "same title")
    b = generate_note_id("2026-07-03T12:00:00Z", "same title")
    assert a == b


def test_id_varies_with_inputs() -> None:
    a = generate_note_id("2026-07-03T12:00:00Z", "title one")
    b = generate_note_id("2026-07-03T12:00:00Z", "title two")
    c = generate_note_id("2026-07-03T12:00:01Z", "title one")
    assert a != b
    assert a != c


def test_note_and_task_ids_are_not_uuids_or_sequential() -> None:
    nid = generate_note_id("2026-07-03T12:00:00Z", "x")
    # Default id is `n-` + exactly 4 base-32 chars: not a 36-char hyphenated uuid.
    assert len(nid) == 6
    assert "-" not in nid[2:]
    # Consecutive creations do not produce consecutive/incrementing ids.
    crockford = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
    bodies = [generate_note_id("2026-07-03T12:00:00Z", f"note {i}")[2:] for i in range(5)]
    assert len(set(bodies)) == len(bodies)  # all distinct, no counter pattern
    values = [
        sum(crockford.index(ch) * 32**pos for pos, ch in enumerate(reversed(body)))
        for body in bodies
    ]
    deltas = {values[i + 1] - values[i] for i in range(len(values) - 1)}
    assert len(deltas) > 1  # gaps vary -> hash-derived, not a fixed sequence


def test_id_collision_extension_appends_one_char() -> None:
    created, title = "2026-07-03T12:00:00Z", "collides once"
    base = generate_note_id(created, title)

    calls = {"n": 0}

    def taken(candidate: str) -> bool:
        # Report the first (4-char) candidate as taken, then accept the next.
        calls["n"] += 1
        return calls["n"] == 1

    extended = generate_note_id(created, title, exists=taken)
    assert len(extended[2:]) == len(base[2:]) + 1
    # Extension appends deterministically: the shorter id is a prefix of it.
    assert extended.startswith(base)
