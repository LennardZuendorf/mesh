"""core-hardening/5 — the daemon's warm read handlers and their parity contract.

Before this unit the daemon accelerated almost nothing: eight of nine dispatch
slots were permanent ``503`` stubs, so every ``note list`` / ``task list`` /
``status`` / tag pull silently walked and YAML-parsed the whole vault while the
warm :class:`~shards.index.warm.VaultIndex` held that exact frontmatter in RAM.
Four list-shaped reads are now bound to that index — ``note.list``, ``task.list``,
``vault.status``, ``search.tag_pull`` — and the point reads and ``indexed``
passthroughs are gone.

Two properties have to hold together, and this module is where they are pinned:

* **The acceleration is real.** With a warm daemon the disk walk is never entered
  — proven by patching the walkers to raise and watching the reads still succeed
  (and watching the same patch *break* the daemon-down path, so the test cannot
  pass vacuously).
* **The acceleration is invisible.** Warm and cold answers are byte-identical.
  The whole read matrix is therefore run twice against one seeded fixture vault —
  daemon-up and daemon-down — and compared, rather than each read being asserted
  once against whichever transport the test happened to get.

The transport-failure surface (down / hung / mid-request kill / truncated reply)
gets the same treatment: every one of them must degrade to the identical answer,
never to an error. The daemon accelerates; it never gates.
"""

from __future__ import annotations

import contextlib
import json
import socket
import threading
from collections.abc import Iterator
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

import frontmatter
import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app
from shards.core.notes import _iter_note_files, in_note_scope
from shards.core.search import hit_dict
from shards.core.tasks import _iter_task_files, in_task_scope
from shards.daemon.client import DaemonClient
from shards.index.warm import VaultIndex, iter_vault_md
from shards.schemas.config import Config, load_config
from shards.schemas.note import Note
from tests.daemon.conftest import running_daemon

_AGENT = "test-agent"  # matches the ``shards_config`` fixture's [core].agent
_OTHER = "other-agent"


# --------------------------------------------------------------------------- #
# Fixtures — one seeded vault, two transports                                  #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _write(path: Path, meta: dict[str, Any], body: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(frontmatter.dumps(frontmatter.Post(body, **meta)), encoding="utf-8")
    return path


def _note_meta(
    note_id: str,
    title: str,
    *,
    note_type: str = "note",
    tags: list[str] | None = None,
    owner: str = _AGENT,
    age_days: int = 0,
) -> dict[str, Any]:
    when = datetime.now(UTC) - timedelta(days=age_days)
    return {
        "id": note_id,
        "type": note_type,
        "title": title,
        "tags": list(tags or []),
        "owner": owner,
        "created": when,
        "updated": when,
        "related": [],
    }


def _note(
    vault: Path,
    *,
    note_id: str,
    title: str,
    note_type: str = "note",
    sub: str = "notes",
    tags: list[str] | None = None,
    owner: str = _AGENT,
    age_days: int = 0,
    body: str = "Note body.",
) -> Path:
    meta = _note_meta(
        note_id, title, note_type=note_type, tags=tags, owner=owner, age_days=age_days
    )
    return _write(vault / sub / f"{note_id}.md", meta, body)


def _task_meta(
    task_id: str,
    title: str,
    *,
    status: str = "open",
    priority: str | None = None,
    tags: list[str] | None = None,
    owner: str = _AGENT,
    claimed_by: str | None = None,
    project: str | None = None,
    age_days: int = 0,
) -> dict[str, Any]:
    return {
        **_note_meta(task_id, title, note_type="task", tags=tags, owner=owner, age_days=age_days),
        "status": status,
        "priority": priority,
        "claimed_by": claimed_by,
        "project": project,
        "blocks": [],
        "blocked_by": [],
    }


def _task(
    vault: Path,
    *,
    task_id: str,
    title: str,
    status: str = "open",
    priority: str | None = None,
    tags: list[str] | None = None,
    owner: str = _AGENT,
    claimed_by: str | None = None,
    project: str | None = None,
    age_days: int = 0,
    body: str = "Task body.",
) -> Path:
    meta = _task_meta(
        task_id,
        title,
        status=status,
        priority=priority,
        tags=tags,
        owner=owner,
        claimed_by=claimed_by,
        project=project,
        age_days=age_days,
    )
    sub = "tasks/done" if status in {"done", "cancelled"} else "tasks/open"
    return _write(vault / sub / f"{task_id}.md", meta, body)


