"""notes/5 — wikilinks: ``[[Title]]`` / ``[[n-id]]`` / ``[[t-id]]`` → ``related``.

Exercises R4 (Wikilinks). :func:`shards.core.wikilinks.resolve_wikilinks` scans a
note body for ``[[...]]`` links and returns ``(body_unchanged, resolved_ids)``:
a ``[[Title]]`` link is resolved by an on-disk lookup across ``notes/`` (no
daemon), an id-form ``[[n-id]]`` / ``[[t-id]]`` passes through verbatim and its
id is taken directly (no file lookup). Unresolvable titles stay verbatim in the
body and surface via :func:`shards.core.wikilinks.find_dangling` for ``shards
status``. The amend verbs (``append_note`` / ``update_note``) call the resolver
on every body write and persist the derived ``related`` list — ``related`` is a
pure function of the body.

Only shards-owned notes (id ``n-…``) participate in title resolution, mirroring
``list_notes``: a coexisting foreign file (title present, non-shards id)
must never shadow a link nor leak a foreign id into ``related``.
"""

from __future__ import annotations

from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest

from shards.core.notes import append_note, update_note
from shards.core.wikilinks import find_dangling, resolve_wikilinks
from shards.schemas.config import Config, load_config
from shards.storage.files import note_folder

_WHEN = datetime(2026, 6, 1, 12, 0, 0, tzinfo=UTC)


