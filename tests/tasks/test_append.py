"""team-awareness/2 — ``task append``: body append with no lifecycle change (R2).

A task body is otherwise write-once after creation (only ``finish``/``cancel``
ever touch it, and only once, terminally). ``append_task`` is the missing half:
it writes text into a task's body under the per-entity ``O_EXCL`` lock without
touching ``status`` or moving the file between ``tasks/open/`` and
``tasks/done/``. ``related`` is recomputed from the amended body exactly as
``append_note`` does, so a ``[[…]]`` mention in appended text becomes
discoverable through the inbound lens (``shards graph <id> --direction in``,
team-awareness/1). No second body-append implementation exists: ``append_task``
reuses ``core/notes.py``'s private block helpers directly.
"""

from __future__ import annotations

import json
import re
import threading
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime
from pathlib import Path
from typing import cast

import frontmatter
import pytest

import shards.core.tasks as tasks_core
import shards.storage.locks as locks_mod
from shards.cli.__main__ import app
from shards.core.context import graph_query
from shards.core.tasks import TaskNotFoundError, append_task, finish_task
from shards.schemas.config import Config, load_config
from shards.storage.files import note_folder, task_folder

_ISO_UTC = re.compile(r"\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\b")
_OLD = datetime(2026, 1, 1, 9, 0, 0, tzinfo=UTC)


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


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
    extra: dict[str, object] | None = None,
) -> Path:
    """Write a shards task straight to disk in the folder matching its status."""
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
    if extra:
        meta.update(extra)
    folder = task_folder(status, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def _seed_note(vault: Path, note_id: str, title: str = "Note") -> Path:
    """Write a minimal shards note straight to disk under notes/."""
    meta: dict[str, object] = {
        "id": note_id,
        "type": "note",
        "title": title,
        "tags": [],
        "owner": "seed-agent",
        "created": _OLD,
        "updated": _OLD,
        "related": [],
    }
    folder = note_folder("note", vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{note_id}.md"
    post = frontmatter.Post("Note body.")
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def _reload(path: Path) -> frontmatter.Post:
    return frontmatter.loads(path.read_text(encoding="utf-8"))


def _open_path(vault: Path, task_id: str = "t-seed") -> Path:
    return task_folder("open", vault) / f"{task_id}.md"


def _done_path(vault: Path, task_id: str = "t-seed") -> Path:
    return task_folder("done", vault) / f"{task_id}.md"


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    from typer.testing import CliRunner

    return CliRunner().invoke(app, args)


# --------------------------------------------------------------------------- #
# append_task (core) — no lifecycle change                                     #
# --------------------------------------------------------------------------- #


def test_append_claimed_task_leaves_status_and_folder(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, status="claimed", claimed_by="flights-agent")
    task = append_task(cfg, "t-seed", "blocked on progress")

    assert task.status == "claimed"
    assert _open_path(vault).exists()
    assert not _done_path(vault).exists()

    reloaded = _reload(path)
    assert reloaded.metadata["status"] == "claimed"
    assert reloaded.metadata["claimed_by"] == "flights-agent"
    assert "blocked on progress" in reloaded.content
    assert "Task body." in reloaded.content  # original body preserved
    assert cast(datetime, reloaded.metadata["updated"]) > _OLD  # bumped
    assert reloaded.metadata["created"] == _OLD  # created untouched
    assert task.updated > _OLD


def test_append_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault)
    with pytest.raises(TaskNotFoundError):
        append_task(cfg, "t-missing", "x")


def _seed_malformed(vault: Path, task_id: str = "t-bad") -> Path:
    """Write a ``t-`` id file under tasks/open/ whose frontmatter is unparseable YAML."""
    folder = task_folder("open", vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text("---\ntitle: [unclosed\nstatus: open\n---\nbody\n", encoding="utf-8")
    return path


def test_append_malformed_yaml_raises_not_found(cfg: Config, vault: Path) -> None:
    """A resolved-but-unreadable task's content read maps to TaskNotFoundError."""
    _seed_malformed(vault, "t-bad")
    with pytest.raises(TaskNotFoundError):
        append_task(cfg, "t-bad", "x")


# --------------------------------------------------------------------------- #
# related — the R1 delivery path end-to-end                                    #
# --------------------------------------------------------------------------- #


def test_append_mention_lands_in_related(cfg: Config, vault: Path) -> None:
    _seed_task(vault, body="Task body.")
    _seed_note(vault, "n-FEWP", title="Flight FEWP")
    append_task(cfg, "t-seed", "blocked on [[n-FEWP]]")
    reloaded = _reload(_open_path(vault))
    assert reloaded.metadata["related"] == ["n-FEWP"]


def test_append_mention_is_discoverable_via_inbound_graph(cfg: Config, vault: Path) -> None:
    """The R1 + R2 payoff: an appended mention is delivered through --direction in."""
    _seed_task(vault, task_id="t-10NT", title="Book ANA flight")
    _seed_note(vault, "n-FEWP", title="Flight FEWP")
    append_task(cfg, "t-10NT", "blocked on [[n-FEWP]]")

    result = graph_query(cfg, "n-FEWP", depth=1, direction="in")
    assert "t-10NT" in result.ids
    assert ("t-10NT", "n-FEWP") in result.edges


# --------------------------------------------------------------------------- #
# Terminal tasks — append is allowed, no second Outcome/Cancelled section       #
# --------------------------------------------------------------------------- #


def test_append_done_task_writes_text_keeps_status_and_folder(cfg: Config, vault: Path) -> None:
    body = "Task body.\n\n## Outcome\n\n2026-01-01T09:00:00Z\nShipped it."
    path = _seed_task(vault, status="done", body=body)
    task = append_task(cfg, "t-seed", "post-mortem: latency spike at check-in")

    assert task.status == "done"
    assert _done_path(vault).exists()
    assert not _open_path(vault).exists()

    reloaded = _reload(path)
    assert reloaded.metadata["status"] == "done"
    assert "post-mortem: latency spike at check-in" in reloaded.content
    assert reloaded.content.count("## Outcome") == 1  # no second Outcome section


def test_append_cancelled_task_writes_text_no_second_cancelled_section(
    cfg: Config, vault: Path
) -> None:
    body = "Task body.\n\n## Cancelled\n\n2026-01-01T09:00:00Z\nNo longer needed."
    path = _seed_task(vault, status="cancelled", body=body)
    task = append_task(cfg, "t-seed", "closing note")

    assert task.status == "cancelled"
    assert _done_path(vault).exists()
    assert not _open_path(vault).exists()

    reloaded = _reload(path)
    assert reloaded.metadata["status"] == "cancelled"
    assert "closing note" in reloaded.content
    assert reloaded.content.count("## Cancelled") == 1


# --------------------------------------------------------------------------- #
# --section / --timestamp — mirrors note append flag-for-flag                  #
# --------------------------------------------------------------------------- #


def test_append_section_creates_heading_when_absent(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, body="Intro paragraph.")
    append_task(cfg, "t-seed", "new note", section="Follow-ups")
    content = _reload(path).content
    assert "## Follow-ups" in content
    assert content.index("Intro paragraph.") < content.index("## Follow-ups")
    assert content.index("## Follow-ups") < content.index("new note")


def test_append_section_appends_under_existing_heading(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, body="Intro.\n\n## Follow-ups\n\nfirst follow-up.")
    append_task(cfg, "t-seed", "second follow-up", section="Follow-ups")
    content = _reload(path).content
    assert content.count("## Follow-ups") == 1
    fu = content.index("## Follow-ups")
    first = content.index("first follow-up.")
    second = content.index("second follow-up")
    assert fu < first < second


def test_append_timestamp_prepends_iso_line(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault)
    append_task(cfg, "t-seed", "with clock", timestamp=True)
    content = _reload(path).content
    match = _ISO_UTC.search(content)
    assert match is not None
    assert match.start() < content.index("with clock")


def test_append_roundtrips_unknown_frontmatter_keys(cfg: Config, vault: Path) -> None:
    path = _seed_task(vault, extra={"tolaria_pinned": True, "custom_ref": "PROJ-1"})
    append_task(cfg, "t-seed", "x")
    meta = _reload(path).metadata
    assert meta["tolaria_pinned"] is True
    assert meta["custom_ref"] == "PROJ-1"


# --------------------------------------------------------------------------- #
# team-awareness/8 — the stamp names the editor, never the task's owner         #
# --------------------------------------------------------------------------- #


def _write_agent_config(tmp_path: Path, vault: Path, agent: str | None) -> Path:
    """Write a standalone ``config.toml`` identifying as ``agent`` (or with no
    ``[core].agent`` at all when ``agent`` is ``None``), pointed at ``vault``, so
    a test can hold two distinct-identity ``Config`` objects over one vault."""
    lines = ["[core]", f'tolaria_path = "{vault}"']
    if agent is not None:
        lines.append(f'agent = "{agent}"')
    path = tmp_path / f"{agent or 'noagent'}.toml"
    path.write_text("\n".join([*lines, ""]), encoding="utf-8")
    return path


def test_append_timestamp_names_the_editor_not_the_owner(
    vault: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """R8: tolaria-agent appends to a task owned by flights-agent; the stamp
    names the editor (tolaria-agent), never the task's ``owner``, and the ISO
    token stays the first field on the line."""
    path = _seed_task(vault, owner="flights-agent")
    cfg_file = _write_agent_config(tmp_path, vault, "tolaria-agent")
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(cfg_file))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    editor_cfg = load_config()

    append_task(editor_cfg, "t-seed", "appended by the editor", timestamp=True)
    content = _reload(path).content
    stamp_line = next(line for line in content.splitlines() if _ISO_UTC.search(line))
    match = _ISO_UTC.search(stamp_line)
    assert match is not None
    assert match.start() == 0
    assert stamp_line == f"{match.group(0)} — tolaria-agent"
    assert "flights-agent" not in stamp_line


def test_append_timestamp_unset_identity_is_bare_iso(
    vault: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """No ``[core].agent``/``$SHARDS_AGENT`` degrades to a bare ISO line — no
    stray trailing separator, no crash."""
    path = _seed_task(vault)
    cfg_file = _write_agent_config(tmp_path, vault, None)
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(cfg_file))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    noagent_cfg = load_config()
    assert noagent_cfg.agent is None

    append_task(noagent_cfg, "t-seed", "anonymous append", timestamp=True)
    content = _reload(path).content
    stamp_line = next(line for line in content.splitlines() if _ISO_UTC.search(line))
    match = _ISO_UTC.search(stamp_line)
    assert match is not None
    assert stamp_line == match.group(0)


# --------------------------------------------------------------------------- #
# Mechanics — atomic write, entity lock, no second implementation              #
# --------------------------------------------------------------------------- #


def test_append_uses_atomic_write(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _seed_task(vault)
    calls: list[Path] = []
    real = tasks_core.atomic_write

    def spy(path: Path, content: str) -> None:
        calls.append(path)
        real(path, content)

    monkeypatch.setattr(tasks_core, "atomic_write", spy)
    append_task(cfg, "t-seed", "x")
    assert calls, "append_task must route writes through storage.atomic_write"


def test_append_acquires_entity_lock(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    seen: list[Path] = []
    real = locks_mod.acquire

    def spy(lock_path: Path):  # type: ignore[no-untyped-def]
        seen.append(lock_path)
        return real(lock_path)

    _seed_task(vault)
    monkeypatch.setattr(locks_mod, "acquire", spy)
    append_task(cfg, "t-seed", "x")
    assert seen == [vault / "tasks" / ".locks" / "t-seed.lock"]


def test_append_task_imports_helpers_from_notes() -> None:
    """No second body-append implementation: tasks.py imports notes.py's helpers."""
    import shards.core.notes as notes_core

    assert tasks_core._append_to_end is notes_core._append_to_end
    assert tasks_core._append_under_section is notes_core._append_under_section
    assert tasks_core._format_block is notes_core._format_block


# --------------------------------------------------------------------------- #
# Concurrency — O_EXCL lock serializes append vs finish, no lost updates       #
# --------------------------------------------------------------------------- #


def test_concurrent_appends_all_land(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-race")
    n = 6
    barrier = threading.Barrier(n)

    def attempt(i: int) -> None:
        barrier.wait()
        append_task(cfg, "t-race", f"MARKER-{i}")

    with ThreadPoolExecutor(max_workers=n) as pool:
        list(pool.map(attempt, range(n)))

    content = _reload(_open_path(vault, "t-race")).content
    for i in range(n):
        assert f"MARKER-{i}" in content, f"lost update: MARKER-{i} missing"


def test_concurrent_append_and_finish_serialize_without_lost_write(
    cfg: Config, vault: Path
) -> None:
    """An append racing a finish on the same id: both effects survive.

    Whichever wins the lock first, the loser must not race a vanished/moved
    path: resolution happens *inside* the lock for both verbs. Regardless of
    interleaving the appended marker and the outcome section both end up in the
    single surviving (done/) file.
    """
    _seed_task(vault, task_id="t-race2", status="open", body="Body.")
    barrier = threading.Barrier(2)

    def do_append() -> None:
        barrier.wait()
        append_task(cfg, "t-race2", "APPEND-MARKER")

    def do_finish() -> None:
        barrier.wait()
        finish_task(cfg, "t-race2", "Shipped.")

    with ThreadPoolExecutor(max_workers=2) as pool:
        f1 = pool.submit(do_append)
        f2 = pool.submit(do_finish)
        f1.result(timeout=30)
        f2.result(timeout=30)

    # The file always ends up in done/ (finish wins the lifecycle regardless of
    # ordering); no file is stranded in open/, and neither write is lost.
    assert not _open_path(vault, "t-race2").exists()
    content = _reload(_done_path(vault, "t-race2")).content
    assert "APPEND-MARKER" in content
    assert "## Outcome" in content
    assert content.count("## Outcome") == 1


# --------------------------------------------------------------------------- #
# CLI — shards task append                                                     #
# --------------------------------------------------------------------------- #


def test_cli_append_success(shards_config: Path, vault: Path) -> None:
    path = _seed_task(vault, status="claimed", claimed_by="flights-agent")
    result = _invoke(["task", "append", "t-seed", "cli text"])
    assert result.exit_code == 0, result.output
    reloaded = _reload(path)
    assert "cli text" in reloaded.content
    assert reloaded.metadata["status"] == "claimed"


def test_cli_append_not_found_exits_3(shards_config: Path, vault: Path) -> None:
    _seed_task(vault)
    result = _invoke(["task", "append", "t-missing", "x"])
    assert result.exit_code == 3


def test_cli_append_section_and_timestamp(shards_config: Path, vault: Path) -> None:
    path = _seed_task(vault)
    result = _invoke(
        ["task", "append", "t-seed", "logged", "--section", "Follow-ups", "--timestamp"]
    )
    assert result.exit_code == 0, result.output
    content = _reload(path).content
    assert "## Follow-ups" in content
    assert _ISO_UTC.search(content) is not None
    assert "logged" in content


def test_cli_append_quiet_emits_id_only(shards_config: Path, vault: Path) -> None:
    _seed_task(vault)
    result = _invoke(["--quiet", "task", "append", "t-seed", "x"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "t-seed"


def test_cli_append_json_object(shards_config: Path, vault: Path) -> None:
    _seed_task(vault, status="claimed", claimed_by="flights-agent")
    result = _invoke(["--json", "task", "append", "t-seed", "x"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["id"] == "t-seed"
    assert obj["status"] == "claimed"


def test_append_command_registered() -> None:
    import shards.cli.task as task_cli

    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert "append" in names