@pytest.fixture
def seeded_vault(vault: Path) -> Path:
    """One canonical corpus every read in this module is asserted against.

    Deliberately includes the awkward cases: a foreign (id-less) Tolaria file that
    the tag pull must still surface with ``id: None``, a file whose frontmatter is
    malformed YAML that every reader must skip, a note carrying a wikilink that
    dangles, both task folders, and — the case that matters most for parity —
    three **misfiled** entities that the warm index holds but the on-disk walks do
    not reach: a task nested under ``tasks/open/sub/``, a task in a folder outside
    the ``open|done`` lifecycle, and an ``n-`` note filed under ``tasks/``. None of
    the three is gettable, claimable or finishable on disk, so neither list may
    surface them; all three *are* part of the search corpus. Without them here the
    parity matrix would compare two identically-canonical vaults and prove nothing
    about scope.
    """
    _note(vault, note_id="n-alpha", title="Alpha", tags=["a", "shared"])
    _note(
        vault,
        note_id="n-beta",
        title="Beta",
        note_type="decision",
        sub="notes/decisions",
        tags=["shared"],
        owner=_OTHER,
        age_days=30,
        body="links to [[Nowhere At All]]",
    )
    _note(vault, note_id="n-proj", title="Proj", note_type="project", sub="notes/projects")

    _task(vault, task_id="t-open", title="Open One", priority="high", tags=["a"], project="n-proj")
    _task(
        vault,
        task_id="t-claim",
        title="Claimed",
        status="claimed",
        priority="low",
        owner=_OTHER,
        claimed_by=_AGENT,
    )
    _task(
        vault,
        task_id="t-done",
        title="Done One",
        status="done",
        owner=_OTHER,
        tags=["shared"],
        age_days=30,
    )

    # Misfiled entities — in the vault-wide index, outside every on-disk walk.
    for rel, task_id in (
        ("tasks/open/sub", "t-nested"),
        ("tasks/archive", "t-stray"),
    ):
        _write(vault / rel / f"{task_id}.md", _task_meta(task_id, f"Misfiled {task_id}"), "body")
    _write(
        vault / "tasks" / "n-misfiled.md",
        _note_meta("n-misfiled", "Misfiled Note"),
        "body",
    )

    # Coexisting Tolaria markdown: no shards id, so it is invisible to note/task
    # list but part of the search corpus.
    _write(vault / "notes" / "tolaria.md", {"title": "Foreign", "tags": ["shared"]}, "foreign body")
    # Malformed frontmatter: skipped silently by every reader, warm or cold.
    (vault / "notes" / "broken.md").write_text("---\nid: [n-x\n---\nbroken\n", encoding="utf-8")
    return vault


@pytest.fixture
def cold(missing_socket: Path) -> DaemonClient:
    """A client whose socket cannot exist — every read takes the file-op fallback."""
    return DaemonClient(socket_path=missing_socket)


@pytest.fixture(params=["warm", "cold"])
def reads(
    request: pytest.FixtureRequest,
    cfg: Config,
    seeded_vault: Path,
    socket_path: Path,
    missing_socket: Path,
) -> Iterator[DaemonClient]:
    """The same client API over a live warm daemon and over no daemon at all.

    Every behavioural assertion below runs twice, once per transport, so a read
    that only works warm (or only works cold) fails loudly and by name.
    """
    if request.param == "cold":
        yield DaemonClient(socket_path=missing_socket)
        return
    with running_daemon(socket_path, config=cfg):
        yield DaemonClient(socket_path=socket_path)


# --------------------------------------------------------------------------- #
# Snapshot helpers                                                             #
# --------------------------------------------------------------------------- #


def _rows(views: list[Any], field: str) -> list[dict[str, Any]]:
    """Render list views to a comparable JSON shape (frontmatter + path + body)."""
    out: list[dict[str, Any]] = []
    for view in views:
        model: Note = getattr(view, field)
        out.append(
            {"meta": model.model_dump(mode="json"), "path": str(view.path), "body": view.body}
        )
    return out


