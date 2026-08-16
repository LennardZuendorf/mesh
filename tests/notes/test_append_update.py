"""notes/3 — append / update: section append, tag mutation, locked concurrent edits.

Exercises R2 (Amend): ``shards note append`` (with ``--section`` / ``--timestamp``)
and ``shards note update`` (tag delta/replace, ``--type`` folder move). Every write
goes through :func:`shards.storage.files.atomic_write`; the per-entity
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

import shards.core.notes as notes_core
import shards.storage.locks as locks_mod
from shards.cli.__main__ import app
from shards.core.notes import (
    AmbiguousSlugError,
    NoteNotFoundError,
    append_note,
    apply_tag_spec,
    update_note,
)
from shards.schemas.config import Config, load_config
from shards.schemas.note import Note
from shards.storage.files import note_folder

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
    """Write a shards note straight to disk in the folder matching its type."""
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
def cfg(shards_config: Path) -> Config:
    return load_config()


# --------------------------------------------------------------------------- #
# apply_tag_spec — pure tag algebra                                             #
# --------------------------------------------------------------------------- #


def test_apply_tag_spec_delta_adds_and_removes() -> None:
    assert apply_tag_spec(["a", "y"], "+x,-y") == ["a", "x"]


def test_apply_tag_spec_delta_is_idempotent() -> None:
    # Adding an existing tag / removing an absent one is a no-op, no dupes.
    assert apply_tag_spec(["a"], "+a,-z") == ["a"]


def test_apply_tag_spec_delta_remove_absent_is_noop() -> None:
    assert apply_tag_spec(["a"], "-nope") == ["a"]


def test_apply_tag_spec_bare_list_is_additive_not_replace() -> None:
    """agent-usability/3 — the silent-wipe regression, locked. A bare comma list
    keeps every existing tag and adds the named ones; it must never replace."""
    assert apply_tag_spec(["infra", "urgent", "q3"], "urgent") == [
        "infra",
        "urgent",
        "q3",
    ]


def test_apply_tag_spec_bare_list_add_is_idempotent_no_dupes() -> None:
    assert apply_tag_spec(["a", "b"], "a,c,c") == ["a", "b", "c"]


def test_apply_tag_spec_explicit_replace_replaces_whole_list() -> None:
    """Only the leading ``=`` opt-in replaces — a bare list never does."""
    assert apply_tag_spec(["a", "b"], "=x,y") == ["x", "y"]


def test_apply_tag_spec_explicit_replace_dedupes_preserving_order() -> None:
    assert apply_tag_spec(["a"], "=x,x,y") == ["x", "y"]


def test_apply_tag_spec_explicit_replace_bare_equals_clears_all_tags() -> None:
    assert apply_tag_spec(["a", "b"], "=") == []


def test_apply_tag_spec_only_explicit_replace_path_replaces() -> None:
    """Neither the additive bare-list path nor the delta path ever replaces —
    replacement is reachable only through the leading ``=``."""
    existing = ["a", "b"]
    assert apply_tag_spec(existing, "c") != ["c"]  # additive, not replace
    assert apply_tag_spec(existing, "+c") != ["c"]  # delta, not replace
    assert apply_tag_spec(existing, "=c") == ["c"]  # explicit replace only


# --------------------------------------------------------------------------- #
# apply_tag_spec — mixed-prefix boundary (fix round 1)                         #
# --------------------------------------------------------------------------- #
#
# A partially-prefixed spec (some tokens start with +/-, some don't, no
# leading "=") used to fall through to the additive branch and write a
# literal "+x" tag into the vault as permanent garbage. It now raises
# ValueError instead of guessing. Four boundary cases must NOT raise (all-
# delta, all-additive, explicit-replace, and a tag merely containing +/- mid-
# string), and two mixed forms must raise.


def test_apply_tag_spec_all_prefixed_is_still_delta_not_rejected() -> None:
    assert apply_tag_spec(["a", "y"], "+x,-y") == ["a", "x"]


def test_apply_tag_spec_all_unprefixed_is_still_additive_not_rejected() -> None:
    assert apply_tag_spec(["a"], "x,y") == ["a", "x", "y"]


def test_apply_tag_spec_leading_equals_is_still_explicit_replace_not_rejected() -> None:
    assert apply_tag_spec(["a", "b"], "=x,y") == ["x", "y"]


def test_apply_tag_spec_mid_string_plus_minus_is_not_a_prefix_not_rejected() -> None:
    """Only a token's *first* character counts as a prefix — a legitimate tag
    name containing '+'/'-' anywhere else is never mistaken for one."""
    assert apply_tag_spec(["a"], "c++") == ["a", "c++"]
    assert apply_tag_spec(["a"], "sci-fi") == ["a", "sci-fi"]
    # Alongside an unprefixed token, both stay additive — no mix detected,
    # because neither token's first character is +/-.
    assert apply_tag_spec(["a"], "c++,sci-fi") == ["a", "c++", "sci-fi"]


def test_apply_tag_spec_mixed_leading_plus_then_bare_raises() -> None:
    with pytest.raises(ValueError, match="ambiguous tag spec"):
        apply_tag_spec(["a", "b"], "+x,y")


def test_apply_tag_spec_mixed_bare_then_leading_minus_raises() -> None:
    with pytest.raises(ValueError, match="ambiguous tag spec"):
        apply_tag_spec(["a", "b"], "x,-y")


def test_apply_tag_spec_mixed_error_names_offending_spec() -> None:
    with pytest.raises(ValueError, match=r"\+x,y"):
        apply_tag_spec(["a", "b"], "+x,y")


def test_apply_tag_spec_mixed_spec_does_not_mutate_existing() -> None:
    """The rejection happens before any list is built — a caller retrying with
    a valid spec sees the original, untouched list."""
    existing = ["a", "b"]
    with pytest.raises(ValueError):
        apply_tag_spec(existing, "+x,y")
    assert existing == ["a", "b"]


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


def _seed_malformed(vault: Path, note_id: str = "n-bad") -> Path:
    """Write an ``n-`` id file under ``notes/`` whose frontmatter is unparseable YAML."""
    path = vault / "notes" / f"{note_id}.md"
    path.write_text("---\ntitle: [unclosed\n---\nbody\n", encoding="utf-8")
    return path


def test_append_malformed_yaml_raises_not_found(cfg: Config, vault: Path) -> None:
    """A resolved-but-unreadable note's content read maps to NoteNotFoundError.

    ``_resolve_path`` matches on filename stem first, so a malformed target
    still resolves; the content read (routed through ``read_post``) is what
    fails here, and it maps to the same not-found contract as ``get_note``.
    """
    _seed_malformed(vault, "n-bad")
    with pytest.raises(NoteNotFoundError):
        append_note(cfg, "n-bad", "x")


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


# --------------------------------------------------------------------------- #
# team-awareness/8 — the stamp names the editor, never the note's owner         #
# --------------------------------------------------------------------------- #


def _write_agent_config(tmp_path: Path, vault: Path, agent: str | None) -> Path:
    """Write a standalone ``config.toml`` (distinct from the ``shards_config``
    fixture's) identifying as ``agent`` — or with no ``[core].agent`` at all when
    ``agent`` is ``None`` — so a test can hold two ``Config`` objects pointed at
    the same vault under two different identities."""
    lines = ["[core]", f'tolaria_path = "{vault}"']
    if agent is not None:
        lines.append(f'agent = "{agent}"')
    path = tmp_path / f"{agent or 'noagent'}.toml"
    path.write_text("\n".join([*lines, ""]), encoding="utf-8")
    return path


def test_append_timestamp_names_the_editor_not_the_owner(
    vault: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The observed team-sim bug (R8): tolaria-agent appends to a note owned by
    flights-agent; the stamp must name the editor (tolaria-agent), never the
    note's ``owner``, and the ISO token stays the first field on the line."""
    path = _seed_note(vault, extra={"owner": "flights-agent"})
    cfg_file = _write_agent_config(tmp_path, vault, "tolaria-agent")
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(cfg_file))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    editor_cfg = load_config()

    append_note(editor_cfg, "n-seed", "appended by the editor", timestamp=True)
    content = _reload(path).content
    stamp_line = next(line for line in content.splitlines() if _ISO_UTC.search(line))
    match = _ISO_UTC.search(stamp_line)
    assert match is not None
    assert match.start() == 0  # the ISO token is the first field on the line
    assert stamp_line == f"{match.group(0)} — tolaria-agent"
    assert "flights-agent" not in stamp_line  # names the editor, not the owner


