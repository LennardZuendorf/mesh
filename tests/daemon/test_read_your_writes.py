"""An agent sees its own write on the very next read, daemon up or down.

The warm index is refreshed by the filesystem watcher, so its freshness lagged
every write by however long inotify delivery and processing took. That gap landed
on the commonest agent sequence there is — create, then immediately list — where
the agent saw a list without its own new entity and could only conclude the write
had failed. A reviewer reproduced it at 5/5 creations.

Writes stay durable-before-notify and the notification swallows every failure, so
these also pin that the daemon still never gates a write.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from mesh.core.notes import create_note, delete_note
from mesh.core.tasks import claim_task, create_task, finish_task
from mesh.daemon.client import DaemonClient, default_socket_path
from mesh.schemas.config import Config, load_config
from tests.daemon.conftest import running_daemon


@pytest.fixture
def cfg(mesh_config: Path) -> Config:
    """The loaded config for the tmp vault the shared fixture just wrote."""
    return load_config()


def _listed_note_ids(client: DaemonClient, cfg: Config) -> set[str]:
    return {view.note.id for view in client.note_list(cfg, limit=-1)}


def _listed_task_ids(client: DaemonClient, cfg: Config) -> set[str]:
    return {view.task.id for view in client.task_list(cfg, limit=-1)}


def test_a_created_note_is_visible_to_the_very_next_warm_list(cfg: Config, vault: Path) -> None:
    sock = default_socket_path(cfg)
    sock.parent.mkdir(parents=True, exist_ok=True)
    with running_daemon(sock, config=cfg):
        client = DaemonClient(sock, config=cfg)
        for n in range(5):
            note = create_note(cfg, f"Immediate {n}", body="body")
            assert note.id in _listed_note_ids(client, cfg), (
                f"note {note.id} invisible to the read that followed its own write"
            )


def test_a_created_task_is_visible_to_the_very_next_warm_list(cfg: Config, vault: Path) -> None:
    sock = default_socket_path(cfg)
    sock.parent.mkdir(parents=True, exist_ok=True)
    with running_daemon(sock, config=cfg):
        client = DaemonClient(sock, config=cfg)
        for n in range(5):
            task = create_task(cfg, f"Immediate task {n}")
            assert task.id in _listed_task_ids(client, cfg), (
                f"task {task.id} invisible to the read that followed its own write"
            )


def test_a_deleted_note_disappears_from_the_very_next_warm_list(cfg: Config, vault: Path) -> None:
    sock = default_socket_path(cfg)
    sock.parent.mkdir(parents=True, exist_ok=True)
    with running_daemon(sock, config=cfg):
        client = DaemonClient(sock, config=cfg)
        note = create_note(cfg, "Doomed", body="body")
        assert note.id in _listed_note_ids(client, cfg)

        delete_note(cfg, note.id)

        assert note.id not in _listed_note_ids(client, cfg), (
            "a deleted note was still served warm — note get would say not found"
        )


def test_a_finished_task_is_served_at_its_new_path(cfg: Config, vault: Path) -> None:
    """finish moves the file open/ -> done/; both paths must be announced."""
    sock = default_socket_path(cfg)
    sock.parent.mkdir(parents=True, exist_ok=True)
    with running_daemon(sock, config=cfg):
        client = DaemonClient(sock, config=cfg)
        task = create_task(cfg, "Moves on finish")
        claim_task(cfg, task.id, "test-agent")
        finish_task(cfg, task.id, "done", actor="test-agent")

        rows = {view.task.id: view for view in client.task_list(cfg, limit=-1)}
        assert task.id in rows
        assert str(rows[task.id].path).endswith(f"tasks/done/{task.id}.md"), (
            f"warm row still points at the pre-move path: {rows[task.id].path}"
        )


def test_writes_still_succeed_when_the_notification_cannot_be_delivered(
    cfg: Config, vault: Path
) -> None:
    """The daemon never gates a write: a failed poke changes nothing but freshness."""
    note = create_note(cfg, "No daemon here", body="body")

    assert (vault / "notes" / f"{note.id}.md").exists()