def _seed_note(
    vault: Path,
    *,
    note_id: str,
    title: str = "A Note",
    note_type: str = "note",
    body: str = "Body line.",
    related: list[str] | None = None,
) -> Path:
    """Write a shards note straight to disk in the folder matching its type."""
    meta: dict[str, object] = {
        "id": note_id,
        "type": note_type,
        "title": title,
        "tags": [],
        "owner": "seed-agent",
        "created": _WHEN,
        "updated": _WHEN,
        "related": list(related or []),
    }
    folder = note_folder(note_type, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{note_id}.md"
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def _seed_foreign(vault: Path, name: str, meta: dict[str, object]) -> Path:
    """Write a non-shards Markdown file (id is not a shards ``n-`` id) under ``notes/``."""
    path = vault / "notes" / f"{name}.md"
    post = frontmatter.Post("Foreign content.")
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def _reload(path: Path) -> frontmatter.Post:
    return frontmatter.loads(path.read_text(encoding="utf-8"))


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


# --------------------------------------------------------------------------- #
# resolve_wikilinks — title resolution, id passthrough, body verbatim          #
# --------------------------------------------------------------------------- #


def test_title_link_resolves_to_id_body_unchanged(vault: Path) -> None:
    _seed_note(vault, note_id="n-tgt1", title="CLID Fallback")
    body = "See [[CLID Fallback]] for the rationale."
    out_body, related = resolve_wikilinks(body, vault)
    assert related == ["n-tgt1"]
    assert out_body == body  # body returned verbatim, never rewritten


def test_id_links_passthrough_without_file_lookup(vault: Path) -> None:
    # No files exist for these ids; id-form links must still resolve directly.
    body = "Refs [[n-abcd]] and [[t-wxyz]] inline."
    out_body, related = resolve_wikilinks(body, vault)
    assert related == ["n-abcd", "t-wxyz"]
    assert out_body == body


def test_title_lookup_crosses_folders(vault: Path) -> None:
    _seed_note(vault, note_id="n-dec1", title="Big Decision", note_type="decision")
    _, related = resolve_wikilinks("Per [[Big Decision]].", vault)
    assert related == ["n-dec1"]


def test_no_wikilinks_yields_empty_related(vault: Path) -> None:
    out_body, related = resolve_wikilinks("Plain body, no links here.", vault)
    assert related == []
    assert out_body == "Plain body, no links here."


def test_related_dedup_and_stable_insertion_order(vault: Path) -> None:
    _seed_note(vault, note_id="n-aaaa", title="Alpha")
    _seed_note(vault, note_id="n-bbbb", title="Beta")
    # Beta→n-bbbb, n-aaaa (id), Alpha→n-aaaa (dup), then repeats — dupes dropped,
    # first-seen order preserved.
    body = "[[Beta]] [[n-aaaa]] [[Alpha]] [[Beta]] [[n-aaaa]]"
    _, related = resolve_wikilinks(body, vault)
    assert related == ["n-bbbb", "n-aaaa"]


# --------------------------------------------------------------------------- #
# Dangling links — unresolvable titles stay verbatim, reported by find_dangling #
# --------------------------------------------------------------------------- #


def test_unresolvable_title_is_dangling_and_verbatim(vault: Path) -> None:
    _seed_note(vault, note_id="n-src1", title="Source", body="Link to [[Ghost Note]].")
    out_body, related = resolve_wikilinks("Link to [[Ghost Note]].", vault)
    assert related == []  # nothing resolved
    assert "[[Ghost Note]]" in out_body  # left verbatim in the body text
    assert "Ghost Note" in find_dangling(vault)


def test_find_dangling_dedupes_and_ignores_resolvable_and_id_forms(vault: Path) -> None:
    _seed_note(vault, note_id="n-real", title="Real")
    _seed_note(vault, note_id="n-s1", title="S1", body="[[Real]] [[Phantom]] [[n-zzzz]]")
    _seed_note(vault, note_id="n-s2", title="S2", body="[[Phantom]] mentioned again")
    # Only the unresolvable title 'Phantom' is dangling; resolvable 'Real' and the
    # id-form [[n-zzzz]] are excluded; the repeat across notes is de-duplicated.
    assert find_dangling(vault) == ["Phantom"]


def test_find_dangling_empty_when_all_resolve(vault: Path) -> None:
    _seed_note(vault, note_id="n-t", title="Target")
    _seed_note(vault, note_id="n-r", title="Ref", body="[[Target]] and [[n-t]] and [[t-xxxx]]")
    assert find_dangling(vault) == []


def _seed_task(
    vault: Path,
    *,
    task_id: str,
    rel: str = "tasks/open",
    body: str = "Task body.",
) -> Path:
    """Write a shards task straight to disk (core-hardening/4: dangling covers tasks too)."""
    meta: dict[str, object] = {
        "id": task_id,
        "type": "task",
        "title": "A Task",
        "tags": [],
        "owner": "seed-agent",
        "status": "open",
        "created": _WHEN,
        "updated": _WHEN,
        "related": [],
    }
    folder = vault / rel
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def test_find_dangling_covers_task_bodies(vault: Path) -> None:
    # root tech.md § B6 / product.md "Vault-health counts cover the whole vault":
    # a title-form wikilink in a task body that matches no note is dangling too.
    _seed_task(vault, task_id="t-open1", body="Blocked on [[Missing Design Doc]].")
    assert "Missing Design Doc" in find_dangling(vault)


def test_find_dangling_task_id_form_link_is_not_dangling(vault: Path) -> None:
    # An id-form link in a task body is never dangling, matching note behaviour.
    _seed_task(vault, task_id="t-open2", body="See [[n-nope]] and [[t-nope]].")
    assert find_dangling(vault) == []


def test_find_dangling_task_title_link_resolving_to_a_note_is_not_dangling(vault: Path) -> None:
    _seed_note(vault, note_id="n-spec", title="Design Doc")
    _seed_task(vault, task_id="t-open3", body="See [[Design Doc]] for details.")
    assert find_dangling(vault) == []


def test_find_dangling_dedupes_across_notes_and_tasks(vault: Path) -> None:
    _seed_note(vault, note_id="n-s1", title="S1", body="[[Phantom]]")
    _seed_task(vault, task_id="t-open4", body="Also references [[Phantom]].")
    assert find_dangling(vault) == ["Phantom"]


# --------------------------------------------------------------------------- #
# Shards-notes-only: coexisting foreign files must not resolve or leak ids      #
# --------------------------------------------------------------------------- #


def test_foreign_title_does_not_resolve(vault: Path) -> None:
    # A non-shards file with a title but a foreign id must not shadow the link.
    _seed_foreign(vault, "daily-2026-06-01", {"id": "ext-123", "title": "Daily Log"})
    _seed_note(vault, note_id="n-src", title="Src", body="See [[Daily Log]].")
    out_body, related = resolve_wikilinks("See [[Daily Log]].", vault)
    assert related == []  # foreign id never leaks into related
    assert "[[Daily Log]]" in out_body  # unresolved -> left verbatim
    # The shards note's link to the foreign title is dangling, reported for status.
    assert find_dangling(vault) == ["Daily Log"]


def test_shards_note_wins_over_foreign_same_title(vault: Path) -> None:
    _seed_foreign(vault, "foreign", {"id": "ext-9", "title": "Shared Title"})
    _seed_note(vault, note_id="n-shards", title="Shared Title")
    _, related = resolve_wikilinks("[[Shared Title]]", vault)
    assert related == ["n-shards"]


# --------------------------------------------------------------------------- #
# Idempotency                                                                   #
# --------------------------------------------------------------------------- #


def test_resolution_is_idempotent(vault: Path) -> None:
    _seed_note(vault, note_id="n-tgt2", title="Target")
    body = "[[Target]] and [[n-xxxx]] and [[Missing One]]"
    b1, r1 = resolve_wikilinks(body, vault)
    b2, r2 = resolve_wikilinks(b1, vault)
    assert r1 == r2 == ["n-tgt2", "n-xxxx"]
    assert b1 == b2 == body  # running twice changes nothing


# --------------------------------------------------------------------------- #
# Wiring — append_note / update_note derive & persist related on body writes     #
# --------------------------------------------------------------------------- #


def test_append_persists_resolved_related(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-ref9", title="Referenced")
    src = _seed_note(vault, note_id="n-src9", title="Source", body="Intro.")
    append_note(cfg, "n-src9", "Now see [[Referenced]] and [[t-task1]].")
    meta = _reload(src).metadata
    assert meta["related"] == ["n-ref9", "t-task1"]


def test_append_leaves_wikilink_text_verbatim_in_body(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-refb", title="Target B")
    src = _seed_note(vault, note_id="n-srcb", title="Src B", body="Intro.")
    append_note(cfg, "n-srcb", "See [[Target B]] and [[Dangling One]].")
    content = _reload(src).content
    assert "[[Target B]]" in content  # resolved link stays in body
    assert "[[Dangling One]]" in content  # dangling link stays verbatim too
    assert _reload(src).metadata["related"] == ["n-refb"]


def test_append_recomputes_related_dropping_stale(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-kept", title="Kept")
    src = _seed_note(vault, note_id="n-srck", title="Src K", body="[[Kept]]", related=["n-stale"])
    # related is a pure function of the body: a pre-existing stale id is dropped.
    append_note(cfg, "n-srck", "trailing text, no links")
    assert _reload(src).metadata["related"] == ["n-kept"]


def test_update_persists_related_from_body(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-refa", title="Linked")
    src = _seed_note(vault, note_id="n-srca", title="Src A", body="Refers to [[Linked]] here.")
    update_note(cfg, "n-srca", tags="+x")
    assert _reload(src).metadata["related"] == ["n-refa"]


def test_append_related_is_idempotent(cfg: Config, vault: Path) -> None:
    _seed_note(vault, note_id="n-refc", title="Once")
    src = _seed_note(vault, note_id="n-srcc", title="Src C", body="[[Once]]")
    append_note(cfg, "n-srcc", "extra 1")
    first = _reload(src).metadata["related"]
    append_note(cfg, "n-srcc", "extra 2")
    second = _reload(src).metadata["related"]
    assert first == second == ["n-refc"]  # no dupes accumulate across writes


def test_malformed_yaml_note_does_not_crash_wikilinks(cfg: Config, vault: Path) -> None:
    """A corrupt note in ``notes/`` must be skipped by title-index and dangling scans."""
    _seed_note(vault, note_id="n-ref", title="Target")
    (vault / "notes" / "n-broken.md").write_text(
        '---\ntitle: "unterminated\n---\n[[Nowhere]]\n', encoding="utf-8"
    )
    # Title resolution still works despite the corrupt sibling.
    _, related = resolve_wikilinks("[[Target]]", vault)
    assert related == ["n-ref"]
    # find_dangling (feeds `shards status`) skips the corrupt file instead of raising.
    assert find_dangling(vault) == []


# --------------------------------------------------------------------------- #
# Task scope — the dangling scan walks exactly the task lifecycle folders        #
# --------------------------------------------------------------------------- #


def test_find_dangling_ignores_files_outside_the_task_lifecycle_folders(vault: Path) -> None:
    """The scan walks ``tasks/{open,done}`` non-recursively, like ``core.tasks``.

    ``core.tasks._iter_task_files`` / ``in_task_scope`` are deliberately
    non-recursive over exactly those two folders, so a file filed beside them
    (``tasks/archive/``) or a level deeper (``tasks/open/sub/``) is not a task
    this program can get, claim, finish or edit. ``shards status`` must not
    count broken links from files no verb can reach — the count would name a
    problem the tool offers no way to fix.
    """
    _seed_task(vault, task_id="t-arch", rel="tasks/archive", body="[[Ghost From Archive]]")
    _seed_task(vault, task_id="t-nest", rel="tasks/open/sub", body="[[Ghost From Subfolder]]")
    assert find_dangling(vault) == []


def test_find_dangling_still_covers_both_lifecycle_folders(vault: Path) -> None:
    """Narrowing the walk must not lose ``tasks/done/`` — both folders still scan."""
    _seed_task(vault, task_id="t-o", rel="tasks/open", body="[[Ghost Open]]")
    _seed_task(vault, task_id="t-d", rel="tasks/done", body="[[Ghost Done]]")
    assert find_dangling(vault) == ["Ghost Open", "Ghost Done"]
