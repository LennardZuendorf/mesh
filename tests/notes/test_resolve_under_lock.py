"""core-hardening — the note amend/delete verbs resolve *inside* the entity lock.

``update_note(new_type=...)`` ``os.replace``s a note into a different folder, so
the path a caller resolved a moment ago can stop existing before that caller
acts on it. The task side already closed this window (``core.tasks``: the lock
name derives from the caller's id, the *path* is resolved inside the lock — see
``update_task`` / ``claim_task`` / ``_terminate_task`` / ``delete_task``); the
note side resolved before ``hold(...)`` and so kept a live TOCTOU:

* ``delete_note`` unlinked a path a racing type-move had already renamed →
  ``FileNotFoundError`` (CLI exit 1, ``io error: [Errno 2]``) **and the note
  survived the delete**;
* ``append_note`` / ``update_note`` re-read a path that no longer existed →
  a spurious ``NoteNotFoundError`` (CLI exit 3) for a note that is right there.

Each test below forces exactly that interleaving. The technique is to widen the
window rather than to invent one: ``_resolve_path`` is wrapped so the racing
verb's **first** call parks after returning, the other thread completes its type
move, and only then does the first thread proceed. The first call is the one
that happens outside the lock in both the broken and the fixed shape, so the
same test drives both without deadlocking — under the fix the thread parks
before taking the lock, wakes, takes it, and re-resolves.
"""

from __future__ import annotations

import threading
from collections.abc import Callable, Iterator
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest

import shards.core.notes as notes_mod
from shards.core.notes import append_note, delete_note, update_note
from shards.schemas.config import Config, load_config
from shards.storage.files import note_folder

_NOW = datetime(2026, 6, 1, 12, 0, 0, tzinfo=UTC)
_RACER = "racing-verb"  # thread name the widened resolver parks on
_TIMEOUT = 20.0  # generous: a hang means the fix deadlocked, not that CI is slow


def _seed_note(vault: Path, *, note_id: str, title: str = "Race Note") -> Path:
    """Write a plain ``type: note`` straight into ``notes/``."""
    folder = note_folder("note", vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{note_id}.md"
    post = frontmatter.Post("Body line.")
    post.metadata = {
        "id": note_id,
        "type": "note",
        "title": title,
        "tags": [],
        "owner": "seed-agent",
        "created": _NOW,
        "updated": _NOW,
        "related": [],
    }
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


class _Interleave:
    """Drives one forced ordering: racer resolves → mover moves → racer continues."""

    def __init__(self) -> None:
        self.resolved = threading.Event()
        self.moved = threading.Event()
        self.error: BaseException | None = None
        self._parked = False

    def widen(self, real: Callable[[Config, str], Path]) -> Callable[[Config, str], Path]:
        """Wrap ``_resolve_path`` so the racer's first call parks after returning."""

        def widened(config: Config, id_or_slug: str) -> Path:
            result = real(config, id_or_slug)
            if threading.current_thread().name == _RACER and not self._parked:
                self._parked = True
                self.resolved.set()
                assert self.moved.wait(timeout=_TIMEOUT), "mover never finished"
            return result

        return widened

    def run(self, racer: Callable[[], None], mover: Callable[[], None]) -> None:
        """Run ``racer`` on its own thread, interleaved with ``mover`` here."""

        def guarded() -> None:
            try:
                racer()
            except BaseException as exc:  # noqa: BLE001 — re-raised on the main thread
                self.error = exc

        thread = threading.Thread(target=guarded, name=_RACER)
        thread.start()
        try:
            assert self.resolved.wait(timeout=_TIMEOUT), "racer never resolved"
            mover()
        finally:
            self.moved.set()
            thread.join(timeout=_TIMEOUT)
        assert not thread.is_alive(), "racer hung"
        if self.error is not None:
            raise self.error


@pytest.fixture
def interleave(monkeypatch: pytest.MonkeyPatch) -> Iterator[_Interleave]:
    """An ``_Interleave`` with ``core.notes._resolve_path`` already widened."""
    orchestrator = _Interleave()
    monkeypatch.setattr(notes_mod, "_resolve_path", orchestrator.widen(notes_mod._resolve_path))
    yield orchestrator


def _moved_path(vault: Path, note_id: str) -> Path:
    return note_folder("log", vault) / f"{note_id}.md"


# --------------------------------------------------------------------------- #
# delete — the proven failure: unlink of a path a racing move already renamed  #
# --------------------------------------------------------------------------- #


def test_delete_survives_a_concurrent_type_move(
    cfg: Config, vault: Path, interleave: _Interleave
) -> None:
    """Agent A deletes while agent B retypes: the note is deleted, no ``io error``."""
    original = _seed_note(vault, note_id="n-race1")

    interleave.run(
        racer=lambda: delete_note(cfg, "n-race1"),
        mover=lambda: update_note(cfg, "n-race1", new_type="log"),
    )

    assert not original.exists()
    assert not _moved_path(vault, "n-race1").exists()  # deleted, not resurrected


def test_delete_by_slug_survives_a_concurrent_type_move(
    cfg: Config, vault: Path, interleave: _Interleave
) -> None:
    """The lock name comes from the resolved id, so a slug caller races identically."""
    original = _seed_note(vault, note_id="n-race2", title="Slug Target")

    interleave.run(
        racer=lambda: delete_note(cfg, "slug-target"),
        mover=lambda: update_note(cfg, "n-race2", new_type="log"),
    )

    assert not original.exists()
    assert not _moved_path(vault, "n-race2").exists()


# --------------------------------------------------------------------------- #
# append / update — the spurious "note not found" for a note that exists       #
# --------------------------------------------------------------------------- #


def test_append_survives_a_concurrent_type_move(
    cfg: Config, vault: Path, interleave: _Interleave
) -> None:
    _seed_note(vault, note_id="n-race3")

    interleave.run(
        racer=lambda: append_note(cfg, "n-race3", "appended text"),
        mover=lambda: update_note(cfg, "n-race3", new_type="log"),
    )

    moved = _moved_path(vault, "n-race3")
    assert moved.exists()
    assert "appended text" in frontmatter.loads(moved.read_text(encoding="utf-8")).content


def test_update_survives_a_concurrent_type_move(
    cfg: Config, vault: Path, interleave: _Interleave
) -> None:
    _seed_note(vault, note_id="n-race4")

    interleave.run(
        racer=lambda: update_note(cfg, "n-race4", tags="+tagged"),
        mover=lambda: update_note(cfg, "n-race4", new_type="log"),
    )

    moved = _moved_path(vault, "n-race4")
    assert moved.exists()
    assert frontmatter.loads(moved.read_text(encoding="utf-8")).metadata["tags"] == ["tagged"]


# --------------------------------------------------------------------------- #
# A note that genuinely vanishes mid-race is still not-found, not a crash      #
# --------------------------------------------------------------------------- #


def test_delete_racing_a_real_delete_is_still_not_found(
    cfg: Config, vault: Path, interleave: _Interleave
) -> None:
    """Re-resolving inside the lock must not paper over an actual disappearance."""
    original = _seed_note(vault, note_id="n-race5")

    with pytest.raises(notes_mod.NoteNotFoundError):
        interleave.run(
            racer=lambda: delete_note(cfg, "n-race5"),
            mover=lambda: original.unlink(),
        )

    assert not original.exists()
