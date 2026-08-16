"""tasks/5 — list / get / cancel (R4, R5).

Exercises the task read verbs and the cancel lifecycle transition:

* :func:`shards.core.tasks.list_tasks` scans **both** ``tasks/open/`` and
  ``tasks/done/``, surfaces only files whose frontmatter validates as a
  :class:`~shards.schemas.task.Task` (``t-`` id, ``type: task``), and applies
  conjunctive ``status`` / ``owner`` / ``--mine`` / tag / ``--since`` filters with
  ``--sort`` and ``--limit`` (same semantics as notes).
* :func:`shards.core.tasks.get_task` reads one task by id from either folder into a
  :class:`~shards.core.tasks.TaskView`; not-found raises (CLI exit 3).
* :func:`shards.core.tasks.cancel_task` appends a ``## Cancelled`` section (ISO-8601
  timestamp + optional reason), sets ``status=cancelled``, bumps ``updated``, and
  moves the file to ``tasks/done/`` — all under the per-entity lock, idempotent on
  a terminal status.

Ordering tests seed *distinct* timestamps: Python's sort is stable, so ties fall
back to filesystem ``glob`` order and would be non-deterministic.
"""

from __future__ import annotations

import json
import re
from datetime import UTC, datetime, timedelta
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

import shards.cli.task as task_cli
from shards.cli.__main__ import app
from shards.core.tasks import (
    TaskNotFoundError,
    TaskView,
    _resolve_task_path,
    cancel_task,
    get_task,
    list_tasks,
)
from shards.schemas.config import Config, load_config
from shards.storage.files import task_folder

_OLD = datetime(2026, 1, 1, 9, 0, 0, tzinfo=UTC)
_ISO_UTC = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z")


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _reload(path: Path) -> frontmatter.Post:
    return frontmatter.loads(path.read_text(encoding="utf-8"))


def _now() -> datetime:
    return datetime.now(UTC)


def _seed_task(
    vault: Path,
    *,
    task_id: str = "t-seed",
    title: str = "Seed Task",
    status: str = "open",
    priority: str | None = None,
    owner: str | None = "seed-agent",
    claimed_by: str | None = None,
    tags: list[str] | None = None,
    body: str = "Task body.",
    created: datetime = _OLD,
    updated: datetime = _OLD,
) -> Path:
    """Write a shards task straight to disk in the folder matching its status."""
    meta: dict[str, object] = {
        "id": task_id,
        "type": "task",
        "title": title,
        "tags": list(tags or []),
        "owner": owner,
        "created": created,
        "updated": updated,
        "related": [],
        "status": status,
        "priority": priority,
        "claimed_by": claimed_by,
        "blocks": [],
        "blocked_by": [],
    }
    folder = task_folder(status, vault)
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


def _seed_foreign(vault: Path, sub: str, name: str, meta: dict[str, object] | None = None) -> Path:
    """Write a non-shards Markdown file (no valid ``t-`` id / ``type: task``)."""
    folder = vault / "tasks" / sub
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{name}.md"
    path.write_text(
        frontmatter.dumps(frontmatter.Post("Foreign.", **(meta or {}))), encoding="utf-8"
    )
    return path


def _seed_malformed(vault: Path, sub: str, task_id: str) -> Path:
    """Write a ``t-`` id file whose frontmatter is unparseable YAML.

    ``frontmatter.dumps`` only ever emits valid YAML, so the corruption is written
    raw: an unterminated flow sequence (``[unclosed``) makes PyYAML raise a
    ``ParserError`` (a ``yaml.YAMLError``) on load.
    """
    folder = vault / "tasks" / sub
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{task_id}.md"
    path.write_text("---\ntitle: [unclosed\nstatus: open\n---\nbody\n", encoding="utf-8")
    return path


def _open_path(vault: Path, task_id: str = "t-seed") -> Path:
    return task_folder("open", vault) / f"{task_id}.md"


def _done_path(vault: Path, task_id: str = "t-seed") -> Path:
    return task_folder("done", vault) / f"{task_id}.md"


# --------------------------------------------------------------------------- #
# list_tasks (core) — shards-id/type gate + scans both folders                  #
# --------------------------------------------------------------------------- #


def test_list_scans_both_open_and_done(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=1))
    ids = {v.task.id for v in list_tasks(cfg)}
    assert ids == {"t-open", "t-done"}


