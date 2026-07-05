"""tasks/4 — ``task finish``: append outcome + atomic move to done/ (R3).

Exercises R3 (Finish). Finishing a task, under the per-entity ``O_EXCL`` lock:

* appends a ``## Outcome`` section carrying an ISO-8601 UTC timestamp line and
  the (optional) outcome text to the task body;
* sets ``status=done`` and bumps ``updated`` (``created`` is left untouched);
* moves the file from ``tasks/open/`` to ``tasks/done/`` via an atomic rename.

It is accepted from any *non-terminal* status (``open`` or ``claimed``), and is
idempotent: re-finishing an already-``done`` task is a no-op — no second
``## Outcome`` section, no rewrite, the file stays in ``tasks/done/``. Terminal
states (``done``/``cancelled``) never transition. The whole path uses
``storage`` primitives directly, so it behaves identically with the daemon down.
The move is asserted at the behaviour level (file absent from ``open/``, present
in ``done/``, id re-resolves) rather than by spying ``os.replace``.
"""

from __future__ import annotations

import json
import re
import threading
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

import brain.cli.task as task_cli
from brain.cli.__main__ import app
from brain.core.tasks import (
    TaskNotFoundError,
    _resolve_task_path,
    finish_task,
)
from brain.schemas.config import Config, load_config
from brain.storage.files import task_folder

_OLD = datetime(2026, 1, 1, 9, 0, 0, tzinfo=UTC)
_ISO_UTC = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z")