def _read_matrix(client: DaemonClient, cfg: Config) -> dict[str, Any]:
    """Every wired read, across the filter/sort/limit surface, as comparable JSON."""
    status = client.vault_status(cfg)
    # ``age_seconds`` is derived from wall-clock at assembly time, so it differs
    # between two runs by construction; ``mtime`` (its input) is compared instead.
    status = {**status, "freshness": {"mtime": status["freshness"]["mtime"]}}
    return {
        "note.list/all": _rows(client.note_list(cfg, limit=None), "note"),
        "note.list/tag": _rows(client.note_list(cfg, tags=["shared"], limit=None), "note"),
        "note.list/any-tag": _rows(
            client.note_list(cfg, tags=["a", "shared"], any_tag=True, limit=None), "note"
        ),
        "note.list/type": _rows(client.note_list(cfg, note_type="decision", limit=None), "note"),
        "note.list/owner": _rows(client.note_list(cfg, owner=_OTHER, limit=None), "note"),
        "note.list/since": _rows(client.note_list(cfg, since="7d", limit=None), "note"),
        "note.list/title-limit": _rows(client.note_list(cfg, sort="title", limit=2), "note"),
        "note.list/created": _rows(client.note_list(cfg, sort="created", limit=None), "note"),
        "task.list/all": _rows(client.task_list(cfg, limit=None), "task"),
        "task.list/status": _rows(client.task_list(cfg, status="open", limit=None), "task"),
        "task.list/status-csv": _rows(
            client.task_list(cfg, status="open,claimed", limit=None), "task"
        ),
        "task.list/mine": _rows(client.task_list(cfg, mine=True, limit=None), "task"),
        "task.list/owner": _rows(client.task_list(cfg, owner=_OTHER, limit=None), "task"),
        "task.list/project": _rows(client.task_list(cfg, project="n-proj", limit=None), "task"),
        "task.list/tags": _rows(client.task_list(cfg, tags=["shared"], limit=None), "task"),
        "task.list/since": _rows(client.task_list(cfg, since="7d", limit=None), "task"),
        "task.list/stale": _rows(client.task_list(cfg, stale="7d", limit=None), "task"),
        "task.list/since-stale-band": _rows(
            client.task_list(cfg, since="60d", stale="7d", limit=None), "task"
        ),
        "task.list/title-limit": _rows(client.task_list(cfg, sort="title", limit=2), "task"),
        "task.list/priority-sort": _rows(
            client.task_list(cfg, sort="priority", limit=None), "task"
        ),
        "task.list/available": _rows(client.task_list(cfg, available=True, limit=None), "task"),
        "tag_pull/all": [_hit(h) for h in client.tag_pull(cfg, limit=-1)],
        "tag_pull/tag": [_hit(h) for h in client.tag_pull(cfg, tags=["shared"], limit=-1)],
        "tag_pull/type": [_hit(h) for h in client.tag_pull(cfg, type_filter="task", limit=-1)],
        "tag_pull/owner": [_hit(h) for h in client.tag_pull(cfg, owner=_OTHER, limit=-1)],
        "tag_pull/status": [_hit(h) for h in client.tag_pull(cfg, status="done", limit=-1)],
        "tag_pull/limit": [_hit(h) for h in client.tag_pull(cfg, limit=2)],
        "vault.status": status,
        # team-awareness/6: the row-shape change (owner/claimed_by) belongs in
        # the parity matrix like every other wired read.
        "activity.recent": client.activity_recent(cfg, limit=-1)["entries"],
    }


def _hit(result: Any) -> dict[str, Any]:
    return hit_dict(result, meta_only=False, full=False)


def _ids(views: list[Any], field: str) -> list[str]:
    return [str(getattr(view, field).id) for view in views]


# --------------------------------------------------------------------------- #
# The parity contract — one vault, two transports, identical answers            #
# --------------------------------------------------------------------------- #


def test_warm_and_cold_read_matrices_are_identical(
    cfg: Config, seeded_vault: Path, socket_path: Path, missing_socket: Path
) -> None:
    """The whole read suite, run twice against one vault, agrees exactly.

    This is the unit's headline assertion: wiring the daemon must be invisible in
    the answers and visible only in the cost.
    """
    cold_matrix = _read_matrix(DaemonClient(socket_path=missing_socket), cfg)
    with running_daemon(socket_path, config=cfg):
        warm_matrix = _read_matrix(DaemonClient(socket_path=socket_path), cfg)

    assert set(warm_matrix) == set(cold_matrix)
    for key in sorted(cold_matrix):
        assert warm_matrix[key] == cold_matrix[key], key


def test_activity_recent_rows_carry_owner_and_claimed_by(reads: DaemonClient, cfg: Config) -> None:
    """team-awareness/6: identity travels on the row, warm and cold alike.

    ``n-beta`` is owned by a peer (``_OTHER``); ``t-claim`` is owned by ``_OTHER``
    and claimed by ``_AGENT`` (the caller) — the row must say so, not the caller's
    own identity, on *either* transport.
    """
    rows = {e["id"]: e for e in reads.activity_recent(cfg, limit=-1)["entries"]}
    assert rows["n-beta"]["owner"] == _OTHER
    assert rows["n-beta"]["claimed_by"] is None
    assert rows["t-claim"]["owner"] == _OTHER
    assert rows["t-claim"]["claimed_by"] == _AGENT
    for row in rows.values():
        assert set(row.keys()) == {"id", "type", "title", "path", "mtime", "owner", "claimed_by"}


def test_note_list_surfaces_only_shards_notes(reads: DaemonClient, cfg: Config) -> None:
    """Foreign and malformed files are skipped identically on both paths."""
    assert set(_ids(reads.note_list(cfg, limit=None), "note")) == {"n-alpha", "n-beta", "n-proj"}


def test_note_list_filters_and_sorts(reads: DaemonClient, cfg: Config) -> None:
    assert _ids(reads.note_list(cfg, note_type="decision", limit=None), "note") == ["n-beta"]
    assert _ids(reads.note_list(cfg, owner=_OTHER, limit=None), "note") == ["n-beta"]
    assert _ids(reads.note_list(cfg, tags=["a"], limit=None), "note") == ["n-alpha"]
    # ``n-beta`` is 30 days old, so a 7d window drops it.
    assert set(_ids(reads.note_list(cfg, since="7d", limit=None), "note")) == {"n-alpha", "n-proj"}
    assert _ids(reads.note_list(cfg, sort="title", limit=2), "note") == ["n-alpha", "n-beta"]