def test_append_timestamp_unset_identity_is_bare_iso(
    vault: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """No ``[core].agent`` and no ``$SHARDS_AGENT`` degrades to a bare ISO line —
    no stray trailing separator, no crash."""
    path = _seed_note(vault)
    cfg_file = _write_agent_config(tmp_path, vault, None)
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(cfg_file))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    noagent_cfg = load_config()
    assert noagent_cfg.agent is None

    append_note(noagent_cfg, "n-seed", "anonymous append", timestamp=True)
    content = _reload(path).content
    stamp_line = next(line for line in content.splitlines() if _ISO_UTC.search(line))
    match = _ISO_UTC.search(stamp_line)
    assert match is not None
    assert stamp_line == match.group(0)  # bare ISO, no " — ", no placeholder


def test_append_timestamp_adds_no_frontmatter_key(cfg: Config, vault: Path) -> None:
    """R8: the stamp is prose in the body, never a new frontmatter key — the
    frontmatter is unchanged apart from ``updated``."""
    path = _seed_note(vault)
    before = dict(_reload(path).metadata)
    append_note(cfg, "n-seed", "with clock", timestamp=True)
    after = dict(_reload(path).metadata)
    assert set(after) == set(before)  # no key added or removed
    before.pop("updated")
    after_updated = after.pop("updated")
    assert after == before  # every other field is byte-identical
    assert after_updated != _OLD


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

    os.environ["SHARDS_CONFIG_PATH"] = config_path
    from shards.core.notes import append_note as _append
    from shards.schemas.config import load_config as _load

    child_cfg = _load()
    barrier.wait()  # type: ignore[attr-defined]
    _append(child_cfg, note_id, text)