@pytest.fixture
def cfg(brain_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _reload(path: Path) -> frontmatter.Post:
    return frontmatter.loads(path.read_text(encoding="utf-8"))


def _seed_task(
    vault: Path,
    *,
    task_id: str = "t-seed",
    title: str = "Seed Task",
    status: str = "open",
    owner: str | None = "seed-agent",
    claimed_by: str | None = None,
    body: str = "Task body.",
    created: datetime = _OLD,
    updated: datetime = _OLD,
) -> Path:
    """Write a brain task straight to disk in the folder matching its status."""
    meta: dict[str, object] = {
        "id": task_id,
        "type": "task",
        "title": title,
        "tags": [],
        "owner": owner,
        "created": created,
        "updated": updated,
        "related": [],
        "status": status,
        "priority": None,
        "claimed_by": claimed_by,
        "blocks": [],
        "blocked_by": [],
    }
    folder = task_folder(status, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


def _open_path(vault: Path, task_id: str = "t-seed") -> Path:
    return task_folder("open", vault) / f"{task_id}.md"


def _done_path(vault: Path, task_id: str = "t-seed") -> Path:
    return task_folder("done", vault) / f"{task_id}.md"


# --------------------------------------------------------------------------- #
# finish_task (core) — open → done: append outcome, move folder                #
# --------------------------------------------------------------------------- #


def test_finish_open_appends_outcome_and_moves(cfg: Config, vault: Path) -> None:
    _seed_task(vault, status="open", body="Original body.")
    task = finish_task(cfg, "t-seed", "Shipped it.")

    assert task.status == "done"
    # The file moved: gone from open/, present in done/.
    assert not _open_path(vault).exists()
    assert _done_path(vault).exists()

    post = _reload(_done_path(vault))
    assert post.metadata["status"] == "done"
    assert post.metadata["updated"] > _OLD  # bumped on the finishing write
    assert post.metadata["created"] == _OLD  # birth instant untouched
    # Original body survives; the outcome section is appended after it.
    assert "Original body." in post.content
    assert "## Outcome" in post.content
    assert _ISO_UTC.search(post.content) is not None
    assert "Shipped it." in post.content
    # Section header precedes both the timestamp and the outcome text.
    assert post.content.index("## Outcome") < post.content.index("Shipped it.")


def test_finish_claimed_is_allowed(cfg: Config, vault: Path) -> None:
    """R3: a claimed (non-terminal) task may be finished."""
    _seed_task(vault, task_id="t-claimed", status="claimed", claimed_by="test-agent")
    task = finish_task(cfg, "t-claimed", "Done.")
    assert task.status == "done"
    assert not _open_path(vault, "t-claimed").exists()
    assert _done_path(vault, "t-claimed").exists()


def test_finish_timestamp_line_precedes_outcome(cfg: Config, vault: Path) -> None:
    _seed_task(vault, status="open")
    finish_task(cfg, "t-seed", "with clock")
    content = _reload(_done_path(vault)).content
    match = _ISO_UTC.search(content)
    assert match is not None
    assert match.start() < content.index("with clock")


# --------------------------------------------------------------------------- #
# finish_task (core) — optional outcome                                         #
# --------------------------------------------------------------------------- #


def test_finish_without_outcome_still_appends_header_and_timestamp(
    cfg: Config, vault: Path
) -> None:
    _seed_task(vault, status="open")
    task = finish_task(cfg, "t-seed", None)
    assert task.status == "done"
    content = _reload(_done_path(vault)).content
    assert "## Outcome" in content
    assert _ISO_UTC.search(content) is not None


# --------------------------------------------------------------------------- #
# finish_task (core) — idempotent terminal no-op                                #
# --------------------------------------------------------------------------- #


def test_finish_already_done_is_idempotent(cfg: Config, vault: Path) -> None:
    body = "Task body.\n\n## Outcome\n\n2026-01-01T09:00:00Z\nFirst outcome."
    _seed_task(vault, status="done", body=body, updated=_OLD)
    task = finish_task(cfg, "t-seed", "Second outcome.")

    assert task.status == "done"
    # File remains in done/ (never moved back or duplicated).
    assert _done_path(vault).exists()
    assert not _open_path(vault).exists()

    post = _reload(_done_path(vault))
    # No second '## Outcome' section; no re-write (updated unchanged).
    assert post.content.count("## Outcome") == 1
    assert "Second outcome." not in post.content
    assert post.metadata["updated"] == _OLD


def test_finish_already_done_does_not_write(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """An already-done finish must not touch atomic_write (pure no-op)."""
    import brain.core.tasks as tasks_core

    _seed_task(vault, status="done", body="Task body.\n\n## Outcome\n\nx")
    calls: list[Path] = []
    monkeypatch.setattr(tasks_core, "atomic_write", lambda path, content: calls.append(path))
    finish_task(cfg, "t-seed", "ignored")
    assert calls == []


def test_finish_cancelled_is_noop(cfg: Config, vault: Path) -> None:
    """Terminal 'cancelled' never resurrects to done (terminal re-run = no-op)."""
    _seed_task(vault, status="cancelled", body="Task body.\n\n## Cancelled\n\nreason")
    task = finish_task(cfg, "t-seed", "Done.")
    assert task.status == "cancelled"
    post = _reload(_done_path(vault))
    assert post.metadata["status"] == "cancelled"
    assert "## Outcome" not in post.content


def test_finish_reconciles_crash_stranded_file(cfg: Config, vault: Path) -> None:
    """A terminal file stranded in open/ by a crash mid-move is healed, not stuck.

    Simulates a crash between the atomic write (status=done) and the open→done
    rename: the file sits in tasks/open/ with a terminal status. A re-finish must
    reconcile it into done/ rather than short-circuit and leave it unrecoverable.
    """
    meta: dict[str, object] = {
        "id": "t-crash", "type": "task", "title": "Crash", "tags": [],
        "owner": "seed-agent", "created": _OLD, "updated": _OLD, "related": [],
        "status": "done", "priority": None, "claimed_by": None,
        "blocks": [], "blocked_by": [],
    }
    stranded = task_folder("open", vault) / "t-crash.md"
    stranded.write_text(
        frontmatter.dumps(frontmatter.Post("Body.\n\n## Outcome\n\nx", **meta)),
        encoding="utf-8",
    )
    task = finish_task(cfg, "t-crash", "ignored")
    assert task.status == "done"
    assert _done_path(vault, "t-crash").exists()
    assert not _open_path(vault, "t-crash").exists()
    assert _reload(_done_path(vault, "t-crash")).content.count("## Outcome") == 1


# --------------------------------------------------------------------------- #
# finish_task (core) — resolution                                              #
# --------------------------------------------------------------------------- #


def test_finish_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here", status="open")
    with pytest.raises(TaskNotFoundError):
        finish_task(cfg, "t-nope", "Done.")


def test_finish_id_reresolves_from_done(cfg: Config, vault: Path) -> None:
    """After finishing, the id resolves from tasks/done/ (resolver scans both)."""
    _seed_task(vault, status="open")
    finish_task(cfg, "t-seed", "Done.")
    resolved = _resolve_task_path(cfg, "t-seed")
    assert resolved == _done_path(vault).resolve()


# --------------------------------------------------------------------------- #
# finish_task (core) — concurrent finish: idempotent under a race              #
# --------------------------------------------------------------------------- #


def test_finish_concurrent_appends_outcome_once(cfg: Config, vault: Path) -> None:
    """N racing finishers append exactly one ## Outcome and never raise.

    Because finish moves the file open→done, resolution must happen *inside* the
    lock: a loser that resolved the open path before the winner's move would
    otherwise read a vanished path. All callers must return status=done with no
    exception, and the body must carry a single outcome section.
    """
    n = 10
    _seed_task(vault, task_id="t-race", status="open", body="Body.")
    barrier = threading.Barrier(n)

    def attempt(_: int) -> str:
        barrier.wait()  # release all finishers simultaneously
        return finish_task(cfg, "t-race", "Done.").status

    with ThreadPoolExecutor(max_workers=n) as pool:
        statuses = list(pool.map(attempt, range(n)))

    assert statuses == ["done"] * n  # no exception, every call sees terminal
    assert not _open_path(vault, "t-race").exists()
    post = _reload(_done_path(vault, "t-race"))
    assert post.content.count("## Outcome") == 1


# --------------------------------------------------------------------------- #
# CLI — brain task finish                                                       #
# --------------------------------------------------------------------------- #


def test_cli_finish_emits_finished(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="open")
    result = _invoke(["task", "finish", "t-xxx", "--outcome", "Done."])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "finished t-xxx"
    post = _reload(_done_path(vault, "t-xxx"))
    assert post.metadata["status"] == "done"
    assert "Done." in post.content


def test_cli_finish_without_outcome(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="open")
    result = _invoke(["task", "finish", "t-xxx"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "finished t-xxx"
    assert "## Outcome" in _reload(_done_path(vault, "t-xxx")).content


def test_cli_finish_quiet_emits_id_only(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="open")
    result = _invoke(["--quiet", "task", "finish", "t-xxx", "--outcome", "Done."])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "t-xxx"


def test_cli_finish_json_object(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="open")
    result = _invoke(["--json", "task", "finish", "t-xxx", "--outcome", "Done."])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["id"] == "t-xxx"
    assert obj["status"] == "done"
    assert "updated" in obj


def test_cli_finish_not_found_exits_3(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here", status="open")
    result = _invoke(["task", "finish", "t-missing"])
    assert result.exit_code == 3, result.output


def test_cli_finish_already_done_exits_0(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="done", body="b\n\n## Outcome\n\nx")
    result = _invoke(["task", "finish", "t-xxx", "--outcome", "again"])
    assert result.exit_code == 0, result.output
    assert _done_path(vault, "t-xxx").exists()


def test_finish_command_registered() -> None:
    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert "finish" in names
