"""notes/1 — config, Note schema, and id generation.

Covers the shared primitives every later feature inherits: the pydantic Config
model (with SHARDS_CONFIG_PATH / SHARDS_AGENT overrides), the Note frontmatter
schema (unknown-key round-trip), and the deterministic hash-id generator.
"""

from __future__ import annotations

from datetime import UTC, datetime
from pathlib import Path

import pytest

from shards.core.ids import generate_note_id, generate_task_id
from shards.schemas.config import Config, load_config
from shards.schemas.note import Note

# --------------------------------------------------------------------------- #
# Config                                                                        #
# --------------------------------------------------------------------------- #


def test_config_reads_all_sections(shards_config: Path, vault: Path) -> None:
    cfg = load_config()
    assert isinstance(cfg, Config)
    assert cfg.core.tolaria_path == vault
    assert cfg.core.agent == "test-agent"
    assert cfg.search.collection == "test-vault"
    assert cfg.search.hybrid is True
    assert cfg.search.threshold == pytest.approx(0.65)
    assert cfg.tasks.collections == ["test-agent", "other-agent"]


def test_config_accepts_path_alias(config_path: Path, vault: Path) -> None:
    # Root tech.md spells the key `[core].path`; the model must accept it too.
    config_path.write_text(f'[core]\npath = "{vault}"\nagent = "aliased"\n', encoding="utf-8")
    cfg = load_config(config_path)
    assert cfg.core.tolaria_path == vault
    assert cfg.core.agent == "aliased"


def test_config_expands_tilde_in_path(config_path: Path) -> None:
    # `realpath` does not expand `~`; a literal `~/vault` would otherwise become
    # a `./~/vault` dir under CWD. The field validator must expand it.
    config_path.write_text('[core]\npath = "~/vault"\n', encoding="utf-8")
    cfg = load_config(config_path)
    assert cfg.core.tolaria_path == Path.home() / "vault"


def test_missing_config_raises_systemexit_2(tmp_path: Path) -> None:
    missing = tmp_path / "does-not-exist.toml"
    with pytest.raises(SystemExit) as exc:
        load_config(missing)
    assert exc.value.code == 2


def test_shards_config_path_override_is_authoritative(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Point SHARDS_CONFIG_PATH at a nonexistent file. If the override were ignored
    # and it silently fell back to ~/.shards/config.toml this would not raise.
    missing = tmp_path / "nope.toml"
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(missing))
    with pytest.raises(SystemExit) as exc:
        load_config()
    assert exc.value.code == 2


def test_shards_config_path_isolates_from_home(shards_config: Path, vault: Path) -> None:
    # The fixture set SHARDS_CONFIG_PATH; load_config must read *that* file, not
    # any real ~/.shards/config.toml on the host running the suite.
    cfg = load_config()
    assert cfg.core.tolaria_path == vault


def test_shards_agent_env_overrides_config_agent(
    shards_config: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SHARDS_AGENT", "env-agent")
    cfg = load_config()
    assert cfg.core.agent == "env-agent"
    assert cfg.agent == "env-agent"


def test_config_agent_used_when_env_absent(shards_config: Path) -> None:
    cfg = load_config()
    assert cfg.agent == "test-agent"


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
    from pydantic import ValidationError

    with pytest.raises(ValidationError):
        Note(
            id="n-a3f2",
            type="journal",  # type: ignore[arg-type]
            title="x",
            created=_now(),
            updated=_now(),
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
        # Keys shards does not own must survive a load/dump cycle untouched.
        "tolaria_pinned": True,
        "custom_ref": "PROJ-123",
    }
    note = Note.model_validate(payload)
    dumped = note.model_dump()
    assert dumped["tolaria_pinned"] is True
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