def test_task_list_filters(reads: DaemonClient, cfg: Config) -> None:
    assert set(_ids(reads.task_list(cfg, limit=None), "task")) == {"t-open", "t-claim", "t-done"}
    assert _ids(reads.task_list(cfg, status="open", limit=None), "task") == ["t-open"]
    assert _ids(reads.task_list(cfg, project="n-proj", limit=None), "task") == ["t-open"]
    # ``mine`` matches owner *or* claimed_by, resolved against the caller's agent.
    assert set(_ids(reads.task_list(cfg, mine=True, limit=None), "task")) == {"t-open", "t-claim"}
    assert _ids(reads.task_list(cfg, owner=_OTHER, status="done", limit=None), "task") == ["t-done"]


def test_task_list_status_csv_and_stale(reads: DaemonClient, cfg: Config) -> None:
    """team-awareness/4: comma-separated status is a union; --stale inverts --since."""
    assert set(_ids(reads.task_list(cfg, status="open,claimed", limit=None), "task")) == {
        "t-open",
        "t-claim",
    }
    # ``t-done`` is 30 days old — stale for a 7d window, and outside a 7d --since.
    assert _ids(reads.task_list(cfg, stale="7d", limit=None), "task") == ["t-done"]
    assert set(_ids(reads.task_list(cfg, since="7d", limit=None), "task")) == {
        "t-open",
        "t-claim",
    }
    # Combined: the band between 7 and 60 days ago also isolates ``t-done``.
    assert _ids(reads.task_list(cfg, since="60d", stale="7d", limit=None), "task") == ["t-done"]


def test_task_list_priority_sort_and_available(reads: DaemonClient, cfg: Config) -> None:
    """team-awareness/5: priority rank ascending; --available excludes claimed work.

    ``t-open`` (``high``) ranks first, ``t-claim`` (``low``, but ``claimed``) next,
    ``t-done`` (no priority) last — and only ``t-open`` is genuinely takeable.
    """
    assert _ids(reads.task_list(cfg, sort="priority", limit=None), "task") == [
        "t-open",
        "t-claim",
        "t-done",
    ]
    assert _ids(reads.task_list(cfg, available=True, limit=None), "task") == ["t-open"]


def test_task_list_default_limit_is_unbounded_only_when_asked(
    reads: DaemonClient, cfg: Config
) -> None:
    assert len(reads.task_list(cfg, limit=1)) == 1
    assert len(reads.task_list(cfg, limit=None)) == 3


def test_list_rows_carry_no_body_on_either_path(reads: DaemonClient, cfg: Config) -> None:
    """The index holds no bodies, so neither path may hand one out (parity)."""
    assert [v.body for v in reads.note_list(cfg, limit=None)] == ["", "", ""]
    assert [v.body for v in reads.task_list(cfg, limit=None)] == ["", "", ""]


def test_tag_pull_includes_foreign_files(reads: DaemonClient, cfg: Config) -> None:
    """A tag pull covers the *whole* corpus — coexisting Tolaria files included.

    This is the property that forced the warm index to hold id-less rows too: a
    warm tag pull that saw only shards entities would silently drop foreign hits
    the disk scan returns, which is a contract change rather than an acceleration.
    """
    hits = reads.tag_pull(cfg, tags=["shared"], limit=-1)
    assert None in {hit.id for hit in hits}
    assert {hit.title for hit in hits} == {"Alpha", "Beta", "Done One", "Foreign"}


def test_tag_pull_filters_and_scores(reads: DaemonClient, cfg: Config) -> None:
    # The *corpus* is wider than the entity scope: the misfiled tasks are search
    # hits even though no task verb can reach them. Warm and cold agree on that.
    assert {h.id for h in reads.tag_pull(cfg, type_filter="task", limit=-1)} == {
        "t-open",
        "t-claim",
        "t-done",
        "t-nested",
        "t-stray",
    }
    assert {h.id for h in reads.tag_pull(cfg, status="done", limit=-1)} == {"t-done"}
    assert all(h.score == 1.0 for h in reads.tag_pull(cfg, limit=-1))
    assert all(h.snippet is None for h in reads.tag_pull(cfg, limit=-1))
    assert len(reads.tag_pull(cfg, limit=2)) == 2