def test_list_returns_taskviews(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-one", body="Hello.")
    views = list_tasks(cfg)
    assert all(isinstance(v, TaskView) for v in views)
    assert views[0].task.id == "t-one"
    # core-hardening/5: a *list* row carries no body. The daemon's warm index
    # holds frontmatter only, so a list result that carried a body on disk would
    # silently empty whenever the daemon came up. ``get_task`` still returns one.
    assert views[0].body == ""


def test_list_skips_non_task_files(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-real", title="Real")
    _seed_foreign(vault, "open", "plain")  # no frontmatter
    _seed_foreign(vault, "open", "wrong-prefix", {"id": "x-123", "type": "task", "title": "X"})
    _seed_foreign(vault, "done", "no-id", {"type": "task", "title": "No Id"})
    ids = [v.task.id for v in list_tasks(cfg)]
    assert ids == ["t-real"]


# --------------------------------------------------------------------------- #
# list_tasks (core) — conjunctive filters                                      #
# --------------------------------------------------------------------------- #


def test_list_status_exact_match(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-claimed", status="claimed", updated=_now() - timedelta(minutes=1))
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=2))
    ids = {v.task.id for v in list_tasks(cfg, status="claimed")}
    assert ids == {"t-claimed"}


def test_list_owner_exact_match(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a", owner="alice", updated=_now())
    _seed_task(vault, task_id="t-b", owner="alicia", updated=_now() - timedelta(minutes=1))
    ids = {v.task.id for v in list_tasks(cfg, owner="alice")}
    assert ids == {"t-a"}


def test_list_mine_matches_owner_or_claimed_by(cfg: Config, vault: Path) -> None:
    # config agent (conftest) == "test-agent".
    _seed_task(vault, task_id="t-owned", owner="test-agent", status="open", updated=_now())
    _seed_task(
        vault,
        task_id="t-claimed",
        owner="someone",
        claimed_by="test-agent",
        status="claimed",
        updated=_now() - timedelta(minutes=1),
    )
    _seed_task(
        vault,
        task_id="t-other",
        owner="other-agent",
        claimed_by="other-agent",
        status="claimed",
        updated=_now() - timedelta(minutes=2),
    )
    ids = {v.task.id for v in list_tasks(cfg, mine=True)}
    assert ids == {"t-owned", "t-claimed"}


def test_list_mine_and_status_conjunctive(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-owned", owner="test-agent", status="open", updated=_now())
    _seed_task(
        vault,
        task_id="t-claimed",
        owner="someone",
        claimed_by="test-agent",
        status="claimed",
        updated=_now() - timedelta(minutes=1),
    )
    ids = {v.task.id for v in list_tasks(cfg, mine=True, status="claimed")}
    assert ids == {"t-claimed"}


def test_list_tags_and_semantics(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-both", tags=["ndc", "flights"], updated=_now())
    _seed_task(vault, task_id="t-one", tags=["ndc"], updated=_now() - timedelta(minutes=1))
    ids = {v.task.id for v in list_tasks(cfg, tags=["ndc", "flights"])}
    assert ids == {"t-both"}


def test_list_tags_any_semantics(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-both", tags=["ndc", "flights"], updated=_now())
    _seed_task(vault, task_id="t-one", tags=["ndc"], updated=_now() - timedelta(minutes=1))
    _seed_task(vault, task_id="t-none", tags=["misc"], updated=_now() - timedelta(minutes=2))
    ids = {v.task.id for v in list_tasks(cfg, tags=["ndc", "flights"], any_tag=True)}
    assert ids == {"t-both", "t-one"}


def test_list_since_duration_days(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-recent", updated=_now() - timedelta(days=1))
    _seed_task(vault, task_id="t-old", updated=_now() - timedelta(days=30))
    ids = {v.task.id for v in list_tasks(cfg, since="7d")}
    assert ids == {"t-recent"}


# --------------------------------------------------------------------------- #
# list_tasks (core) — --stale (team-awareness/4): the inverse of --since        #
# --------------------------------------------------------------------------- #


def test_stale_is_the_inverse_of_since_on_one_fixture(cfg: Config, vault: Path) -> None:
    """The load-bearing inversion: one fixture, opposite outcomes.

    A task claimed and idle for four days is returned by ``--stale 2d`` and
    hidden by ``--since 2d`` — proving the two are genuine inverses over the same
    ``updated`` field rather than two filters that happen to both pass on
    different data.
    """
    _seed_task(
        vault,
        task_id="t-idle",
        status="claimed",
        claimed_by="agent-a",
        updated=_now() - timedelta(days=4),
    )
    assert {v.task.id for v in list_tasks(cfg, stale="2d")} == {"t-idle"}
    assert {v.task.id for v in list_tasks(cfg, since="2d")} == set()


def test_stale_excludes_a_recently_touched_task(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-fresh", updated=_now() - timedelta(hours=1))
    assert {v.task.id for v in list_tasks(cfg, stale="2d")} == set()


def test_stale_and_since_combined_yield_a_band(cfg: Config, vault: Path) -> None:
    """Combined, ``--since``/``--stale`` band-pass: not fresher, not older."""
    _seed_task(vault, task_id="t-fresh", updated=_now() - timedelta(hours=1))
    _seed_task(vault, task_id="t-band", updated=_now() - timedelta(days=4))
    _seed_task(vault, task_id="t-ancient", updated=_now() - timedelta(days=10))
    ids = {v.task.id for v in list_tasks(cfg, since="7d", stale="2d")}
    assert ids == {"t-band"}


def test_stale_does_not_imply_a_status(cfg: Config, vault: Path) -> None:
    """``--stale`` alone filters purely on recency — statuses are untouched."""
    _seed_task(vault, task_id="t-open-old", status="open", updated=_now() - timedelta(days=5))
    _seed_task(vault, task_id="t-done-old", status="done", updated=_now() - timedelta(days=5))
    ids = {v.task.id for v in list_tasks(cfg, stale="2d")}
    assert ids == {"t-open-old", "t-done-old"}


def test_invalid_stale_duration_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a")
    with pytest.raises(ValueError):
        list_tasks(cfg, stale="bogus")


# --------------------------------------------------------------------------- #
# list_tasks (core) — --status CSV (team-awareness/4): union, not exact-match   #
# --------------------------------------------------------------------------- #


def test_status_csv_returns_the_union(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-claimed", status="claimed", updated=_now() - timedelta(minutes=1))
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=2))
    ids = {v.task.id for v in list_tasks(cfg, status="open,claimed")}
    assert ids == {"t-open", "t-claimed"}


def test_status_single_value_still_exact_match(cfg: Config, vault: Path) -> None:
    """A single ``--status`` value behaves exactly as before the CSV change."""
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-claimed", status="claimed", updated=_now() - timedelta(minutes=1))
    ids = {v.task.id for v in list_tasks(cfg, status="open")}
    assert ids == {"t-open"}


def test_status_csv_unknown_status_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a")
    with pytest.raises(ValueError):
        list_tasks(cfg, status="open,bogus")


# --------------------------------------------------------------------------- #
# list_tasks (core) — sort / limit                                             #
# --------------------------------------------------------------------------- #


def test_list_default_sort_updated_desc(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-mid", updated=_now() - timedelta(hours=2))
    _seed_task(vault, task_id="t-new", updated=_now())
    _seed_task(vault, task_id="t-old", updated=_now() - timedelta(hours=5))
    ids = [v.task.id for v in list_tasks(cfg)]
    assert ids == ["t-new", "t-mid", "t-old"]


def test_list_sort_created_desc(cfg: Config, vault: Path) -> None:
    base = datetime(2026, 1, 1, 0, 0, tzinfo=UTC)
    _seed_task(vault, task_id="t-first", created=base, updated=base)
    _seed_task(vault, task_id="t-second", created=base + timedelta(days=1), updated=base)
    _seed_task(vault, task_id="t-third", created=base + timedelta(days=2), updated=base)
    ids = [v.task.id for v in list_tasks(cfg, sort="created")]
    assert ids == ["t-third", "t-second", "t-first"]


def test_list_sort_title_asc(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-b", title="Bravo", updated=_now())
    _seed_task(vault, task_id="t-a", title="Alpha", updated=_now() - timedelta(minutes=1))
    _seed_task(vault, task_id="t-c", title="Charlie", updated=_now() - timedelta(minutes=2))
    titles = [v.task.title for v in list_tasks(cfg, sort="title")]
    assert titles == ["Alpha", "Bravo", "Charlie"]


def test_list_invalid_sort_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a")
    with pytest.raises(ValueError):
        list_tasks(cfg, sort="bogus")


# --------------------------------------------------------------------------- #
# list_tasks (core) — --sort priority (team-awareness/5)                       #
# --------------------------------------------------------------------------- #


def test_list_sort_priority_orders_high_normal_low_unprioritized(cfg: Config, vault: Path) -> None:
    base = datetime(2026, 1, 1, 0, 0, tzinfo=UTC)
    _seed_task(vault, task_id="t-low", priority="low", created=base, updated=base)
    _seed_task(
        vault, task_id="t-high", priority="high", created=base + timedelta(minutes=1), updated=base
    )
    _seed_task(
        vault,
        task_id="t-normal",
        priority="normal",
        created=base + timedelta(minutes=2),
        updated=base,
    )
    _seed_task(
        vault, task_id="t-none", priority=None, created=base + timedelta(minutes=3), updated=base
    )
    ids = [v.task.id for v in list_tasks(cfg, sort="priority")]
    assert ids == ["t-high", "t-normal", "t-low", "t-none"]


def test_list_sort_priority_ties_broken_by_created_ascending(cfg: Config, vault: Path) -> None:
    """FIFO within a rank: the older task (by ``created``) sorts first."""
    base = datetime(2026, 1, 1, 0, 0, tzinfo=UTC)
    _seed_task(
        vault,
        task_id="t-high-newer",
        priority="high",
        created=base + timedelta(days=1),
        updated=base,
    )
    _seed_task(vault, task_id="t-high-older", priority="high", created=base, updated=base)
    ids = [v.task.id for v in list_tasks(cfg, sort="priority")]
    assert ids == ["t-high-older", "t-high-newer"]


def test_list_task_with_garbage_priority_still_appears_and_sorts_last(
    cfg: Config, vault: Path
) -> None:
    """The load-bearing tolerant-read guarantee (R5).

    A pre-existing task whose ``priority`` is a free-form string outside the
    write-boundary vocabulary must never make ``list_tasks`` skip the row — that
    is exactly the silent-vanish failure the spec's tolerant-read decision exists
    to prevent. It shares the trailing "unprioritized" rank with an absent
    priority (tie-broken by ``created`` ascending, so the older legacy task
    sorts first within that shared bucket) and its on-disk value is never
    rewritten by a read.
    """
    base = datetime(2026, 1, 1, 0, 0, tzinfo=UTC)
    legacy_path = _seed_task(
        vault, task_id="t-legacy", priority="urgent-ish", created=base, updated=base
    )
    _seed_task(
        vault, task_id="t-high", priority="high", created=base + timedelta(minutes=1), updated=base
    )
    _seed_task(
        vault, task_id="t-none", priority=None, created=base + timedelta(minutes=2), updated=base
    )

    before = _reload(legacy_path).metadata
    ids = [v.task.id for v in list_tasks(cfg, sort="priority")]
    after = _reload(legacy_path).metadata

    assert "t-legacy" in ids  # never silently dropped
    assert ids == ["t-high", "t-legacy", "t-none"]
    assert after == before  # a read never mutates the file
    assert after["priority"] == "urgent-ish"  # round-trips untouched


# --------------------------------------------------------------------------- #
# list_tasks (core) — --available (team-awareness/5)                           #
# --------------------------------------------------------------------------- #


def test_available_excludes_claimed_and_stale_claimed_by(cfg: Config, vault: Path) -> None:
    """``--available`` = ``status == open and claimed_by is None``.

    Excludes a genuinely claimed task *and* a hand-edited ``open`` file that
    still carries a stale ``claimed_by`` — the difference from ``--status open``
    the spec calls out.
    """
    _seed_task(vault, task_id="t-free", status="open", claimed_by=None, updated=_now())
    _seed_task(
        vault,
        task_id="t-claimed",
        status="claimed",
        claimed_by="agent-a",
        updated=_now() - timedelta(minutes=1),
    )
    _seed_task(
        vault,
        task_id="t-stale-open",
        status="open",
        claimed_by="agent-b",
        updated=_now() - timedelta(minutes=2),
    )
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=3))
    ids = {v.task.id for v in list_tasks(cfg, available=True)}
    assert ids == {"t-free"}


def test_available_is_independent_of_owner(cfg: Config, vault: Path) -> None:
    """The takeable pool is defined by ``claimed_by``, not ``owner`` — no
    unowned state exists, and ``--available`` never filters on identity."""
    _seed_task(vault, task_id="t-a", owner="alice", status="open", updated=_now())
    _seed_task(
        vault, task_id="t-b", owner="bob", status="open", updated=_now() - timedelta(minutes=1)
    )
    ids = {v.task.id for v in list_tasks(cfg, available=True)}
    assert ids == {"t-a", "t-b"}


def test_available_composes_with_other_filters(cfg: Config, vault: Path) -> None:
    """``--available`` is conjunctive, exactly like every other filter."""
    _seed_task(vault, task_id="t-tagged", status="open", tags=["urgent"], updated=_now())
    _seed_task(vault, task_id="t-untagged", status="open", updated=_now() - timedelta(minutes=1))
    _seed_task(
        vault,
        task_id="t-tagged-claimed",
        status="claimed",
        claimed_by="agent-a",
        tags=["urgent"],
        updated=_now() - timedelta(minutes=2),
    )
    ids = {v.task.id for v in list_tasks(cfg, available=True, tags=["urgent"])}
    assert ids == {"t-tagged"}


def test_available_default_when_no_takeable_work(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-claimed", status="claimed", claimed_by="agent-a")
    assert list_tasks(cfg, available=True) == []


def test_list_limit_caps_results(cfg: Config, vault: Path) -> None:
    for i in range(5):
        _seed_task(vault, task_id=f"t-{i:02d}", updated=_now() - timedelta(minutes=i))
    assert len(list_tasks(cfg, limit=3)) == 3


def test_list_default_limit_is_20(cfg: Config, vault: Path) -> None:
    for i in range(25):
        _seed_task(vault, task_id=f"t-{i:02d}", updated=_now() - timedelta(minutes=i))
    assert len(list_tasks(cfg)) == 20


# --------------------------------------------------------------------------- #
# get_task (core)                                                              #
# --------------------------------------------------------------------------- #


def test_get_task_returns_view(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-seed", title="Seed", body="Hello world.")
    view = get_task(cfg, "t-seed")
    assert isinstance(view, TaskView)
    assert view.task.id == "t-seed"
    assert view.task.title == "Seed"
    assert view.body == "Hello world."


def test_get_task_resolves_from_done(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-done", status="done", body="Done body.")
    view = get_task(cfg, "t-done")
    assert view.task.status == "done"
    assert view.path == _done_path(vault, "t-done").resolve()


def test_get_task_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here")
    with pytest.raises(TaskNotFoundError):
        get_task(cfg, "t-missing")


def test_get_task_malformed_yaml_raises_not_found(cfg: Config, vault: Path) -> None:
    """A ``t-`` id file with unparseable frontmatter is treated as not-found.

    The stem still resolves (resolution is id-only and never reads the body), but
    the read must not crash with a bare ``yaml.YAMLError`` traceback — it surfaces
    as :class:`TaskNotFoundError` (CLI exit 3), matching the docstring contract.
    """
    _seed_malformed(vault, "open", "t-bad")
    with pytest.raises(TaskNotFoundError):
        get_task(cfg, "t-bad")


# --------------------------------------------------------------------------- #
# list_tasks (core) — malformed YAML is skipped silently                       #
# --------------------------------------------------------------------------- #


def test_list_skips_malformed_yaml(cfg: Config, vault: Path) -> None:
    """A file with unparseable frontmatter is skipped silently, never crashing."""
    _seed_task(vault, task_id="t-good", updated=_now())
    _seed_malformed(vault, "open", "t-bad")
    _seed_malformed(vault, "done", "t-bad2")
    ids = [v.task.id for v in list_tasks(cfg)]
    assert ids == ["t-good"]


# --------------------------------------------------------------------------- #
# cancel_task (core) — append ## Cancelled + atomic move to done/              #
# --------------------------------------------------------------------------- #


def test_cancel_open_appends_section_and_moves(cfg: Config, vault: Path) -> None:
    _seed_task(vault, status="open", body="Original body.")
    task = cancel_task(cfg, "t-seed", "not needed")

    assert task.status == "cancelled"
    # The file moved: gone from open/, present in done/.
    assert not _open_path(vault).exists()
    assert _done_path(vault).exists()

    post = _reload(_done_path(vault))
    assert post.metadata["status"] == "cancelled"
    assert post.metadata["updated"] > _OLD  # bumped on the cancelling write
    assert post.metadata["created"] == _OLD  # birth instant untouched
    assert "Original body." in post.content  # body preserved
    assert "## Cancelled" in post.content
    assert _ISO_UTC.search(post.content) is not None
    assert "not needed" in post.content
    assert post.content.index("## Cancelled") < post.content.index("not needed")


def test_cancel_without_reason_still_appends_header_and_timestamp(cfg: Config, vault: Path) -> None:
    _seed_task(vault, status="open")
    task = cancel_task(cfg, "t-seed", None)
    assert task.status == "cancelled"
    content = _reload(_done_path(vault)).content
    assert "## Cancelled" in content
    assert _ISO_UTC.search(content) is not None


def test_cancel_timestamp_line_precedes_reason(cfg: Config, vault: Path) -> None:
    _seed_task(vault, status="open")
    cancel_task(cfg, "t-seed", "superseded")
    content = _reload(_done_path(vault)).content
    match = _ISO_UTC.search(content)
    assert match is not None
    assert match.start() < content.index("superseded")


def test_cancel_already_cancelled_is_idempotent(cfg: Config, vault: Path) -> None:
    body = "Task body.\n\n## Cancelled\n\n2026-01-01T09:00:00Z\nFirst reason."
    _seed_task(vault, status="cancelled", body=body, updated=_OLD)
    task = cancel_task(cfg, "t-seed", "Second reason.")

    assert task.status == "cancelled"
    assert _done_path(vault).exists()
    assert not _open_path(vault).exists()

    post = _reload(_done_path(vault))
    # No second '## Cancelled' section; no re-write (updated unchanged).
    assert post.content.count("## Cancelled") == 1
    assert "Second reason." not in post.content
    assert post.metadata["updated"] == _OLD


def test_cancel_already_cancelled_does_not_write(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """An already-cancelled cancel must not touch atomic_write (pure no-op)."""
    import shards.core.tasks as tasks_core

    _seed_task(vault, status="cancelled", body="Task body.\n\n## Cancelled\n\nx")
    calls: list[Path] = []
    monkeypatch.setattr(tasks_core, "atomic_write", lambda path, content: calls.append(path))
    cancel_task(cfg, "t-seed", "ignored")
    assert calls == []


def test_cancel_done_task_is_noop(cfg: Config, vault: Path) -> None:
    """Terminal 'done' never transitions to cancelled (terminal re-run = no-op)."""
    _seed_task(vault, status="done", body="Task body.\n\n## Outcome\n\nshipped")
    task = cancel_task(cfg, "t-seed", "too late")
    assert task.status == "done"
    post = _reload(_done_path(vault))
    assert post.metadata["status"] == "done"
    assert "## Cancelled" not in post.content


def test_cancel_not_found_raises(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here")
    with pytest.raises(TaskNotFoundError):
        cancel_task(cfg, "t-nope", "reason")


def test_cancel_id_reresolves_from_done(cfg: Config, vault: Path) -> None:
    _seed_task(vault, status="open")
    cancel_task(cfg, "t-seed", "reason")
    resolved = _resolve_task_path(cfg, "t-seed")
    assert resolved == _done_path(vault).resolve()


# --------------------------------------------------------------------------- #
# team-awareness/8 — ## Cancelled names the acting agent                        #
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


def test_cancel_reason_names_the_acting_agent(
    vault: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """R8: a task owned by flights-agent, cancelled by tolaria-agent — the
    ``## Cancelled`` stamp names the canceller, never the task's ``owner``."""
    _seed_task(vault, status="open", owner="flights-agent")
    cfg_file = _write_agent_config(tmp_path, vault, "tolaria-agent")
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(cfg_file))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    canceller_cfg = load_config()

    cancel_task(canceller_cfg, "t-seed", "not needed")
    content = _reload(_done_path(vault)).content
    stamp_line = next(line for line in content.splitlines() if _ISO_UTC.search(line))
    match = _ISO_UTC.search(stamp_line)
    assert match is not None
    assert match.start() == 0
    assert stamp_line == f"{match.group(0)} — tolaria-agent"
    assert "flights-agent" not in stamp_line


def test_cancel_unset_identity_stamp_is_bare_iso(
    vault: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """No ``[core].agent``/``$SHARDS_AGENT`` degrades to a bare ISO line — no
    stray trailing separator, no crash."""
    _seed_task(vault, status="open")
    cfg_file = _write_agent_config(tmp_path, vault, None)
    monkeypatch.setenv("SHARDS_CONFIG_PATH", str(cfg_file))
    monkeypatch.delenv("SHARDS_AGENT", raising=False)
    noagent_cfg = load_config()
    assert noagent_cfg.agent is None

    cancel_task(noagent_cfg, "t-seed", "not needed")
    content = _reload(_done_path(vault)).content
    stamp_line = next(line for line in content.splitlines() if _ISO_UTC.search(line))
    match = _ISO_UTC.search(stamp_line)
    assert match is not None
    assert stamp_line == match.group(0)


# --------------------------------------------------------------------------- #
# CLI — shards task list                                                        #
# --------------------------------------------------------------------------- #


def test_cli_list_no_filters_returns_all(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=1))
    result = _invoke(["--quiet", "task", "list"])
    assert result.exit_code == 0, result.output
    assert set(result.output.split()) == {"t-open", "t-done"}


def test_cli_list_default_limit_caps(cfg: Config, vault: Path) -> None:
    for i in range(25):
        _seed_task(vault, task_id=f"t-{i:02d}", updated=_now() - timedelta(minutes=i))
    result = _invoke(["--quiet", "task", "list"])
    assert result.exit_code == 0, result.output
    assert len(result.output.split()) == 20


def test_cli_list_mine_status_json_is_array(cfg: Config, vault: Path) -> None:
    """Acceptance: shards task list --mine --status claimed --json → JSON array of tasks."""
    _seed_task(vault, task_id="t-owned", owner="test-agent", status="open", updated=_now())
    _seed_task(
        vault,
        task_id="t-mine",
        owner="someone",
        claimed_by="test-agent",
        status="claimed",
        updated=_now() - timedelta(minutes=1),
    )
    _seed_task(
        vault,
        task_id="t-other",
        owner="other-agent",
        claimed_by="other-agent",
        status="claimed",
        updated=_now() - timedelta(minutes=2),
    )
    result = _invoke(["--json", "task", "list", "--mine", "--status", "claimed"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.output)
    assert isinstance(arr, list)
    assert [o["id"] for o in arr] == ["t-mine"]
    assert arr[0]["status"] == "claimed"


def test_cli_list_quiet_one_id_per_line(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a", updated=_now())
    _seed_task(vault, task_id="t-b", updated=_now() - timedelta(minutes=1))
    result = _invoke(["--quiet", "task", "list"])
    assert result.output.split() == ["t-a", "t-b"]


def test_cli_list_status_filter(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=1))
    result = _invoke(["--quiet", "task", "list", "--status", "done"])
    assert result.output.split() == ["t-done"]


def test_cli_list_invalid_sort_exits_2(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a")
    result = _invoke(["task", "list", "--sort", "bogus"])
    assert result.exit_code == 2


def test_cli_list_status_csv_returns_union(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-claimed", status="claimed", updated=_now() - timedelta(minutes=1))
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=2))
    result = _invoke(["--quiet", "task", "list", "--status", "open,claimed"])
    assert result.exit_code == 0, result.output
    assert set(result.output.split()) == {"t-open", "t-claimed"}


def test_cli_list_status_csv_unknown_exits_2(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a")
    result = _invoke(["task", "list", "--status", "open,bogus"])
    assert result.exit_code == 2, result.output
    assert "Traceback" not in result.output


def test_cli_list_stale_is_inverse_of_since(cfg: Config, vault: Path) -> None:
    _seed_task(
        vault,
        task_id="t-idle",
        status="claimed",
        claimed_by="agent-a",
        updated=_now() - timedelta(days=4),
    )
    stale_result = _invoke(["--quiet", "task", "list", "--stale", "2d"])
    since_result = _invoke(["--quiet", "task", "list", "--since", "2d"])
    assert stale_result.output.split() == ["t-idle"]
    assert since_result.output.split() == []


def test_cli_list_invalid_stale_exits_2(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-a")
    result = _invoke(["task", "list", "--stale", "bogus"])
    assert result.exit_code == 2, result.output
    assert "Traceback" not in result.output


# --------------------------------------------------------------------------- #
# CLI — shards task list --available (team-awareness/5)                        #
# --------------------------------------------------------------------------- #


def test_cli_list_available_excludes_claimed(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-free", status="open", updated=_now())
    _seed_task(
        vault,
        task_id="t-claimed",
        status="claimed",
        claimed_by="agent-a",
        updated=_now() - timedelta(minutes=1),
    )
    result = _invoke(["--quiet", "task", "list", "--available"])
    assert result.exit_code == 0, result.output
    assert result.output.split() == ["t-free"]


def test_cli_list_available_excludes_stale_claimed_by_on_open_status(
    cfg: Config, vault: Path
) -> None:
    _seed_task(vault, task_id="t-free", status="open", updated=_now())
    _seed_task(
        vault,
        task_id="t-stale",
        status="open",
        claimed_by="agent-b",
        updated=_now() - timedelta(minutes=1),
    )
    result = _invoke(["--quiet", "task", "list", "--available"])
    assert result.output.split() == ["t-free"]


def test_cli_list_available_defaults_to_priority_sort(cfg: Config, vault: Path) -> None:
    base = datetime(2026, 1, 1, 0, 0, tzinfo=UTC)
    _seed_task(vault, task_id="t-low", status="open", priority="low", created=base, updated=base)
    _seed_task(
        vault,
        task_id="t-high",
        status="open",
        priority="high",
        created=base + timedelta(minutes=1),
        updated=base,
    )
    result = _invoke(["--quiet", "task", "list", "--available"])
    assert result.exit_code == 0, result.output
    assert result.output.split() == ["t-high", "t-low"]


def test_cli_list_available_explicit_sort_overrides_the_priority_default(
    cfg: Config, vault: Path
) -> None:
    _seed_task(
        vault, task_id="t-old", status="open", priority="low", updated=_now() - timedelta(minutes=5)
    )
    _seed_task(vault, task_id="t-new", status="open", priority="high", updated=_now())
    result = _invoke(["--quiet", "task", "list", "--available", "--sort", "updated"])
    assert result.exit_code == 0, result.output
    assert result.output.split() == ["t-new", "t-old"]


def test_cli_list_available_composes_with_status_filter(cfg: Config, vault: Path) -> None:
    """``--available`` and ``--status`` are conjunctive, like every other filter pair."""
    _seed_task(vault, task_id="t-open", status="open", updated=_now())
    _seed_task(vault, task_id="t-done", status="done", updated=_now() - timedelta(minutes=1))
    result = _invoke(["--quiet", "task", "list", "--available", "--status", "open"])
    assert result.output.split() == ["t-open"]


# --------------------------------------------------------------------------- #
# CLI — shards task list human text rows carry claimed_by (team-awareness/4)    #
# --------------------------------------------------------------------------- #


def test_cli_list_text_row_shows_claimed_by(cfg: Config, vault: Path) -> None:
    _seed_task(
        vault,
        task_id="t-held",
        title="Held Task",
        status="claimed",
        claimed_by="agent-a",
        updated=_now(),
    )
    result = _invoke(["task", "list"])
    assert result.exit_code == 0, result.output
    line = result.output.strip()
    assert line.split("\t") == ["t-held", "claimed", "agent-a", "Held Task"]


def test_cli_list_text_row_shows_dash_when_unclaimed(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-open", title="Open Task", status="open", updated=_now())
    result = _invoke(["task", "list"])
    assert result.exit_code == 0, result.output
    line = result.output.strip()
    assert line.split("\t") == ["t-open", "open", "-", "Open Task"]


def test_cli_list_json_output_unchanged_by_the_row_format_change(cfg: Config, vault: Path) -> None:
    """The text-row change (id/status/claimed_by/title) never touches --json."""
    _seed_task(vault, task_id="t-held", status="claimed", claimed_by="agent-a", updated=_now())
    result = _invoke(["--json", "task", "list"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.output)
    assert arr[0]["claimed_by"] == "agent-a"
    assert set(arr[0]) >= {"id", "status", "claimed_by", "title", "owner"}


def test_list_command_registered() -> None:
    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert "list" in names


# --------------------------------------------------------------------------- #
# CLI — shards task get                                                         #
# --------------------------------------------------------------------------- #


def test_cli_get_default_preview_truncates_at_200(cfg: Config, vault: Path) -> None:
    body = "A" * 250
    _seed_task(vault, task_id="t-seed", title="Seed", body=body)
    result = _invoke(["task", "get", "t-seed"])
    assert result.exit_code == 0, result.output
    assert "id: t-seed" in result.output
    assert "A" * 200 in result.output
    assert "A" * 201 not in result.output


def test_cli_get_full_shows_whole_body(cfg: Config, vault: Path) -> None:
    body = "A" * 250
    _seed_task(vault, task_id="t-seed", body=body)
    result = _invoke(["task", "get", "t-seed", "--full"])
    assert result.exit_code == 0, result.output
    assert "A" * 250 in result.output


def test_cli_get_json_is_full_model_dump(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-seed", title="Seed", tags=["x"], status="claimed", body="the body")
    result = _invoke(["--json", "task", "get", "t-seed"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert isinstance(obj, dict)
    assert obj["id"] == "t-seed"
    assert obj["title"] == "Seed"
    assert obj["type"] == "task"
    assert obj["status"] == "claimed"
    assert obj["tags"] == ["x"]


def test_cli_get_meta_only_omits_body(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-seed", body="UNIQUEBODYMARKER")
    result = _invoke(["task", "get", "t-seed", "--meta-only"])
    assert result.exit_code == 0, result.output
    assert "id: t-seed" in result.output
    assert "UNIQUEBODYMARKER" not in result.output


def test_cli_get_not_found_exits_3(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here")
    result = _invoke(["task", "get", "t-missing"])
    assert result.exit_code == 3, result.output


def test_cli_get_broken_task_file_exits_3(cfg: Config, vault: Path) -> None:
    """A t-id file with malformed frontmatter maps to exit 3, not a traceback."""
    # Valid t- stem (so it resolves) but frontmatter is missing required fields.
    path = task_folder("open", vault) / "t-broken.md"
    post = frontmatter.Post("body", id="t-broken", type="task")  # no title/created/updated
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    result = _invoke(["task", "get", "t-broken"])
    assert result.exit_code == 3, result.output
    assert result.exception is None or isinstance(result.exception, SystemExit), result.output


def test_cli_get_malformed_yaml_exits_3(cfg: Config, vault: Path) -> None:
    """A t-id file whose frontmatter is unparseable YAML maps to exit 3, not a crash."""
    _seed_malformed(vault, "open", "t-bad")
    result = _invoke(["task", "get", "t-bad"])
    assert result.exit_code == 3, result.output
    assert result.exception is None or isinstance(result.exception, SystemExit), result.output


def test_get_command_registered() -> None:
    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert "get" in names


# --------------------------------------------------------------------------- #
# CLI — shards task cancel                                                       #
# --------------------------------------------------------------------------- #


def test_cli_cancel_emits_cancelled(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="open")
    result = _invoke(["task", "cancel", "t-xxx"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "cancelled t-xxx"
    post = _reload(_done_path(vault, "t-xxx"))
    assert post.metadata["status"] == "cancelled"
    assert "## Cancelled" in post.content


def test_cli_cancel_with_reason_records_it(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="open")
    result = _invoke(["task", "cancel", "t-xxx", "--reason", "not needed"])
    assert result.exit_code == 0, result.output
    assert result.output.strip() == "cancelled t-xxx"
    content = _reload(_done_path(vault, "t-xxx")).content
    assert "not needed" in content
    assert content.index("## Cancelled") < content.index("not needed")


def test_cli_cancel_idempotent_exits_0(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="cancelled", body="b\n\n## Cancelled\n\nx")
    result = _invoke(["task", "cancel", "t-xxx", "--reason", "again"])
    assert result.exit_code == 0, result.output
    post = _reload(_done_path(vault, "t-xxx"))
    assert post.content.count("## Cancelled") == 1
    assert "again" not in post.content


def test_cli_cancel_not_found_exits_3(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-here", status="open")
    result = _invoke(["task", "cancel", "t-missing"])
    assert result.exit_code == 3, result.output


def test_cli_cancel_json_object(cfg: Config, vault: Path) -> None:
    _seed_task(vault, task_id="t-xxx", status="open")
    result = _invoke(["--json", "task", "cancel", "t-xxx"])
    assert result.exit_code == 0, result.output
    obj = json.loads(result.output)
    assert obj["id"] == "t-xxx"
    assert obj["status"] == "cancelled"


def test_cancel_command_registered() -> None:
    names = {cmd.name for cmd in task_cli.task_app.registered_commands}
    assert "cancel" in names
