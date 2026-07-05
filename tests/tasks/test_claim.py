"""tasks/3 — ``task claim``: atomic, single-winner claim (R2).

Exercises R2 (Claim). A claim is an atomic check-and-set on ``claimed_by`` under
the per-entity ``O_EXCL`` lock at ``tasks/.locks/<id>.lock``:

* unclaimed → durable ``claimed_by=claimer``, ``status=claimed``, ``updated``
  bumped, written atomically **in place** (the task stays in ``tasks/open/`` —
  ``open``/``claimed`` both route there, so a claim never moves folders);
* already claimed by the same agent → no-op, no write (``updated`` unchanged),
  exit 0 (idempotent);
* claimed by a *different* agent → :class:`ClaimConflictError` (CLI exit 4).

The lock only serializes the read-modify-write; it is released after the write
and does **not** encode the claim, so the claim survives lock TTL expiry and
process exit (it lives in the frontmatter, not the lock file). The whole path
uses ``storage.locks.acquire`` + ``storage.files.atomic_write`` directly, so it
behaves identically with the daemon down. The concurrency test proves that under
a simultaneous N-thread race exactly one claimer wins.
"""

from __future__ import annotations

import json
import threading
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

import brain.cli.task as task_cli
import brain.core.tasks as tasks_core
import brain.storage.locks as locks_mod
from brain.cli.__main__ import app
from brain.core.tasks import (
    ClaimConflictError,
    TaskNotFoundError,
    claim_task,
)
from brain.schemas.config import Config, load_config
from brain.storage.files import task_folder

_OLD = datetime(2026, 1, 1, 9, 0, 0, tzinfo=UTC)


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


def _lock_path(vault: Path, task_id: str) -> Path:
    return vault / "tasks" / ".locks" / f"{task_id}.lock"


# --------------------------------------------------------------------------- #
# claim_task (core) — unclaimed → durable claim                                #
# --------------------------------------------------------------------------- #


def test_claim_unclaimed_writes_durable_claim(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, status="open", claimed_by=None)
    task = claim_task(cfg, "t-seed", "test-agent")
    assert task.claimed_by == "test-agent"
    assert task.status == "claimed"
    meta = _reload(path).metadata
    assert meta["claimed_by"] == "test-agent"
    assert meta["status"] == "claimed"
    assert meta["updated"] > _OLD  # bumped on the claiming write
    assert meta["created"] == _OLD  # birth instant untouched


def test_claim_stays_in_open_folder(cfg: Config, vault: Path) -> None:
    """open|claimed both route to tasks/open/ — a claim never moves the file."""
    _seed_task(vault, task_id="t-stay", status="open", claimed_by=None)
    claim_task(cfg, "t-stay", "test-agent")
    assert (task_folder("open", vault) / "t-stay.md").exists()
    assert list((vault / "tasks" / "done").glob("*.md")) == []


def test_claim_durable_survives_lock_release(cfg: Config, vault: Path) -> None:
    """The O_EXCL lock is released after the write; the claim lives in frontmatter."""
    path = _seed_task(vault, task_id="t-durable", status="open", claimed_by=None)
    claim_task(cfg, "t-durable", "test-agent")
    # Lock released (does not encode the claim)...
    assert not _lock_path(vault, "t-durable").exists()
    # ...but the claim persists on disk.
    assert _reload(path).metadata["claimed_by"] == "test-agent"


# --------------------------------------------------------------------------- #
# claim_task (core) — idempotent same-owner no-op                              #
# --------------------------------------------------------------------------- #


def test_claim_same_owner_is_noop(cfg: Config, vault: Path) -> None:
    path = _seed_task(
        vault,
        task_id="t-mine",
        status="claimed",
        owner="test-agent",
        claimed_by="test-agent",
        updated=_OLD,
    )
    task = claim_task(cfg, "t-mine", "test-agent")
    assert task.claimed_by == "test-agent"
    meta = _reload(path).metadata
    # No-op: nothing was rewritten, so ``updated`` is unchanged.
    assert meta["updated"] == _OLD
    assert meta["claimed_by"] == "test-agent"
    assert meta["status"] == "claimed"