def test_vault_status_shape(reads: DaemonClient, cfg: Config) -> None:
    report = reads.vault_status(cfg)
    assert report["notes"] == 3
    assert report["tasks"] == {"open": 1, "claimed": 1, "done": 1, "cancelled": 0}
    assert report["tasks_total"] == 3
    assert report["freshness"]["mtime"] is not None
    assert report["freshness"]["age_seconds"] >= 0.0
    # Dangling links need note/task *bodies*, which the index does not hold — the
    # handler computes them on disk on both paths, so the field is present either way.
    assert report["dangling_links"] == ["Nowhere At All"]
    assert report["stale_locks"] == []


# --------------------------------------------------------------------------- #
# The acceleration is real — a warm read never enters the disk walk             #
# --------------------------------------------------------------------------- #


def _explode(*_args: object, **_kwargs: object) -> Any:
    raise AssertionError("the vault walker must not run while the daemon is warm")


_WALKERS: dict[str, str] = {
    "task.list": "shards.core.tasks._iter_task_files",
    "note.list": "shards.core.notes._iter_note_files",
    "search.tag_pull": "shards.index.tagpull.iter_corpus",
}


@pytest.mark.parametrize("method", sorted(_WALKERS))
def test_warm_reads_never_walk_the_vault(
    cfg: Config,
    seeded_vault: Path,
    socket_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    method: str,
) -> None:
    """With the index warm, the on-disk walker is not called at all.

    The walker is patched to raise *after* the daemon has warmed, so the daemon's
    own answer comes purely from RAM. Without this the wiring could be nominal —
    a socket hop in front of the same full-vault parse.
    """
    with running_daemon(socket_path, config=cfg):
        client = DaemonClient(socket_path=socket_path)
        monkeypatch.setattr(_WALKERS[method], _explode)
        if method == "task.list":
            assert _ids(client.task_list(cfg, limit=None), "task")
        elif method == "note.list":
            assert _ids(client.note_list(cfg, limit=None), "note")
        else:
            assert client.tag_pull(cfg, limit=-1)


@pytest.mark.parametrize("method", sorted(_WALKERS))
def test_the_walker_patch_would_catch_a_disk_read(
    cfg: Config,
    seeded_vault: Path,
    cold: DaemonClient,
    monkeypatch: pytest.MonkeyPatch,
    method: str,
) -> None:
    """Control for the test above: with no daemon, the same patch *does* fire.

    Without this the warm assertion could pass vacuously — e.g. if the patched
    name were never the walker the fallback uses.
    """
    monkeypatch.setattr(_WALKERS[method], _explode)
    with pytest.raises(AssertionError, match="must not run"):
        if method == "task.list":
            cold.task_list(cfg, limit=None)
        elif method == "note.list":
            cold.note_list(cfg, limit=None)
        else:
            cold.tag_pull(cfg, limit=-1)


# --------------------------------------------------------------------------- #
# Scope — the warm projection selects exactly what the on-disk walk would        #
# --------------------------------------------------------------------------- #


def test_scope_predicates_match_the_on_disk_walks(cfg: Config, seeded_vault: Path) -> None:
    """The membership predicates and the walkers state one scope, not two.

    The index is vault-wide (the tag pull needs the whole corpus), so the warm
    ``note.list`` / ``task.list`` handlers project it through these predicates. If
    the predicate and the walker ever drift, the warm answer becomes a *superset*
    of the cold one and the two paths silently stop agreeing — which is precisely
    what the misfiled entities in ``seeded_vault`` are here to catch.
    """
    vault = cfg.core.tolaria_path
    corpus = set(iter_vault_md(vault))

    assert {p for p in corpus if in_note_scope(vault, p)} == set(_iter_note_files(cfg))
    assert {p for p in corpus if in_task_scope(vault, p)} == set(_iter_task_files(cfg))

    # …and the misfiled files really are in the corpus, so the assertions above
    # are not comparing two empty differences.
    assert vault / "tasks" / "open" / "sub" / "t-nested.md" in corpus
    assert vault / "tasks" / "archive" / "t-stray.md" in corpus
    assert vault / "tasks" / "n-misfiled.md" in corpus


def test_misfiled_entities_are_in_no_list_on_either_path(reads: DaemonClient, cfg: Config) -> None:
    """A task outside ``tasks/{open,done}`` — or a note under ``tasks/`` — is not listed.

    They are not resolvable by ``task get`` / ``note get`` either (the same walk
    backs ``_resolve_task_path``), so listing them would advertise entities no
    other verb can act on.
    """
    task_ids = set(_ids(reads.task_list(cfg, limit=None), "task"))
    note_ids = set(_ids(reads.note_list(cfg, limit=None), "note"))
    assert not task_ids & {"t-nested", "t-stray"}
    assert "n-misfiled" not in note_ids
    assert reads.vault_status(cfg)["tasks_total"] == 3


# --------------------------------------------------------------------------- #
# An undecodable reply falls back — it never becomes an empty answer            #
# --------------------------------------------------------------------------- #

_UNDECODABLE_ENTRIES: tuple[Any, ...] = (
    None,
    [],
    {},
    {"entries": "not-a-list"},
    {"entries": [7]},
    {"entries": [{"path": "/x"}]},  # no meta
    {"entries": [{"meta": {"id": "n-x"}, "path": "/x"}]},  # meta fails validation
)


