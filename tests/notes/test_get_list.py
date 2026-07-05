"""notes/4 — get / list: preview modes, slug resolve, filters/sort.

Exercises R3 (Read / list): :func:`shards.core.notes.get_note` and
:func:`shards.core.notes.list_notes` plus the ``shards note get`` / ``shards note
list`` CLI surface. ``get`` yields frontmatter + a 200-char body preview
(``--full`` / ``--meta-only`` / ``--related`` switch the shape); ``list`` only
surfaces files carrying a valid shards ``n-`` id (Tolaria files are skipped) and
supports tag/owner/type/``--since`` filters with ``--sort`` and ``--limit``.

Ordering tests seed *distinct* timestamps: Python's sort is stable, so ties fall
back to filesystem ``rglob`` order and would be non-deterministic.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta
from pathlib import Path

import frontmatter
import pytest

from shards.core.notes import (
    AmbiguousSlugError,
    NoteNotFoundError,
    NoteView,
    get_note,
    list_notes,
    resolve_slug,
)
from shards.schemas.config import Config, load_config
from shards.storage.files import note_folder


def _now() -> datetime:
    return datetime.now(UTC)


def _seed_note(
    vault: Path,
    *,
    note_id: str,
    note_type: str = "note",
    title: str = "A Note",
    tags: list[str] | None = None,
    owner: str = "seed-agent",
    body: str = "Body line.",
    created: datetime | None = None,
    updated: datetime | None = None,
    related: list[str] | None = None,
    extra: dict[str, object] | None = None,
) -> Path:
    """Write a shards note straight to disk in the folder matching its type."""
    when = _now()
    meta: dict[str, object] = {
        "id": note_id,
        "type": note_type,
        "title": title,
        "tags": list(tags or []),
        "owner": owner,
        "created": created or when,
        "updated": updated or when,
        "related": list(related or []),
    }
    if extra:
        meta.update(extra)
    folder = note_folder(note_type, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{note_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


def _seed_tolaria(vault: Path, name: str, meta: dict[str, object] | None = None) -> Path:
    """Write a non-shards Markdown file (no valid ``n-`` id) under ``notes/``."""
    path = vault / "notes" / f"{name}.md"
    post = frontmatter.Post("Tolaria content.", **(meta or {}))
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    from typer.testing import CliRunner

    from shards.cli.__main__ import app

    return CliRunner().invoke(app, args)


# --------------------------------------------------------------------------- #
# resolve_slug (core)                                                          #
# --------------------------------------------------------------------------- #


def test_resolve_slug_by_id(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-abcd", title="CLID Fallback")
    assert resolve_slug(cfg, "n-abcd") == "n-abcd"


def test_resolve_slug_by_slug(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-abcd", title="CLID Fallback")
    assert resolve_slug(cfg, "clid-fallback") == "n-abcd"


def test_resolve_slug_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-abcd", title="CLID Fallback")
    with pytest.raises(NoteNotFoundError):
        resolve_slug(cfg, "nope")


def test_resolve_slug_ambiguous_lists_ids(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-aaaa", title="Same Title")
    _seed_note(vault, note_id="n-bbbb", title="Same Title", note_type="decision")
    with pytest.raises(AmbiguousSlugError) as exc:
        resolve_slug(cfg, "same-title")
    assert exc.value.ids == ["n-aaaa", "n-bbbb"]
    # The message carries the ids so the CLI can surface them (exit 2).
    assert "n-aaaa" in str(exc.value)
    assert "n-bbbb" in str(exc.value)


# --------------------------------------------------------------------------- #
# get_note (core)                                                              #
# --------------------------------------------------------------------------- #


def test_get_note_returns_view_with_body(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-seed", title="Seed", body="Hello world.")
    view = get_note(cfg, "n-seed")
    assert isinstance(view, NoteView)
    assert view.note.id == "n-seed"
    assert view.note.title == "Seed"
    assert view.body == "Hello world."


def test_get_note_resolves_by_slug(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-abcd", title="CLID Fallback", body="B")
    assert get_note(cfg, "clid-fallback").note.id == "n-abcd"


def test_get_note_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-seed")
    with pytest.raises(NoteNotFoundError):
        get_note(cfg, "n-missing")


def test_get_note_refuses_foreign_file(cfg: Config, vault: Path) -> None:
    """Finding #1: a coexisting Tolaria file is never resolved by get."""
    foreign = _seed_tolaria(vault, "tolaria-foo", {"title": "Tolaria Foo"})
    with pytest.raises(NoteNotFoundError):
        get_note(cfg, "tolaria-foo")  # by stem
    with pytest.raises(NoteNotFoundError):
        get_note(cfg, "tolaria-foo")  # by slug of the title
    assert foreign.exists()  # untouched


