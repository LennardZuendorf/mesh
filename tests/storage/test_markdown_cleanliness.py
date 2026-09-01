"""The Markdown mesh writes stays plain for every other tool sharing the folder.

Invariant 3 (`.spec/tech.md`) promises clean Markdown and byte-for-byte round-trip
of frontmatter keys mesh does not own. These tests pin the two ways a write can
quietly break a *coexisting* tool rather than mesh itself: YAML anchors that a
restricted frontmatter parser cannot resolve, and permission narrowing on a file
mesh did not create.
"""

from __future__ import annotations

import os
from pathlib import Path

import frontmatter

from mesh.storage.files import atomic_write, dump_post, read_post


def _anchors(text: str) -> bool:
    """True when the frontmatter block carries a YAML anchor or alias."""
    block = text.split("---", 2)[1]
    return "&id" in block or "*id" in block


def test_one_object_bound_to_two_keys_emits_no_anchor() -> None:
    """The create path binds one datetime to `created` and `updated`."""
    import datetime

    now = datetime.datetime.now(datetime.UTC)
    post = frontmatter.Post("body", created=now, updated=now, id="n-1")

    assert _anchors(frontmatter.dumps(post)), "precondition: plain dumps anchors"
    assert not _anchors(dump_post(post))


def test_created_and_updated_survive_as_independent_values(tmp_path: Path) -> None:
    """An anchor-free dump still round-trips both timestamps intact."""
    import datetime

    now = datetime.datetime.now(datetime.UTC)
    post = frontmatter.Post("body", created=now, updated=now, id="n-1")
    path = tmp_path / "n-1.md"
    atomic_write(path, dump_post(post))

    reread = read_post(path)
    assert reread is not None
    assert reread["created"] == reread["updated"]
    assert "*id001" not in path.read_text(encoding="utf-8")


def test_unknown_keys_round_trip_byte_for_byte(tmp_path: Path) -> None:
    """Invariant 3: keys mesh does not own come back unchanged."""
    post = frontmatter.Post(
        "body",
        id="n-1",
        cssclass="foreign",
        aliases=["one", "two"],
        nested={"a": [1, 2], "b": {"c": "d"}},
    )
    path = tmp_path / "n-1.md"
    atomic_write(path, dump_post(post))

    reread = read_post(path)
    assert reread is not None
    assert reread["cssclass"] == "foreign"
    assert reread["aliases"] == ["one", "two"]
    assert reread["nested"] == {"a": [1, 2], "b": {"c": "d"}}


def test_overwrite_keeps_the_destination_mode(tmp_path: Path) -> None:
    """A 0644 file checked in by another tool stays 0644 after a mesh write."""
    path = tmp_path / "shared.md"
    path.write_text("before", encoding="utf-8")
    os.chmod(path, 0o644)

    atomic_write(path, "after")

    assert path.stat().st_mode & 0o777 == 0o644
    assert path.read_text(encoding="utf-8") == "after"


def test_overwrite_keeps_a_group_writable_mode(tmp_path: Path) -> None:
    """A shared-vault file keeps every bit it had, not just the read bits."""
    path = tmp_path / "shared.md"
    path.write_text("before", encoding="utf-8")
    os.chmod(path, 0o664)

    atomic_write(path, "after")

    assert path.stat().st_mode & 0o777 == 0o664


def test_new_file_follows_the_process_umask(tmp_path: Path) -> None:
    """A file mesh creates looks like any other tool's, not a 0600 secret."""
    path = tmp_path / "fresh.md"
    previous = os.umask(0o022)
    try:
        atomic_write(path, "content")
    finally:
        os.umask(previous)

    assert path.stat().st_mode & 0o777 == 0o644