def _replying(client: DaemonClient, monkeypatch: pytest.MonkeyPatch, payload: Any) -> None:
    """Make every RPC on ``client`` succeed with ``payload`` as its result."""
    monkeypatch.setattr(client, "_request", lambda _method, _params: payload)


@pytest.mark.parametrize("payload", _UNDECODABLE_ENTRIES)
def test_note_list_falls_back_when_the_reply_cannot_be_decoded(
    cfg: Config,
    seeded_vault: Path,
    cold: DaemonClient,
    monkeypatch: pytest.MonkeyPatch,
    payload: Any,
) -> None:
    """A live daemon answering ``ok: true`` with rows we cannot read must not gate.

    This is the version-skew case the widened fallback-code set names: the daemon
    knows the method and answers successfully, but its rows no longer validate
    after a schema change. Returning what parsed — usually nothing — would show
    "no notes" over a full vault, which is the accelerator gating the read.
    """
    _replying(cold, monkeypatch, payload)
    assert set(_ids(cold.note_list(cfg, limit=None), "note")) == {"n-alpha", "n-beta", "n-proj"}


@pytest.mark.parametrize("payload", _UNDECODABLE_ENTRIES)
def test_task_list_falls_back_when_the_reply_cannot_be_decoded(
    cfg: Config,
    seeded_vault: Path,
    cold: DaemonClient,
    monkeypatch: pytest.MonkeyPatch,
    payload: Any,
) -> None:
    _replying(cold, monkeypatch, payload)
    assert set(_ids(cold.task_list(cfg, limit=None), "task")) == {"t-open", "t-claim", "t-done"}


@pytest.mark.parametrize(
    "payload",
    [None, [], {}, {"results": "not-a-list"}, {"results": [7]}, {"results": [{"nope": 1}]}],
)
def test_tag_pull_falls_back_when_the_reply_cannot_be_decoded(
    cfg: Config,
    seeded_vault: Path,
    cold: DaemonClient,
    monkeypatch: pytest.MonkeyPatch,
    payload: Any,
) -> None:
    _replying(cold, monkeypatch, payload)
    assert len(cold.tag_pull(cfg, tags=["shared"], limit=-1)) == 4


@pytest.mark.parametrize(
    "payload",
    [
        None,
        [],
        {},
        {"notes": 3, "tasks": {}},  # the pre-split payload an older daemon would send
        {"note_count": "three", "task_statuses": [], "newest": []},
        {"note_count": 3, "task_statuses": "open", "newest": []},
        {"note_count": 3, "task_statuses": [1], "newest": []},
        {"note_count": 3, "task_statuses": [], "newest": "no"},
        {"note_count": True, "task_statuses": [], "newest": []},  # bool is not a count
    ],
)
def test_vault_status_falls_back_when_the_reply_cannot_be_decoded(
    cfg: Config,
    seeded_vault: Path,
    cold: DaemonClient,
    monkeypatch: pytest.MonkeyPatch,
    payload: Any,
) -> None:
    """A half-built report is never returned: the client recomputes the whole thing."""
    _replying(cold, monkeypatch, payload)
    report = cold.vault_status(cfg)
    assert report["notes"] == 3
    assert report["tasks_total"] == 3
    assert report["dangling_links"] == ["Nowhere At All"]


# --------------------------------------------------------------------------- #
# vault.status keeps the disk half off the daemon's event loop                  #
# --------------------------------------------------------------------------- #