def test_claim_same_owner_does_not_write(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A same-owner reclaim must not touch atomic_write (pure no-op)."""
    _seed_task(vault, task_id="t-mine", claimed_by="test-agent", status="claimed")
    calls: list[Path] = []
    monkeypatch.setattr(tasks_core, "atomic_write", lambda path, content: calls.append(path))
    claim_task(cfg, "t-mine", "test-agent")
    assert calls == []


# --------------------------------------------------------------------------- #
# claim_task (core) — conflict with a different owner                          #
# --------------------------------------------------------------------------- #


def test_claim_conflict_raises_with_existing_owner(cfg: Config, vault: Path) -> None:
    path = _seed_task(
        vault,
        task_id="t-taken",
        status="claimed",
        owner="other-agent",
        claimed_by="other-agent",
        updated=_OLD,
    )
    with pytest.raises(ClaimConflictError) as exc:
        claim_task(cfg, "t-taken", "test-agent")
    assert exc.value.existing_owner == "other-agent"
    # The losing attempt leaves the frontmatter untouched.
    meta = _reload(path).metadata
    assert meta["claimed_by"] == "other-agent"
    assert meta["updated"] == _OLD


def test_claim_conflict_does_not_write(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _seed_task(vault, task_id="t-taken", status="claimed", claimed_by="other-agent")
    calls: list[Path] = []
    monkeypatch.setattr(tasks_core, "atomic_write", lambda path, content: calls.append(path))
    with pytest.raises(ClaimConflictError):
        claim_task(cfg, "t-taken", "test-agent")
    assert calls == []


def test_claim_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here")
    with pytest.raises(TaskNotFoundError):
        claim_task(cfg, "t-nope", "test-agent")


# --------------------------------------------------------------------------- #
# claim_task (core) — terminal statuses are never claimable (idempotent no-op) #
# --------------------------------------------------------------------------- #


def test_claim_done_task_is_noop(cfg: Config, vault: Path) -> None:
    """A finished task (status=done, claimed_by=None) is never re-opened by a claim.

    ``finish_task`` never clears ``claimed_by``, so an open→done task carries
    ``claimed_by: None`` with ``status: done``. Without a terminal guard the claim
    would fall through and write ``status: claimed`` back into a file sitting in
    ``tasks/done/`` — a done→claimed transition the one-way lifecycle forbids. The
    guard makes it an idempotent no-op that leaves the file terminal.
    """
    path = _seed_task(vault, task_id="t-done", status="done", claimed_by=None)
    task = claim_task(cfg, "t-done", "test-agent")
    assert task.status == "done"
    assert task.claimed_by is None
    meta = _reload(path).metadata
    assert meta["status"] == "done"
    assert meta["claimed_by"] is None
    assert meta["updated"] == _OLD  # pure no-op: nothing rewritten
    # Stays terminal — never resurrected into tasks/open/.
    assert (task_folder("done", vault) / "t-done.md").exists()
    assert list((vault / "tasks" / "open").glob("*.md")) == []


def test_claim_cancelled_task_is_noop(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-cancelled", status="cancelled", claimed_by=None)
    task = claim_task(cfg, "t-cancelled", "test-agent")
    assert task.status == "cancelled"
    meta = _reload(path).metadata
    assert meta["status"] == "cancelled"
    assert meta["updated"] == _OLD


def test_claim_terminal_does_not_write(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A claim on a terminal task must not touch atomic_write (pure no-op)."""
    _seed_task(vault, task_id="t-done", status="done", claimed_by=None)
    calls: list[Path] = []
    monkeypatch.setattr(tasks_core, "atomic_write", lambda path, content: calls.append(path))
    claim_task(cfg, "t-done", "test-agent")
    assert calls == []


def test_claim_done_task_with_prior_claimer_does_not_conflict(cfg: Config, vault: Path) -> None:
    """A task finished after being claimed stays done on re-claim (no exit-4 conflict).

    The terminal guard runs before the ``claimed_by`` inspection, so a different
    agent claiming an already-finished task gets the idempotent terminal no-op —
    not a :class:`ClaimConflictError` that would mislead the caller into thinking
    the task is live.
    """
    path = _seed_task(
        vault, task_id="t-fin", status="done", owner="other-agent", claimed_by="other-agent"
    )
    task = claim_task(cfg, "t-fin", "test-agent")
    assert task.status == "done"
    assert task.claimed_by == "other-agent"
    assert _reload(path).metadata["updated"] == _OLD


# --------------------------------------------------------------------------- #
# claim_task (core) — resolves the path *inside* the entity lock (TOCTOU-safe) #
# --------------------------------------------------------------------------- #


def test_claim_resolves_inside_lock(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The lock is acquired *before* the path is resolved.

    A concurrent finish/cancel renames the file open→done while holding the same
    (id-derived, location-independent) entity lock. Resolving before acquiring the
    lock would open a TOCTOU window where this caller reads a path the winner just
    moved. Acquiring first closes it — verified by the acquire→resolve ordering.
    """
    _seed_task(vault, task_id="t-order", status="open", claimed_by=None)
    events: list[str] = []
    real_acquire = locks_mod.acquire
    real_resolve = tasks_core._resolve_task_path

    def spy_acquire(lock_path: Path):  # type: ignore[no-untyped-def]
        events.append("acquire")
        return real_acquire(lock_path)

    def spy_resolve(config: Config, task_id: str) -> Path:
        events.append("resolve")
        return real_resolve(config, task_id)

    monkeypatch.setattr(locks_mod, "acquire", spy_acquire)
    monkeypatch.setattr(tasks_core, "_resolve_task_path", spy_resolve)
    claim_task(cfg, "t-order", "test-agent")
    assert events.index("acquire") < events.index("resolve")


# --------------------------------------------------------------------------- #
# claim_task (core) — routes through the storage primitives (daemon-down)      #
# --------------------------------------------------------------------------- #


def test_claim_acquires_stable_entity_lock(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The lock lives at tasks/.locks/<id>.lock regardless of open/done subfolder."""
    seen: list[Path] = []
    real = locks_mod.acquire

    def spy(lock_path: Path):  # type: ignore[no-untyped-def]
        seen.append(lock_path)
        return real(lock_path)

    _seed_task(vault, task_id="t-lock", status="open", claimed_by=None)
    monkeypatch.setattr(locks_mod, "acquire", spy)
    claim_task(cfg, "t-lock", "test-agent")
    assert seen == [_lock_path(vault, "t-lock")]


def test_claim_uses_atomic_write(cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Claim writes via storage.atomic_write directly — no socket, works daemon-down."""
    path = _seed_task(vault, task_id="t-atomic", status="open", claimed_by=None)
    calls: list[Path] = []
    real = tasks_core.atomic_write

    def spy(target: Path, content: str) -> None:
        calls.append(target)
        real(target, content)

    monkeypatch.setattr(tasks_core, "atomic_write", spy)
    claim_task(cfg, "t-atomic", "test-agent")
    assert calls == [path]


# --------------------------------------------------------------------------- #
# claim_task (core) — concurrent race: exactly one winner                      #
# --------------------------------------------------------------------------- #


def test_claim_concurrent_single_winner(cfg: Config, vault: Path) -> None:
    n = 10
    path = _seed_task(vault, task_id="t-race", status="open", claimed_by=None)
    barrier = threading.Barrier(n)
    identities = [f"agent-{i}" for i in range(n)]

    def attempt(who: str) -> tuple[str, object]:
        barrier.wait()  # release all claimers simultaneously
        try:
            task = claim_task(cfg, "t-race", who)
            return ("ok", task.claimed_by)
        except ClaimConflictError as exc:
            return ("conflict", exc.existing_owner)

    with ThreadPoolExecutor(max_workers=n) as pool:
        results = list(pool.map(attempt, identities))

    winners = [claimed for status, claimed in results if status == "ok"]
    conflicts = [existing for status, existing in results if status == "conflict"]
    assert len(winners) == 1, results  # exactly one thread claimed
    assert len(conflicts) == n - 1
    sole_winner = winners[0]
    # Every loser saw the same, real winner; the file records that winner.
    assert set(conflicts) == {sole_winner}
    assert _reload(path).metadata["claimed_by"] == sole_winner


# --------------------------------------------------------------------------- #
# CLI — brain task claim                                                        #
# --------------------------------------------------------------------------- #


def test_cli_claim_success_exit_0(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, task_id="t-c7d1", status="open", claimed_by=None)
    result = _invoke(["task", "claim", "t-c7d1"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "claimed t-c7d1"
    meta = _reload(path).metadata
    assert meta["claimed_by"] == "test-agent"
    assert meta["status"] == "claimed"


def test_cli_claim_same_agent_reclaim_exit_0(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-c7d1", status="claimed", claimed_by="test-agent")
    result = _invoke(["task", "claim", "t-c7d1"])
    assert result.exit_code == 0, result.output


def test_cli_claim_taken_by_other_exits_4(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-c7d1", status="claimed", claimed_by="other-agent")
    result = _invoke(["task", "claim", "t-c7d1"])
    assert result.exit_code == 4, result.output


def test_cli_claim_not_found_exits_3(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here")
    result = _invoke(["task", "claim", "t-missing"])
    assert result.exit_code == 3, result.output


def test_cli_claim_quiet_emits_id_only(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-c7d1", status="open", claimed_by=None)
    result = _invoke(["--quiet", "task", "claim", "t-c7d1"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "t-c7d1"


def test_cli_claim_json_object(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-c7d1", status="open", claimed_by=None)
    result = _invoke(["--json", "task", "claim", "t-c7d1"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["id"] == "t-c7d1"
    assert obj["status"] == "claimed"
    assert "updated" in obj


def test_cli_claim_owner_flag_sets_claimer(cfg: Config, vault: Path) -> None:
    """The global --owner flag chooses the acting agent (claimed_by)."""
    path = _seed_task(vault, task_id="t-c7d1", status="open", claimed_by=None)
    result = _invoke(["--owner", "other-agent", "task", "claim", "t-c7d1"])
    assert result.exit_code == 0, result.output
    assert _reload(path).metadata["claimed_by"] == "other-agent"


def test_cli_claim_agentless_config_exits_2(
    vault: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """With no [core].agent and no --owner there is no claimer identity → exit 2."""
    _seed_task(vault, task_id="t-noagent", status="open", claimed_by=None)
    cfg_file = tmp_path / "noagent.toml"
    cfg_file.write_text(
        "\n".join(
            (
                "[core]",
                f'tolaria_path = "{vault}"',
                "",
                "[tasks]",
                "collections = []",
                "",
            )
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("BRAIN_CONFIG_PATH", str(cfg_file))
    monkeypatch.delenv("BRAIN_AGENT", raising=False)
    result = _invoke(["task", "claim", "t-noagent"])
    assert result.exit_code == 2, result.output


def test_claim_command_registered() -> None:
    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert "claim" in names