def test_concurrent_appends_all_land(shards_config: Path, vault: Path) -> None:
    _seed_note(vault)
    n = 6
    ctx = mp.get_context("fork")
    barrier = ctx.Barrier(n)
    procs = [
        ctx.Process(
            target=_append_worker,
            args=(str(shards_config), "n-seed", f"MARKER-{i}", barrier),
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


def test_update_tags_delta_remove_absent_is_noop(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, tags=["ndc", "stale"])
    update_note(cfg, "n-seed", tags="-nope")
    assert _reload(path).metadata["tags"] == ["ndc", "stale"]


def test_update_tags_bare_list_is_additive(cfg: Config, vault: Path) -> None:
    """agent-usability/3 — the silent-wipe regression, locked end to end through
    ``update_note``: a note tagged ["infra", "urgent", "q3"] updated with
    tags="urgent" retains all three."""
    path = _seed_note(vault, tags=["infra", "urgent", "q3"])
    note = update_note(cfg, "n-seed", tags="urgent")
    meta = _reload(path).metadata
    assert meta["tags"] == ["infra", "urgent", "q3"]
    assert note.tags == ["infra", "urgent", "q3"]


def test_update_tags_bare_list_adds_new_tag_without_dropping_others(
    cfg: Config, vault: Path
) -> None:
    path = _seed_note(vault, tags=["ndc", "stale"])
    update_note(cfg, "n-seed", tags="flights")
    assert _reload(path).metadata["tags"] == ["ndc", "stale", "flights"]


def test_update_tags_explicit_replace(cfg: Config, vault: Path) -> None:
    path = _seed_note(vault, tags=["ndc", "stale"])
    update_note(cfg, "n-seed", tags="=x,y")
    assert _reload(path).metadata["tags"] == ["x", "y"]


def test_update_tags_roundtrips_unknown_keys(cfg: Config, vault: Path) -> None:
    """Root tech.md Invariant 3 — a tag mutation on the update path must not
    disturb foreign frontmatter keys the msgspec ``_Frontmatter`` stash keeps."""
    path = _seed_note(
        vault,
        tags=["infra", "urgent", "q3"],
        extra={"tolaria_pinned": True, "custom_ref": "PROJ-1"},
    )
    update_note(cfg, "n-seed", tags="urgent")
    meta = _reload(path).metadata
    assert meta["tolaria_pinned"] is True
    assert meta["custom_ref"] == "PROJ-1"
    assert meta["tags"] == ["infra", "urgent", "q3"]


def test_update_tags_mixed_spec_raises_and_writes_nothing(cfg: Config, vault: Path) -> None:
    """End to end through update_note: a mixed spec is rejected before the
    write, not written as a garbage literal tag."""
    path = _seed_note(vault, tags=["ndc", "stale"])
    before = path.read_text(encoding="utf-8")
    with pytest.raises(ValueError, match="ambiguous tag spec"):
        update_note(cfg, "n-seed", tags="+x,y")
    assert path.read_text(encoding="utf-8") == before  # untouched


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


def test_update_malformed_yaml_raises_not_found(cfg: Config, vault: Path) -> None:
    """A resolved-but-unreadable note's content read maps to NoteNotFoundError."""
    _seed_malformed(vault, "n-bad")
    with pytest.raises(NoteNotFoundError):
        update_note(cfg, "n-bad", tags="+x")


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
# Finding #4 — amend verbs refuse foreign (non-shards) files                     #
# --------------------------------------------------------------------------- #


def _seed_foreign(vault: Path, name: str, title: str) -> Path:
    """Write a coexisting Tolaria file with no shards ``n-`` id (non-``n-`` stem)."""
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
# CLI — shards note append / update                                             #
# --------------------------------------------------------------------------- #


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    from typer.testing import CliRunner

    return CliRunner().invoke(app, args)


def test_cli_append_success(shards_config: Path, vault: Path) -> None:
    path = _seed_note(vault)
    result = _invoke(["note", "append", "n-seed", "cli text"])
    assert result.exit_code == 0, result.output
    assert "cli text" in _reload(path).content


def test_cli_append_not_found_exits_3(shards_config: Path, vault: Path) -> None:
    _seed_note(vault)
    result = _invoke(["note", "append", "n-missing", "x"])
    assert result.exit_code == 3


def test_cli_append_section_and_timestamp(shards_config: Path, vault: Path) -> None:
    path = _seed_note(vault)
    result = _invoke(
        ["note", "append", "n-seed", "logged", "--section", "Follow-ups", "--timestamp"]
    )
    assert result.exit_code == 0, result.output
    content = _reload(path).content
    assert "## Follow-ups" in content
    assert _ISO_UTC.search(content) is not None
    assert "logged" in content


def test_cli_update_tags(shards_config: Path, vault: Path) -> None:
    path = _seed_note(vault, tags=["ndc", "stale"])
    result = _invoke(["note", "update", "n-seed", "--tags", "+x,-stale"])
    assert result.exit_code == 0, result.output
    assert _reload(path).metadata["tags"] == ["ndc", "x"]


def test_cli_update_type_moves_file(shards_config: Path, vault: Path) -> None:
    old_path = _seed_note(vault, note_type="note")
    result = _invoke(["note", "update", "n-seed", "--type", "decision"])
    assert result.exit_code == 0, result.output
    assert (vault / "notes" / "decisions" / "n-seed.md").exists()
    assert not old_path.exists()


def test_cli_update_ambiguous_slug_exits_2(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-aaaa", title="Dup Title")
    _seed_note(vault, note_id="n-bbbb", title="Dup Title", note_type="log")
    result = _invoke(["note", "update", "dup-title", "--tags", "+x"])
    assert result.exit_code == 2


def test_cli_append_quiet_emits_id_only(shards_config: Path, vault: Path) -> None:
    _seed_note(vault)
    result = _invoke(["--quiet", "note", "append", "n-seed", "x"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "n-seed"