def test_vault_status_handler_does_no_disk_scan(
    cfg: Config, seeded_vault: Path, socket_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The handler ships only the index-derivable half — no body scan on the loop.

    ``DaemonServer._dispatch`` runs handlers synchronously on the event loop, so a
    whole-vault ``find_dangling`` there would block every other agent's warm read
    behind one ``shards status``. The handler is dispatched directly here with the
    disk halves patched to raise: it must still answer.
    """
    with running_daemon(socket_path, config=cfg) as server:
        monkeypatch.setattr("shards.core.lenses.find_dangling", _explode)
        monkeypatch.setattr("shards.core.lenses.scan_stale_locks", _explode)
        monkeypatch.setattr("shards.core.notes._iter_note_files", _explode)
        monkeypatch.setattr("shards.core.tasks._iter_task_files", _explode)
        line = (json.dumps({"id": "s", "method": "vault.status", "params": {}}) + "\n").encode()
        reply = server._dispatch(line)

    assert reply["ok"] is True, reply
    assert set(reply["result"]) == {"note_count", "task_statuses", "newest"}
    assert reply["result"]["note_count"] == 3
    assert sorted(reply["result"]["task_statuses"]) == ["claimed", "done", "open"]


def test_warm_vault_status_never_walks_the_entity_folders(
    cfg: Config, seeded_vault: Path, socket_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """End to end: warm ``vault.status`` costs no note/task frontmatter walk.

    The link scan and the lock listing still run — in *this* process, where they
    block nobody else — so only the two entity walkers are patched.
    """
    with running_daemon(socket_path, config=cfg):
        client = DaemonClient(socket_path=socket_path)
        monkeypatch.setattr("shards.core.notes._iter_note_files", _explode)
        monkeypatch.setattr("shards.core.tasks._iter_task_files", _explode)
        report = client.vault_status(cfg)

    assert report["notes"] == 3
    assert report["tasks"] == {"open": 1, "claimed": 1, "done": 1, "cancelled": 0}
    assert report["dangling_links"] == ["Nowhere At All"]  # the disk half still ran


# --------------------------------------------------------------------------- #
# The whole transport-failure surface degrades, never fails                     #
# --------------------------------------------------------------------------- #


@contextlib.contextmanager
def _rogue_daemon(path: Path, behaviour: str) -> Iterator[None]:
    """A socket that accepts but misbehaves: hangs, dies mid-request, or truncates.

    Covers the failure modes ``.spec/lessons.md`` records under *"Daemon-down
    fallback must catch the whole transport-failure surface"* — a missing socket
    is only the easiest one.
    """
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(str(path))
    listener.listen(4)
    listener.settimeout(0.2)
    stop = threading.Event()

    def serve() -> None:
        while not stop.is_set():
            try:
                conn, _ = listener.accept()
            except (TimeoutError, OSError):
                continue
            with conn, contextlib.suppress(OSError):
                conn.recv(4096)
                if behaviour == "hang":
                    stop.wait(3.0)  # never answers — the client must time out
                elif behaviour == "truncated":
                    conn.sendall(b'{"id": "x", "ok": tr')  # a reply cut mid-line
                # "killed": drop the connection with no bytes at all

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    try:
        yield
    finally:
        stop.set()
        listener.close()
        thread.join(timeout=3)
        path.unlink(missing_ok=True)


@pytest.mark.parametrize("behaviour", ["hang", "killed", "truncated"])
def test_every_wired_read_degrades_on_a_broken_daemon(
    cfg: Config, seeded_vault: Path, sock_dir: Path, missing_socket: Path, behaviour: str
) -> None:
    """Hung, killed mid-request, or truncated: identical answers, no exception."""
    expected = _read_matrix(DaemonClient(socket_path=missing_socket), cfg)
    path = sock_dir / f"rogue-{behaviour}.sock"
    with _rogue_daemon(path, behaviour):
        client = DaemonClient(socket_path=path, timeout=0.3)
        actual = _read_matrix(client, cfg)
    for key in sorted(expected):
        assert actual[key] == expected[key], key


# --------------------------------------------------------------------------- #
# End to end — the CLI surfaces agree warm and cold                             #
# --------------------------------------------------------------------------- #


_CLI_READS: tuple[tuple[str, ...], ...] = (
    ("--json", "note", "list"),
    ("--json", "task", "list"),
    ("--json", "task", "list", "--mine", "--sort", "title"),
    ("--json", "task", "list", "--status", "open,claimed"),
    ("--json", "task", "list", "--stale", "7d"),
    ("--json", "task", "list", "--sort", "priority"),
    ("--json", "task", "list", "--available"),
    ("task", "list"),  # human text rows (id/status/claimed_by/title)
    ("--json", "status"),
    ("search", "--tags", "shared"),
)


def _cli_outputs(runtime_dir: Path, monkeypatch: pytest.MonkeyPatch) -> list[str]:
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(runtime_dir))
    runner = CliRunner()
    outputs: list[str] = []
    for args in _CLI_READS:
        result = runner.invoke(app, list(args))
        assert result.exit_code == 0, (args, result.output)
        outputs.append(result.stdout)
    return outputs


def test_cli_read_output_is_identical_warm_and_cold(
    cfg: Config, seeded_vault: Path, sock_dir: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """End-to-end through the real commands, resolving the real default socket path."""
    empty = sock_dir / "empty"
    empty.mkdir()
    cold_out = _cli_outputs(empty, monkeypatch)

    with running_daemon(sock_dir / "shards.sock", config=cfg):
        warm_out = _cli_outputs(sock_dir, monkeypatch)

    for args, cold_text, warm_text in zip(_CLI_READS, cold_out, warm_out, strict=True):
        if args == ("--json", "status"):
            # ``age_seconds`` moves with the wall clock; compare its input instead.
            cold_obj, warm_obj = json.loads(cold_text), json.loads(warm_text)
            for obj in (cold_obj, warm_obj):
                obj["freshness"].pop("age_seconds")
            assert warm_obj == cold_obj, args
        else:
            assert warm_text == cold_text, args


def test_writes_still_work_with_the_daemon_up(
    cfg: Config, seeded_vault: Path, sock_dir: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Writes bypass the socket entirely — the daemon is never in a write path."""
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(sock_dir))
    runner = CliRunner()
    with running_daemon(sock_dir / "shards.sock", config=cfg):
        created = runner.invoke(app, ["--quiet", "task", "new", "Fresh Work"])
        assert created.exit_code == 0, created.output
        claimed = runner.invoke(app, ["--quiet", "task", "claim", created.stdout.strip()])
        assert claimed.exit_code == 0, claimed.output