# --------------------------------------------------------------------------- #
# list_notes (core) — shards-id gate + filters + sort/limit                     #
# --------------------------------------------------------------------------- #


def test_list_skips_files_without_shards_id(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-real", title="Real")
    _seed_tolaria(vault, "tolaria-plain")  # no frontmatter at all
    _seed_tolaria(vault, "tolaria-titled", {"title": "Tolaria", "tags": ["x"]})  # no id
    _seed_tolaria(vault, "foreign-id", {"id": "x-123", "title": "Foreign"})  # wrong prefix
    ids = [v.note.id for v in list_notes(cfg)]
    assert ids == ["n-real"]


def test_list_tags_and_semantics(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-both", tags=["ndc", "flights"], updated=_now())
    _seed_note(vault, note_id="n-one", tags=["ndc"], updated=_now() - timedelta(minutes=1))
    ids = {v.note.id for v in list_notes(cfg, tags=["ndc", "flights"])}
    assert ids == {"n-both"}


def test_list_tags_any_semantics(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-both", tags=["ndc", "flights"], updated=_now())
    _seed_note(vault, note_id="n-one", tags=["ndc"], updated=_now() - timedelta(minutes=1))
    _seed_note(vault, note_id="n-none", tags=["misc"], updated=_now() - timedelta(minutes=2))
    ids = {v.note.id for v in list_notes(cfg, tags=["ndc", "flights"], any_tag=True)}
    assert ids == {"n-both", "n-one"}


def test_list_owner_exact_match(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", owner="alice", updated=_now())
    _seed_note(vault, note_id="n-b", owner="alicia", updated=_now() - timedelta(minutes=1))
    ids = {v.note.id for v in list_notes(cfg, owner="alice")}
    assert ids == {"n-a"}


def test_list_type_filter(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-note", note_type="note", updated=_now())
    _seed_note(
        vault,
        note_id="n-dec",
        note_type="decision",
        updated=_now() - timedelta(minutes=1),
    )
    ids = {v.note.id for v in list_notes(cfg, note_type="decision")}
    assert ids == {"n-dec"}


def test_list_since_duration_days(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-recent", updated=_now() - timedelta(days=1))
    _seed_note(vault, note_id="n-old", updated=_now() - timedelta(days=30))
    ids = {v.note.id for v in list_notes(cfg, since="7d")}
    assert ids == {"n-recent"}


def test_list_since_iso_date(cfg: Config, vault: Path) -> None:
    _seed_note(
        vault,
        note_id="n-june",
        updated=datetime(2026, 6, 15, 12, 0, tzinfo=UTC),
    )
    _seed_note(
        vault,
        note_id="n-may",
        updated=datetime(2026, 5, 1, 12, 0, tzinfo=UTC),
    )
    ids = {v.note.id for v in list_notes(cfg, since="2026-06-01")}
    assert ids == {"n-june"}


def test_list_default_sort_updated_desc(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-mid", updated=_now() - timedelta(hours=2))
    _seed_note(vault, note_id="n-new", updated=_now())
    _seed_note(vault, note_id="n-old", updated=_now() - timedelta(hours=5))
    ids = [v.note.id for v in list_notes(cfg)]
    assert ids == ["n-new", "n-mid", "n-old"]


def test_list_sort_created_desc(cfg: Config, vault: Path) -> None:
    base = datetime(2026, 1, 1, 0, 0, tzinfo=UTC)
    _seed_note(vault, note_id="n-first", created=base, updated=base)
    _seed_note(
        vault,
        note_id="n-second",
        created=base + timedelta(days=1),
        updated=base,
    )
    _seed_note(
        vault,
        note_id="n-third",
        created=base + timedelta(days=2),
        updated=base,
    )
    ids = [v.note.id for v in list_notes(cfg, sort="created")]
    assert ids == ["n-third", "n-second", "n-first"]


def test_list_sort_title_asc(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bravo", updated=_now())
    _seed_note(vault, note_id="n-a", title="Alpha", updated=_now() - timedelta(minutes=1))
    _seed_note(vault, note_id="n-c", title="Charlie", updated=_now() - timedelta(minutes=2))
    titles = [v.note.title for v in list_notes(cfg, sort="title")]
    assert titles == ["Alpha", "Bravo", "Charlie"]


def test_list_invalid_sort_raises(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-a")
    with pytest.raises(ValueError):
        list_notes(cfg, sort="bogus")


def test_list_limit_caps_results(cfg: Config, vault: Path) -> None:
    for i in range(5):
        _seed_note(vault, note_id=f"n-{i:02d}", updated=_now() - timedelta(minutes=i))
    assert len(list_notes(cfg, limit=3)) == 3


def test_list_default_limit_is_20(cfg: Config, vault: Path) -> None:
    for i in range(25):
        _seed_note(vault, note_id=f"n-{i:02d}", updated=_now() - timedelta(minutes=i))
    assert len(list_notes(cfg)) == 20


# --------------------------------------------------------------------------- #
# CLI — shards note get                                                         #
# --------------------------------------------------------------------------- #


def test_cli_get_default_preview_truncates_at_200(shards_config: Path, vault: Path) -> None:
    body = "A" * 250
    _seed_note(vault, note_id="n-seed", title="Seed", body=body)
    result = _invoke(["note", "get", "n-seed"])
    assert result.exit_code == 0, result.output
    assert "id: n-seed" in result.output  # frontmatter fields present
    assert "A" * 200 in result.output  # first 200 chars of body
    assert "A" * 201 not in result.output  # truncated, not the full 250


def test_cli_get_full_shows_whole_body(shards_config: Path, vault: Path) -> None:
    body = "A" * 250
    _seed_note(vault, note_id="n-seed", body=body)
    result = _invoke(["note", "get", "n-seed", "--full"])
    assert result.exit_code == 0, result.output
    assert "A" * 250 in result.output


def test_cli_get_meta_only_omits_body(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-seed", body="UNIQUEBODYMARKER")
    result = _invoke(["note", "get", "n-seed", "--meta-only"])
    assert result.exit_code == 0, result.output
    assert "id: n-seed" in result.output
    assert "UNIQUEBODYMARKER" not in result.output


def test_cli_get_related_only(shards_config: Path, vault: Path) -> None:
    _seed_note(
        vault,
        note_id="n-seed",
        body="UNIQUEBODYMARKER",
        related=["n-aaaa", "n-bbbb"],
    )
    result = _invoke(["note", "get", "n-seed", "--related"])
    assert result.exit_code == 0, result.output
    assert "n-aaaa" in result.output
    assert "n-bbbb" in result.output
    assert "UNIQUEBODYMARKER" not in result.output  # body suppressed
    assert "title:" not in result.output  # only the related list


def test_cli_get_not_found_exits_3(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-seed")
    result = _invoke(["note", "get", "n-missing"])
    assert result.exit_code == 3


def test_cli_get_foreign_file_exits_3(shards_config: Path, vault: Path) -> None:
    """Finding #1: a foreign file addressed by stem is not-found (exit 3), no crash."""
    _seed_tolaria(vault, "tolaria-cli", {"title": "Tolaria Cli"})
    result = _invoke(["note", "get", "tolaria-cli"])
    assert result.exit_code == 3, result.output


def test_cli_get_broken_shards_file_exits_3(shards_config: Path, vault: Path) -> None:
    """Finding #3: a shards-id file with malformed frontmatter maps to exit 3, not a traceback."""
    # Valid n- stem (so it resolves) but frontmatter is missing required fields.
    path = vault / "notes" / "n-broken.md"
    post = frontmatter.Post("body", id="n-broken", type="note")  # no title/created/updated
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    result = _invoke(["note", "get", "n-broken"])
    assert result.exit_code == 3, result.output
    assert result.exception is None or isinstance(result.exception, SystemExit), result.output


def test_cli_get_ambiguous_slug_exits_2_lists_ids(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-aaaa", title="Dup Title")
    _seed_note(vault, note_id="n-bbbb", title="Dup Title", note_type="log")
    result = _invoke(["note", "get", "dup-title"])
    assert result.exit_code == 2
    assert "n-aaaa" in result.output
    assert "n-bbbb" in result.output


def test_cli_get_json_single_object(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-seed", title="Seed", tags=["x"], body="the body")
    result = _invoke(["--json", "note", "get", "n-seed"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert isinstance(obj, dict)
    assert obj["id"] == "n-seed"
    assert obj["title"] == "Seed"
    assert obj["tags"] == ["x"]
    assert obj["body"] == "the body"


def test_cli_get_json_full_body(shards_config: Path, vault: Path) -> None:
    body = "A" * 250
    _seed_note(vault, note_id="n-seed", body=body)
    obj = json.loads(_invoke(["--json", "note", "get", "n-seed", "--full"]).output)
    assert obj["body"] == body


def test_cli_get_json_meta_only_has_no_body(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-seed", body="x")
    obj = json.loads(_invoke(["--json", "note", "get", "n-seed", "--meta-only"]).output)
    assert "body" not in obj


def test_cli_get_json_related_is_object(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-seed", related=["n-aaaa"])
    obj = json.loads(_invoke(["--json", "note", "get", "n-seed", "--related"]).output)
    assert obj == {"related": ["n-aaaa"]}


def test_cli_get_quiet_emits_id_only(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-seed", title="Seed", body="body")
    result = _invoke(["--quiet", "note", "get", "n-seed"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "n-seed"


# --------------------------------------------------------------------------- #
# CLI — shards note list                                                        #
# --------------------------------------------------------------------------- #


def test_cli_list_surfaces_shards_notes_only(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-real", title="Real")
    _seed_tolaria(vault, "tolaria", {"title": "Tolaria"})
    result = _invoke(["note", "list"])
    assert result.exit_code == 0, result.output
    assert "n-real" in result.output
    assert "Tolaria" not in result.output


def test_cli_list_json_is_array(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", updated=_now())
    _seed_note(vault, note_id="n-b", updated=_now() - timedelta(minutes=1))
    result = _invoke(["--json", "note", "list"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.output)
    assert isinstance(arr, list)
    assert [o["id"] for o in arr] == ["n-a", "n-b"]


def test_cli_list_quiet_one_id_per_line(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", updated=_now())
    _seed_note(vault, note_id="n-b", updated=_now() - timedelta(minutes=1))
    result = _invoke(["--quiet", "note", "list"])
    assert result.exit_code == 0, result.output
    assert result.output.split() == ["n-a", "n-b"]


def test_cli_list_tags_and_filter(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-both", tags=["ndc", "flights"], updated=_now())
    _seed_note(
        vault,
        note_id="n-one",
        tags=["ndc"],
        updated=_now() - timedelta(minutes=1),
    )
    result = _invoke(["--quiet", "note", "list", "--tags", "ndc,flights"])
    assert result.exit_code == 0, result.output
    assert result.output.split() == ["n-both"]


def test_cli_list_any_tag_filter(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-both", tags=["ndc", "flights"], updated=_now())
    _seed_note(
        vault,
        note_id="n-one",
        tags=["ndc"],
        updated=_now() - timedelta(minutes=1),
    )
    result = _invoke(["--quiet", "note", "list", "--tags", "ndc,flights", "--any-tag"])
    assert result.exit_code == 0, result.output
    assert result.output.split() == ["n-both", "n-one"]


def test_cli_list_owner_filter(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-a", owner="alice", updated=_now())
    _seed_note(vault, note_id="n-b", owner="bob", updated=_now() - timedelta(minutes=1))
    result = _invoke(["--quiet", "note", "list", "--owner", "alice"])
    assert result.output.split() == ["n-a"]


def test_cli_list_type_filter(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-note", note_type="note", updated=_now())
    _seed_note(
        vault,
        note_id="n-dec",
        note_type="decision",
        updated=_now() - timedelta(minutes=1),
    )
    result = _invoke(["--quiet", "note", "list", "--type", "decision"])
    assert result.output.split() == ["n-dec"]


def test_cli_list_since_filter(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-recent", updated=_now() - timedelta(days=1))
    _seed_note(vault, note_id="n-old", updated=_now() - timedelta(days=30))
    result = _invoke(["--quiet", "note", "list", "--since", "7d"])
    assert result.output.split() == ["n-recent"]


def test_cli_list_sort_title(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-b", title="Bravo", updated=_now())
    _seed_note(vault, note_id="n-a", title="Alpha", updated=_now() - timedelta(minutes=1))
    result = _invoke(["--quiet", "note", "list", "--sort", "title"])
    assert result.output.split() == ["n-a", "n-b"]


def test_cli_list_limit(shards_config: Path, vault: Path) -> None:
    for i in range(5):
        _seed_note(vault, note_id=f"n-{i:02d}", updated=_now() - timedelta(minutes=i))
    result = _invoke(["--quiet", "note", "list", "--limit", "2"])
    assert len(result.output.split()) == 2


def test_cli_list_invalid_sort_exits_2(shards_config: Path, vault: Path) -> None:
    _seed_note(vault, note_id="n-a")
    result = _invoke(["note", "list", "--sort", "bogus"])
    assert result.exit_code == 2


# --------------------------------------------------------------------------- #
# Malformed-YAML tolerance (regression) — foreign/corrupt files skip silently  #
# --------------------------------------------------------------------------- #


def _seed_malformed(vault: Path, name: str) -> Path:
    """Write a ``.md`` under ``notes/`` whose frontmatter is invalid YAML."""
    path = vault / "notes" / f"{name}.md"
    path.write_text('---\ntitle: "unterminated\ntags: [a, b\n---\nBody.\n', encoding="utf-8")
    return path


def test_list_notes_skips_malformed_yaml(cfg: Config, vault: Path) -> None:
    """A corrupt-frontmatter file must be skipped, not crash the listing."""
    _seed_note(vault, note_id="n-good", title="Good One")
    _seed_malformed(vault, "n-broken")
    views = list_notes(cfg)
    assert [v.note.id for v in views] == ["n-good"]


def test_get_note_by_slug_skips_malformed_yaml(cfg: Config, vault: Path) -> None:
    """The slug scan reads every shards file; a malformed one must not crash it."""
    _seed_note(vault, note_id="n-good", title="Good One")
    _seed_malformed(vault, "n-broken")  # shards-id stem → enters the slug scan
    assert get_note(cfg, "good-one").note.id == "n-good"


def test_get_note_malformed_shards_file_is_not_found(cfg: Config, vault: Path) -> None:
    """A shards file with corrupt YAML resolves to not-found, mirroring get_task."""
    _seed_malformed(vault, "n-broken")
    with pytest.raises(NoteNotFoundError):
        get_note(cfg, "n-broken")


def test_list_notes_since_tolerates_naive_datetime(cfg: Config, vault: Path) -> None:
    """A note with a naive/date-only ``updated`` must not raise on ``--since`` compare."""
    _seed_note(vault, note_id="n-naive", title="Naive", extra={"updated": "2026-01-01"})
    _seed_note(vault, note_id="n-fresh", title="Fresh", updated=_now())
    ids = {v.note.id for v in list_notes(cfg, since="7d")}
    assert ids == {"n-fresh"}  # naive 2026-01-01 is older than the 7d cutoff, not a crash
