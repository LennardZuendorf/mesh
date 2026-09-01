"""One bad file must degrade one event, never the watcher.

Watchdog does not isolate handler failures: an exception escaping the event
callback kills the observer thread, and the warm index then stops updating for
the daemon's whole life with nothing surfaced anywhere. `.spec/lessons.md`
records that failure mode, and `reconcile.py`'s docstring says "never remove that
guard" — but a mutation audit showed every one of those guards could be deleted
with the whole suite still green. These tests close that hole: each drops a file
that used to be able to raise, then proves the watcher is still alive *and* still
indexing.
"""

from __future__ import annotations

import os
from pathlib import Path

import pytest
from watchdog.events import FileSystemEvent

from shards.index.reconcile import reconcile_path
from shards.index.warm import VaultIndex
from shards.index.watcher import Watcher
from shards.schemas.config import Config


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def _good_note(vault: Path, note_id: str) -> Path:
    return _write(
        vault / "notes" / f"{note_id}.md",
        "---\n"
        f"id: {note_id}\n"
        "type: note\n"
        "title: Good Note\n"
        "tags: []\n"
        "owner: a\n"
        "created: 2026-01-01 00:00:00+00:00\n"
        "updated: 2026-01-01 00:00:00+00:00\n"
        "related: []\n"
        "---\n\nbody\n",
    )


@pytest.mark.parametrize(
    ("name", "content"),
    [
        ("malformed.md", "---\nid: [unclosed\n  bad: *yaml\n---\nbody\n"),
        ("tabs.md", "---\n\tid: n-tab\n---\nbody\n"),
        ("bogus-type.md", "---\nid: n-bog\ntype: bogus\ntitle: T\n---\nbody\n"),
        ("bogus-status.md", "---\nid: t-bog\ntype: task\nstatus: nonsense\ntitle: T\n---\nb\n"),
        ("notmarkdown.txt", "not markdown at all"),
        ("empty.md", ""),
        ("no-frontmatter.md", "just a body, no frontmatter\n"),
        ("null-id.md", "---\nid: null\ntype: note\n---\nbody\n"),
        ("list-id.md", "---\nid: [1, 2]\ntype: note\n---\nbody\n"),
    ],
)
def test_reconcile_never_raises_on_a_hostile_file(
    shards_config: Path, vault: Path, name: str, content: str
) -> None:
    """Each of these used to be a live route to killing the observer thread."""
    from shards.schemas.config import load_config

    config: Config = load_config()
    path = _write(vault / "notes" / name, content)

    result = reconcile_path(config, path)

    assert isinstance(result, Path)
    assert path.exists(), "a file shards cannot classify must be left alone"


def test_a_non_markdown_file_is_never_relocated(shards_config: Path, vault: Path) -> None:
    """The `.md` guard is about *placement*, not about raising.

    A sidecar carrying shards-shaped frontmatter (an export, a template, another
    tool's scratch file) must stay exactly where its owner put it. Without the
    suffix guard reconcile happily files it under `notes/logs/`, silently moving a
    file shards does not own.
    """
    from shards.schemas.config import load_config

    sidecar = _write(
        vault / "notes" / "sidecar.txt",
        "---\nid: n-side\ntype: log\ntitle: Sidecar\n---\n\nbody\n",
    )

    result = reconcile_path(load_config(), sidecar)

    assert sidecar.exists(), "a non-.md file was relocated out from under its owner"
    assert not (vault / "notes" / "logs" / "sidecar.txt").exists()
    assert result == sidecar


def test_reconcile_leaves_a_symlink_escaping_the_vault_in_place(
    shards_config: Path, vault: Path, tmp_path: Path
) -> None:
    """A link pointing outside the sandbox is skipped, not followed and moved."""
    from shards.schemas.config import load_config

    outside = _write(tmp_path / "outside" / "n-esc.md", "---\nid: n-esc\ntype: log\n---\nb\n")
    link = vault / "notes" / "n-esc.md"
    os.symlink(outside, link)

    result = reconcile_path(load_config(), link)

    assert isinstance(result, Path)
    assert outside.exists()


def test_the_watcher_survives_a_hostile_file_and_keeps_indexing(
    shards_config: Path, vault: Path
) -> None:
    """The whole point: a bad file costs one event, not the watcher.

    Drives the real watchdog adapter the way the observer thread does, so the
    guard under test is the one that actually runs in the daemon.
    """
    from shards.schemas.config import load_config

    config = load_config()
    index = VaultIndex()
    watcher = Watcher(config, index, None)

    hostile = _write(vault / "notes" / "boom.md", "---\nid: [unclosed\n---\nbody\n")
    watcher.handler.on_modified(_FakeEvent(str(hostile)))

    good = _good_note(vault, "n-live")
    watcher.handler.on_modified(_FakeEvent(str(good)))

    assert any(entry.path == good.resolve() for entry in index.entries()), (
        "the watcher stopped indexing after a hostile file"
    )


def test_an_exception_from_handle_event_never_escapes(
    shards_config: Path, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Belt and braces: even an unforeseen bug must not reach the observer."""
    from shards.schemas.config import load_config

    watcher = Watcher(load_config(), VaultIndex(), None)

    def explode(event: object) -> None:
        raise RuntimeError("unforeseen")

    monkeypatch.setattr(watcher, "handle_event", explode)

    watcher.handler.on_created(_FakeEvent(str(vault / "notes" / "x.md")))
    watcher.handler.on_modified(_FakeEvent(str(vault / "notes" / "x.md")))
    watcher.handler.on_deleted(_FakeEvent(str(vault / "notes" / "x.md")))


class _FakeEvent(FileSystemEvent):
    """Minimal stand-in for a watchdog file event."""

    def __init__(self, src_path: str, event_type: str = "modified") -> None:
        super().__init__(src_path)
        self.event_type = event_type