# --------------------------------------------------------------------------- #
# VaultIndex — the corpus split and the safe-reader ruling                      #
# --------------------------------------------------------------------------- #


def test_index_holds_foreign_rows_in_the_corpus_only(seeded_vault: Path) -> None:
    """Foreign files join ``corpus()`` but never ``entries()`` / ``recent()`` / ``len()``."""
    index = VaultIndex()
    foreign = seeded_vault / "notes" / "tolaria.md"
    index.reparse(foreign)
    index.reparse(seeded_vault / "notes" / "n-alpha.md")

    assert len(index) == 1  # shards entities only
    assert [e.id for e in index.entries()] == ["n-alpha"]
    assert {str(e.path) for e in index.corpus()} == {
        str(foreign),
        str(seeded_vault / "notes" / "n-alpha.md"),
    }
    assert [row["id"] for row in index.recent()] == ["n-alpha"]


def test_reparse_evicts_a_vanished_file_but_keeps_a_corrupt_one(vault: Path) -> None:
    """The safe reader collapses "gone" and "corrupt"; ``reparse`` must not.

    ``storage.files.read_post`` returns ``None`` for both, so ``reparse`` re-checks
    the path: a file that is no longer there is evicted, one that is still there
    but unreadable keeps its last good row rather than vanishing from
    ``activity.recent`` on a transient parse failure.
    """
    path = _note(vault, note_id="n-live", title="Live")
    index = VaultIndex()
    index.reparse(path)
    assert index.get("n-live") is not None

    path.write_text("---\nid: [n-live\n---\ncorrupt\n", encoding="utf-8")
    index.reparse(path)
    assert index.get("n-live") is not None  # corrupt -> skip, last good row kept

    path.unlink()
    index.reparse(path)
    assert index.get("n-live") is None  # vanished -> evict


def test_reparse_evicts_when_a_directory_replaces_the_file(vault: Path) -> None:
    """A directory where a ``.md`` was is an ``IsADirectoryError`` — still an eviction."""
    path = _note(vault, note_id="n-dir", title="Dir")
    index = VaultIndex()
    index.reparse(path)
    path.unlink()
    path.mkdir()
    index.reparse(path)
    assert index.get("n-dir") is None


def test_index_moves_an_id_between_paths_without_leaking(vault: Path) -> None:
    """Re-indexing an id at a new path drops the old path's row (no duplicates)."""
    first = _note(vault, note_id="n-move", title="Move")
    index = VaultIndex()
    index.reparse(first)
    moved = vault / "notes" / "decisions" / "n-move.md"
    moved.parent.mkdir(parents=True, exist_ok=True)
    first.replace(moved)
    index.reparse(moved)

    assert len(index) == 1
    assert [str(e.path) for e in index.corpus()] == [str(moved)]


def test_a_file_that_loses_its_shards_id_moves_to_the_corpus_bucket(vault: Path) -> None:
    path = _note(vault, note_id="n-drop", title="Drop")
    index = VaultIndex()
    index.reparse(path)
    _write(path, {"title": "Now Foreign"}, "body")
    index.reparse(path)

    assert index.get("n-drop") is None
    assert len(index) == 0
    assert [e.id for e in index.corpus()] == [None]


# --------------------------------------------------------------------------- #
# The culled surface                                                           #
# --------------------------------------------------------------------------- #


def test_deleted_methods_have_no_client_verb_and_no_handler(cfg: Config) -> None:
    """``note.get`` / ``task.get`` / ``search.query`` / ``index.reindex`` are gone.

    A point read is one ``open()`` on a path the id already determines, ranking
    and rebuilding live in the ``indexed`` subprocess — none of them gets faster
    for crossing a socket, and all four client methods had zero callers.
    """
    from shards.daemon.server import DaemonServer, default_dispatch

    server = DaemonServer(Path("/tmp/unused.sock"), config=None)
    assert set(default_dispatch()) == {"ping"}
    assert not {"note_get", "task_get"} & set(dir(DaemonClient))
    assert not {"note.get", "task.get", "search.query", "index.reindex"} & set(server._handlers)


def test_warm_startup_registers_exactly_the_wired_reads(cfg: Config, socket_path: Path) -> None:
    with running_daemon(socket_path, config=cfg) as server:
        assert set(server._handlers) == {
            "ping",
            "activity.recent",
            "note.list",
            "task.list",
            "vault.status",
            "search.tag_pull",
        }
