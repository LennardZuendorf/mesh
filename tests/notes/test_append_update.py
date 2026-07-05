"""notes/3 — append / update: section append, tag mutation, locked concurrent edits.

Exercises R2 (Amend): ``brain note append`` (with ``--section`` / ``--timestamp``)
and ``brain note update`` (tag delta/replace, ``--type`` folder move). Every write
goes through :func:`brain.storage.files.atomic_write`; the per-entity
``notes/.locks/<id>.lock`` (``O_EXCL``) is held for the whole read-modify-write
cycle so concurrent appends serialize without lost updates.
"""

from __future__ import annotations

import multiprocessing as mp
import re
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest

import brain.core.notes as notes_core
import brain.storage.locks as locks_mod
from brain.cli.__main__ import app
from brain.core.notes import (
    AmbiguousSlugError,
    NoteNotFoundError,
    append_note,
    apply_tag_spec,
    update_note,
)
from brain.schemas.config import Config, load_config
from brain.schemas.note import Note
from brain.storage.files import note_folder

_ISO_UTC = re.compile(r"\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\b")
_OLD = datetime(2026, 1, 1, 9, 0, 0, tzinfo=UTC)


def _seed_note(
    vault: Path,
    *,
    note_id: str = "n-seed",
    note_type: str = "note",
    title: str = "Seed Note",
    tags: list[str] | None = None,
    body: str = "Body line.",
    created: datetime = _OLD,
    updated: datetime = _OLD,
    extra: dict[str, object] | None = None,
) -> Path:
    """Write a brain note straight to disk in the folder matching its type."""
    meta: dict[str, object] = {
        "id": note_id,
        "type": note_type,
        "title": title,
        "tags": list(tags or []),
        "owner": "seed-agent",
        "created": created,
        "updated": updated,
        "related": [],
    }
    if extra:
        meta.update(extra)
    folder = note_folder(note_type, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{note_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


def _reload(path: Path) -> frontmatter.Post:
    return frontmatter.loads(path.read_text(encoding="utf-8"))


@pytest.fixture
def cfg(brain_config: Path) -> Config:
    return load_config()


# --------------------------------------------------------------------------- #
# apply_tag_spec — pure tag algebra                                             #
# --------------------------------------------------------------------------- #


def test_apply_tag_spec_delta_adds_and_removes() -> None:
    assert apply_tag_spec(["a", "y"], "+x,-y") == ["a", "x"]


def test_apply_tag_spec_delta_is_idempotent() -> None:
    # Adding an existing tag / removing an absent one is a no-op, no dupes.
    assert apply_tag_spec(["a"], "+a,-z") == ["a"]


def test_apply_tag_spec_replace_replaces_whole_list() -> None:
    assert apply_tag_spec(["a", "b"], "x,y") == ["x", "y"]


def test_apply_tag_spec_replace_dedupes_preserving_order() -> None:
    assert apply_tag_spec(["a"], "x,x,y") == ["x", "y"]


# --------------------------------------------------------------------------- #
# append_note (core)                                                            #
# --------------------------------------------------------------------------- #


def test_append_adds_text_and_bumps_updated(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault)
    note = append_note(cfg, "n-seed", "Confirmed J/C")
    reloaded = _reload(path)
    assert "Confirmed J/C" in reloaded.content
    assert "Body line." in reloaded.content  # original body preserved
    assert reloaded.metadata["updated"] > _OLD  # bumped
    assert reloaded.metadata["created"] == _OLD  # created untouched
    assert note.updated > _OLD
    assert note.id == "n-seed"


def test_append_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_note(vault)
    with pytest.raises(NoteNotFoundError):
        append_note(cfg, "n-missing", "x")


def test_append_section_appends_under_existing_heading(cfg: Config, vault: Path) -> None:
    _seed_note(vault, body="Intro.\n\n## Follow-ups\n\nfirst follow-up.")
    path = notes_core._resolve_path(cfg, "n-seed")
    append_note(cfg, "n-seed", "second follow-up", section="Follow-ups")
    content = _reload(path).content
    # Exactly one Follow-ups heading; both items live under it, in order.
    assert content.count("## Follow-ups") == 1
    fu = content.index("## Follow-ups")
    first = content.index("first follow-up.")
    second = content.index("second follow-up")
    assert fu < first < second


def test_append_section_creates_heading_when_absent(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, body="Intro paragraph.")
    append_note(cfg, "n-seed", "new note", section="Follow-ups")
    content = _reload(path).content
    assert "## Follow-ups" in content
    # Heading is created at end-of-body, before the appended text.
    assert content.index("Intro paragraph.") < content.index("## Follow-ups")
    assert content.index("## Follow-ups") < content.index("new note")


def test_append_timestamp_prepends_iso_line(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault)
    append_note(cfg, "n-seed", "with clock", timestamp=True)
    content = _reload(path).content
    match = _ISO_UTC.search(content)
    assert match is not None
    # The timestamp line precedes the appended text.
    assert match.start() < content.index("with clock")


def test_append_roundtrips_unknown_frontmatter_keys(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, extra={"tolaria_pinned": True, "custom_ref": "PROJ-1"})
    append_note(cfg, "n-seed", "x")
    meta = _reload(path).metadata
    assert meta["tolaria_pinned"] is True
    assert meta["custom_ref"] == "PROJ-1"


def test_append_recomputes_related_dropping_stale_ids(cfg: Config, vault: Path) -> None:
    # ``related`` is a pure function of the *full* body, recomputed on every
    # append. Seed a note whose body already links ``[[n-keep]]`` but whose
    # stored ``related`` also carries a stale id (``n-ghost``) no longer backed by
    # any wikilink. After appending a block with a new link, ``related`` reflects
    # only the ids present in the body — the stale one is dropped, the new one
    # added, in first-seen order.
    path = _seed_note(
        vault,
        body="Intro links [[n-keep]].",
        extra={"related": ["n-keep", "n-ghost"]},
    )
    append_note(cfg, "n-seed", "now also see [[n-new]]")
    assert _reload(path).metadata["related"] == ["n-keep", "n-new"]


def test_append_uses_atomic_write(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _seed_note(vault)
    calls: list[Path] = []
    real = notes_core.atomic_write

    def spy(path: Path, content: str) -> None:
        calls.append(path)
        real(path, content)

    monkeypatch.setattr(notes_core, "atomic_write", spy)
    append_note(cfg, "n-seed", "x")
    assert calls, "append_note must route writes through storage.atomic_write"


def test_append_acquires_entity_lock(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen: list[Path] = []
    real = locks_mod.acquire

    def spy(lock_path: Path):  # type: ignore[no-untyped-def]
        seen.append(lock_path)
        return real(lock_path)

    _seed_note(vault)
    monkeypatch.setattr(locks_mod, "acquire", spy)
    append_note(cfg, "n-seed", "x")
    assert seen == [vault / "notes" / ".locks" / "n-seed.lock"]


# --------------------------------------------------------------------------- #
# Concurrency — O_EXCL lock serializes appends, no lost updates                 #
# --------------------------------------------------------------------------- #


def _append_worker(config_path: str, note_id: str, text: str, barrier: object) -> None:
    """Child-process body: load config, wait on the barrier, append once."""
    import os

    os.environ["BRAIN_CONFIG_PATH"] = config_path
    from brain.core.notes import append_note as _append
    from brain.schemas.config import load_config as _load

    child_cfg = _load()
    barrier.wait()  # type: ignore[attr-defined]
    _append(child_cfg, note_id, text)


def test_concurrent_appends_all_land(brain_config: Path, vault: Path) -> None:
    _seed_note(vault)
    n = 6
    ctx = mp.get_context("fork")
    barrier = ctx.Barrier(n)
    procs = [
        ctx.Process(
            target=_append_worker,
            args=(str(brain_config), "n-seed", f"MARKER-{i}", barrier),
        )
        for i in range(n)
    ]
    for p in procs:
        p.start()
    for p in procs:
        p.join(timeout=30)
    for p in procs:
        assert p.exitcode == 0, f"worker exited with {p.exitcode}"

    post = _reload(vault / "notes" / "n-seed.md")
    for i in range(n):
        assert f"MARKER-{i}" in post.content, f"lost update: MARKER-{i} missing"
    # File is still a single valid note with the original id.
    note = Note.model_validate(post.metadata)
    assert note.id == "n-seed"


# --------------------------------------------------------------------------- #
# update_note (core)                                                            #
# --------------------------------------------------------------------------- #


def test_update_tags_delta_add_remove(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, tags=["ndc", "stale"])
    note = update_note(cfg, "n-seed", tags="+flights,-stale")
    meta = _reload(path).metadata
    assert meta["tags"] == ["ndc", "flights"]
    assert note.tags == ["ndc", "flights"]
    assert meta["updated"] > _OLD


def test_update_tags_replace(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, tags=["ndc", "stale"])
    update_note(cfg, "n-seed", tags="x,y")
    assert _reload(path).metadata["tags"] == ["x", "y"]


def test_update_type_moves_file(cfg: Config, vault: Path) -> None:
    old_path = _seed_note(vault, note_type="note", body="Decision body.")
    assert old_path == vault / "notes" / "n-seed.md"
    note = update_note(cfg, "n-seed", new_type="decision")
    new_path = vault / "notes" / "decisions" / "n-seed.md"
    assert new_path.exists()
    assert not old_path.exists()  # old path no longer exists
    reloaded = _reload(new_path)
    assert reloaded.metadata["type"] == "decision"
    assert "Decision body." in reloaded.content  # body preserved across move
    assert note.type == "decision"


def test_update_type_unchanged_is_in_place(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, note_type="note")
    update_note(cfg, "n-seed", new_type="note")
    assert path.exists()


def test_update_invalid_type_raises(cfg: Config, vault: Path) -> None:
    _seed_note(vault)
    with pytest.raises(ValueError):
        update_note(cfg, "n-seed", new_type="journal")


def test_update_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_note(vault)
    with pytest.raises(NoteNotFoundError):
        update_note(cfg, "n-missing", tags="+x")


def test_update_uses_atomic_write(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _seed_note(vault)
    calls: list[Path] = []
    real = notes_core.atomic_write

    def spy(path: Path, content: str) -> None:
        calls.append(path)
        real(path, content)

    monkeypatch.setattr(notes_core, "atomic_write", spy)
    update_note(cfg, "n-seed", tags="+x")
    assert calls


# --------------------------------------------------------------------------- #
# Slug resolution                                                              #
# --------------------------------------------------------------------------- #


def test_append_resolves_by_slug(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, note_id="n-abcd", title="CLID Fallback")
    append_note(cfg, "clid-fallback", "slugged")
    assert "slugged" in _reload(path).content


def test_ambiguous_slug_raises(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-aaaa", title="Same Title")
    _seed_note(vault, note_id="n-bbbb", title="Same Title", note_type="decision")
    with pytest.raises(AmbiguousSlugError):
        append_note(cfg, "same-title", "x")


# --------------------------------------------------------------------------- #
# Finding #4 — amend verbs refuse foreign (non-brain) files                     #
# --------------------------------------------------------------------------- #


def _seed_foreign(vault: Path, name: str, title: str) -> Path:
    """Write a coexisting Tolaria file with no brain ``n-`` id (non-``n-`` stem)."""
    path = vault / "notes" / f"{name}.md"
    post = frontmatter.Post("Foreign body.", title=title, tags=["x"])
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def test_append_refuses_foreign_file(cfg: Config, vault: Path) -> None:
    foreign = _seed_foreign(vault, "tolaria-foo", "Tolaria Foo")
    before = foreign.read_text(encoding="utf-8")
    with pytest.raises(NoteNotFoundError):
        append_note(cfg, "tolaria-foo", "x")  # by stem
    with pytest.raises(NoteNotFoundError):
        append_note(cfg, "tolaria-foo", "x")  # by slug of title
    assert foreign.read_text(encoding="utf-8") == before  # untouched


def test_update_refuses_foreign_file(cfg: Config, vault: Path) -> None:
    foreign = _seed_foreign(vault, "tolaria-bar", "Tolaria Bar")
    before = foreign.read_text(encoding="utf-8")
    with pytest.raises(NoteNotFoundError):
        update_note(cfg, "tolaria-bar", tags="+x")
    assert foreign.read_text(encoding="utf-8") == before  # untouched


# --------------------------------------------------------------------------- #
# CLI — brain note append / update                                             #
# --------------------------------------------------------------------------- #


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    from typer.testing import CliRunner

    return CliRunner().invoke(app, args)


def test_cli_append_success(brain_config: Path, vault: Path) -> None:
    path = _seed_note(vault)
    result = _invoke(["note", "append", "n-seed", "cli text"])
    assert result.exit_code == 0, result.output
    assert "cli text" in _reload(path).content


def test_cli_append_not_found_exits_3(brain_config: Path, vault: Path) -> None:
    _seed_note(vault)
    result = _invoke(["note", "append", "n-missing", "x"])
    assert result.exit_code == 3


def test_cli_append_section_and_timestamp(brain_config: Path, vault: Path) -> None:
    path = _seed_note(vault)
    result = _invoke(
        ["note", "append", "n-seed", "logged", "--section", "Follow-ups", "--timestamp"]
    )
    assert result.exit_code == 0, result.output
    content = _reload(path).content
    assert "## Follow-ups" in content
    assert _ISO_UTC.search(content) is not None
    assert "logged" in content


def test_cli_update_tags(brain_config: Path, vault: Path) -> None:
    path = _seed_note(vault, tags=["ndc", "stale"])
    result = _invoke(["note", "update", "n-seed", "--tags", "+x,-stale"])
    assert result.exit_code == 0, result.output
    assert _reload(path).metadata["tags"] == ["ndc", "x"]


def test_cli_update_type_moves_file(brain_config: Path, vault: Path) -> None:
    old_path = _seed_note(vault, note_type="note")
    result = _invoke(["note", "update", "n-seed", "--type", "decision"])
    assert result.exit_code == 0, result.output
    assert (vault / "notes" / "decisions" / "n-seed.md").exists()
    assert not old_path.exists()


def test_cli_update_ambiguous_slug_exits_2(brain_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-aaaa", title="Dup Title")
    _seed_note(vault, note_id="n-bbbb", title="Dup Title", note_type="log")
    result = _invoke(["note", "update", "dup-title", "--tags", "+x"])
    assert result.exit_code == 2


def test_cli_append_quiet_emits_id_only(brain_config: Path, vault: Path) -> None:
    _seed_note(vault)
    result = _invoke(["--quiet", "note", "append", "n-seed", "x"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "n-seed"
