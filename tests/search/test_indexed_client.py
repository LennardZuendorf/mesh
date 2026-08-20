"""search/2 — the ``indexed`` hybrid wrapper (R1, R3, R4).

Exercises :mod:`shards.index.indexed_client`, a thin wrapper over the first-party
``indexed`` CLI, plus the ``shards search`` hybrid routing it enables. The real
``indexed`` binary is **never** shelled in these unit tests: every subprocess is
faked, either at the module-level seam (``_run_indexed_search`` /
``_run_indexed_update`` / ``_run_indexed_create``) or by monkeypatching
``subprocess.run`` directly.

Contract under test (search/tech.md):

* ``indexed index search "<q>" --collection <c> --json --limit N`` → NDJSON hits
  ``{"path","score","snippet"}``; each is mapped to a :class:`SearchResult` by
  reading frontmatter at ``path`` (a missing shards id → ``id=None``, a foreign
  file).
* Recency tiebreak: two hits whose scores are within ``0.02`` sort by ``updated``
  descending (more recent first).
* ``[search].collection`` absent/None disables hybrid → substring fallback.
* CLI: hybrid only when ``[search].hybrid`` **and** the daemon is up (degradation
  matrix); any ``CalledProcessError`` / ``FileNotFoundError`` degrades to the
  substring fallback with its stderr notice.
"""

from __future__ import annotations

import json
import subprocess
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

import frontmatter
import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app
from shards.index import indexed_client
from shards.schemas.config import Config, load_config
from shards.schemas.search import SearchResult

_NOW = datetime(2026, 6, 20, 12, 0, 0, tzinfo=UTC)
_FALLBACK_NOTICE = "search: using substring fallback (indexed unavailable)"


# --------------------------------------------------------------------------- #
# Fixtures & helpers                                                           #
# --------------------------------------------------------------------------- #


@pytest.fixture
def cfg(shards_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _seed(
    vault: Path,
    rel: str,
    *,
    entry_id: str,
    entry_type: str = "note",
    title: str = "A Note",
    tags: list[str] | None = None,
    owner: str | None = "seed-agent",
    status: str | None = None,
    body: str = "Body text.",
    updated: datetime = _NOW,
) -> Path:
    """Write a shards file (valid ``n-``/``t-`` id) under ``vault/<rel>/<id>.md``."""
    meta: dict[str, object] = {
        "id": entry_id,
        "type": entry_type,
        "title": title,
        "tags": list(tags or []),
        "owner": owner,
        "created": _NOW,
        "updated": updated,
        "related": [],
    }
    if status is not None:
        meta["status"] = status
    folder = vault / rel
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{entry_id}.md"
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def _seed_foreign(vault: Path, rel: str, name: str, *, title: str, body: str = "x") -> Path:
    """Write a non-shards Markdown file (no ``n-``/``t-`` id)."""
    folder = vault / rel
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{name}.md"
    path.write_text(frontmatter.dumps(frontmatter.Post(body, title=title)), encoding="utf-8")
    return path


def _ndjson(*hits: dict[str, Any]) -> str:
    """Render hit dicts as NDJSON (one JSON object per line), the ``indexed`` shape."""
    return "\n".join(json.dumps(h) for h in hits) + "\n"


def _patch_search(monkeypatch: pytest.MonkeyPatch, ndjson: str) -> None:
    """Replace the search subprocess seam so it returns fixture NDJSON."""

    def _run(collection: str, query: str, limit: int) -> str:
        return ndjson

    monkeypatch.setattr(indexed_client, "_run_indexed_search", _run)


# --------------------------------------------------------------------------- #
# search(): mapping indexed hits → SearchResult                                #
# --------------------------------------------------------------------------- #


def test_search_maps_hits_to_search_results(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    a = _seed(vault, "notes", entry_id="n-a", title="Alpha", tags=["x"], owner="me")
    b = _seed(vault, "tasks/open", entry_id="t-b", entry_type="task", status="open", title="Beta")
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(a), "score": 0.91, "snippet": "alpha snip"},
            {"path": str(b), "score": 0.72, "snippet": "beta snip"},
        ),
    )
    results = indexed_client.search(cfg, "alpha")
    assert [r.id for r in results] == ["n-a", "t-b"]
    top = results[0]
    assert isinstance(top, SearchResult)
    assert top.type == "note"
    assert top.title == "Alpha"
    assert top.score == 0.91
    assert top.tags == ["x"]
    assert top.owner == "me"
    assert top.snippet == "alpha snip"
    assert top.path == str(a)
    assert results[1].type == "task"


def test_search_foreign_file_id_none(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    foreign = _seed_foreign(vault, "notes", "othertool", title="Foreign Doc")
    _patch_search(monkeypatch, _ndjson({"path": str(foreign), "score": 0.8, "snippet": "s"}))
    results = indexed_client.search(cfg, "foreign")
    assert len(results) == 1
    assert results[0].id is None
    assert results[0].title == "Foreign Doc"


def test_search_missing_file_skipped(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    real = _seed(vault, "notes", entry_id="n-real", title="Real")
    gone = vault / "notes" / "n-gone.md"  # never created
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(gone), "score": 0.95, "snippet": "s"},
            {"path": str(real), "score": 0.90, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "real")
    assert [r.id for r in results] == ["n-real"]


def test_search_sandbox_skips_escaping_path(
    cfg: Config, vault: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A shards-shaped file that lives OUTSIDE the vault must never be read/returned.
    outside = tmp_path / "outside.md"
    outside.write_text(
        frontmatter.dumps(frontmatter.Post("x", id="n-out", type="note", title="Outside")),
        encoding="utf-8",
    )
    inside = _seed(vault, "notes", entry_id="n-in", title="Inside")
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(outside), "score": 0.99, "snippet": "s"},
            {"path": str(inside), "score": 0.80, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "q")
    assert [r.id for r in results] == ["n-in"]


# --------------------------------------------------------------------------- #
# search(): pinned NDJSON hit schema — drift is detected, not mis-parsed       #
# --------------------------------------------------------------------------- #


def test_search_skips_hit_missing_required_path(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A drifted `indexed` release that drops `path` from a hit must not crash
    # the query or silently coerce a bogus SearchResult — that one hit is
    # skipped and every well-shaped hit around it still comes through.
    keep = _seed(vault, "notes", entry_id="n-keep", title="Keep")
    _patch_search(
        monkeypatch,
        _ndjson(
            {"score": 0.95, "snippet": "no path here"},
            {"path": str(keep), "score": 0.90, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "q")
    assert [r.id for r in results] == ["n-keep"]


def test_search_skips_hit_missing_required_score(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # `bad` is a real, readable file — proving the hit is dropped by the schema
    # check itself, not merely because its path failed to resolve.
    keep = _seed(vault, "notes", entry_id="n-keep", title="Keep")
    bad = _seed(vault, "notes", entry_id="n-bad", title="Bad")
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(bad), "snippet": "no score here"},
            {"path": str(keep), "score": 0.90, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "q")
    assert [r.id for r in results] == ["n-keep"]


def test_search_skips_hit_with_wrong_typed_score(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A shape drift where `score` becomes a string (or any non-numeric JSON
    # value) is a validation failure against the pinned schema, not something
    # silently coerced into a comparable float. `bad` is a real file so a
    # regression that lets the hit through would surface as an extra id.
    keep = _seed(vault, "notes", entry_id="n-keep", title="Keep")
    bad = _seed(vault, "notes", entry_id="n-bad", title="Bad")
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(bad), "score": "not-a-number", "snippet": "s"},
            {"path": str(keep), "score": 0.90, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "q")
    assert [r.id for r in results] == ["n-keep"]


def test_search_rejects_boolean_score(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # JSON `true`/`false` decode to Python bool, a float subtype pitfall the
    # pinned schema must reject explicitly rather than silently treating as
    # 1.0/0.0. `bad` is a real file for the same reason as above.
    keep = _seed(vault, "notes", entry_id="n-keep", title="Keep")
    bad = _seed(vault, "notes", entry_id="n-bad", title="Bad")
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(bad), "score": True, "snippet": "s"},
            {"path": str(keep), "score": 0.90, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "q")
    assert [r.id for r in results] == ["n-keep"]


def test_search_tolerates_unknown_extra_fields_in_hit(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Forward-compatibility: indexed adding fields we don't model yet (an `id`,
    # nested `metadata`, ...) must not break the pinned decode.
    keep = _seed(vault, "notes", entry_id="n-keep", title="Keep")
    _patch_search(
        monkeypatch,
        _ndjson(
            {
                "path": str(keep),
                "score": 0.9,
                "snippet": "s",
                "id": "indexed-internal-id",
                "metadata": {"engine": "hybrid", "rank": 1},
            }
        ),
    )
    results = indexed_client.search(cfg, "q")
    assert [r.id for r in results] == ["n-keep"]


def test_search_coerces_integer_score_to_float(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    keep = _seed(vault, "notes", entry_id="n-keep", title="Keep")
    _patch_search(monkeypatch, _ndjson({"path": str(keep), "score": 1, "snippet": "s"}))
    results = indexed_client.search(cfg, "q", threshold=0.0)
    assert results[0].score == 1.0


def test_search_snippet_defaults_to_none_when_absent(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    keep = _seed(vault, "notes", entry_id="n-keep", title="Keep")
    _patch_search(monkeypatch, _ndjson({"path": str(keep), "score": 0.9}))
    results = indexed_client.search(cfg, "q")
    assert results[0].snippet is None


def test_parse_ndjson_skips_blank_and_garbled_lines(monkeypatch: pytest.MonkeyPatch) -> None:
    text = "\n".join(
        [
            "",
            "not json at all",
            '{"path": "/a", "score": 0.5}',
            "   ",
            '["not", "an", "object"]',
        ]
    )
    hits = indexed_client._parse_ndjson(text)
    assert len(hits) == 1
    assert hits[0].path == "/a"
    assert hits[0].score == 0.5


def test_parse_ndjson_decodes_through_the_pinned_schema() -> None:
    text = '{"path": "/a", "score": 1, "snippet": "s"}'
    hits = indexed_client._parse_ndjson(text)
    assert hits == [indexed_client._IndexedHit(path="/a", score=1.0, snippet="s")]


# --------------------------------------------------------------------------- #
# search(): ordering — score with a recency tiebreak within 0.02              #
# --------------------------------------------------------------------------- #


def test_search_recency_tiebreak_within_epsilon(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Scores 0.90 vs 0.91 are within 0.02 → the more recently updated file wins,
    # even though its raw score is lower.
    recent = _seed(vault, "notes", entry_id="n-recent", title="Recent", updated=_NOW)
    older = _seed(
        vault, "notes", entry_id="n-older", title="Older", updated=_NOW - timedelta(days=10)
    )
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(older), "score": 0.91, "snippet": "s"},
            {"path": str(recent), "score": 0.90, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "q")
    assert [r.id for r in results] == ["n-recent", "n-older"]


def test_search_no_tiebreak_when_scores_far_apart(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Scores 0.95 vs 0.70 differ by > 0.02 → higher score wins regardless of recency.
    high = _seed(vault, "notes", entry_id="n-high", title="High", updated=_NOW - timedelta(days=10))
    low = _seed(vault, "notes", entry_id="n-low", title="Low", updated=_NOW)
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(low), "score": 0.70, "snippet": "s"},
            {"path": str(high), "score": 0.95, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "q")
    assert [r.id for r in results] == ["n-high", "n-low"]


# --------------------------------------------------------------------------- #
# search(): threshold, limit, filters                                          #
# --------------------------------------------------------------------------- #


def test_search_threshold_drops_low_hits(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    keep = _seed(vault, "notes", entry_id="n-keep", title="Keep")
    drop = _seed(vault, "notes", entry_id="n-drop", title="Drop")
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(keep), "score": 0.90, "snippet": "s"},
            {"path": str(drop), "score": 0.50, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "q", threshold=0.65)
    assert [r.id for r in results] == ["n-keep"]


def test_search_limit_caps_results(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    hits = []
    for i in range(4):
        p = _seed(vault, "notes", entry_id=f"n-{i}", title=f"T{i}")
        hits.append({"path": str(p), "score": 0.9 - i * 0.001, "snippet": "s"})
    _patch_search(monkeypatch, _ndjson(*hits))
    results = indexed_client.search(cfg, "q", limit=2)
    assert len(results) == 2


def test_search_type_filter(cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    note = _seed(vault, "notes", entry_id="n-note", title="Shared")
    task = _seed(
        vault, "tasks/open", entry_id="t-task", entry_type="task", status="open", title="Shared"
    )
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(note), "score": 0.9, "snippet": "s"},
            {"path": str(task), "score": 0.8, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "shared", type_filter="task")
    assert [r.id for r in results] == ["t-task"]


def test_search_tags_filter_and_semantics(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    both = _seed(vault, "notes", entry_id="n-both", title="B", tags=["a", "b"])
    one = _seed(vault, "notes", entry_id="n-one", title="O", tags=["a"])
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(both), "score": 0.9, "snippet": "s"},
            {"path": str(one), "score": 0.85, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "q", tags=["a", "b"])
    assert [r.id for r in results] == ["n-both"]


def test_search_owner_filter(cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    mine = _seed(vault, "notes", entry_id="n-mine", title="M", owner="me")
    yours = _seed(vault, "notes", entry_id="n-yours", title="Y", owner="you")
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(mine), "score": 0.9, "snippet": "s"},
            {"path": str(yours), "score": 0.85, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "q", owner="me")
    assert [r.id for r in results] == ["n-mine"]


def test_search_status_filter(cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # --status must be applied post-retrieval on the hybrid path, exactly like
    # --type / --tags / --owner (search/tech.md lists it as a supported filter).
    open_task = _seed(
        vault, "tasks/open", entry_id="t-open", entry_type="task", status="open", title="Shared"
    )
    done_task = _seed(
        vault, "tasks/done", entry_id="t-done", entry_type="task", status="done", title="Shared"
    )
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(open_task), "score": 0.9, "snippet": "s"},
            {"path": str(done_task), "score": 0.85, "snippet": "s"},
        ),
    )
    results = indexed_client.search(cfg, "shared", status="open")
    assert [r.id for r in results] == ["t-open"]


# --------------------------------------------------------------------------- #
# search(): collection / subprocess seam                                       #
# --------------------------------------------------------------------------- #


def test_search_none_collection_falls_back_to_substring(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cfg.search.collection = None  # disables hybrid
    _seed(vault, "notes", entry_id="n-hit", title="Exact Title")

    def _boom(*_args: object, **_kwargs: object) -> str:
        raise AssertionError("indexed must not be shelled when collection is None")

    monkeypatch.setattr(indexed_client, "_run_indexed_search", _boom)
    results = indexed_client.search(cfg, "exact title", threshold=0.1)
    assert [r.id for r in results] == ["n-hit"]
    assert results[0].score == 1.0  # substring 'title exact' tier, not an indexed score


def test_search_disabled_collection_forwards_quiet(
    cfg: Config,
    vault: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    # With no collection, search() degrades to the inner substring fallback. Its
    # single stderr degradation notice must honour the caller's quiet flag
    # (design.md: "hidden with --quiet").
    cfg.search.collection = None
    _seed(vault, "notes", entry_id="n-hit", title="Exact Title")

    indexed_client.search(cfg, "exact title", threshold=0.1, quiet=True)
    assert _FALLBACK_NOTICE not in capsys.readouterr().err  # suppressed under quiet

    indexed_client.search(cfg, "exact title", threshold=0.1, quiet=False)
    assert _FALLBACK_NOTICE in capsys.readouterr().err  # emitted by default


def test_search_builds_expected_argv(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    recorded: dict[str, Any] = {}

    def fake_run(args: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
        recorded["args"] = args
        recorded["kwargs"] = kwargs
        return subprocess.CompletedProcess(args, 0, stdout="", stderr="")

    monkeypatch.setattr(indexed_client.subprocess, "run", fake_run)
    indexed_client.search(cfg, "hello world", limit=5)
    assert recorded["args"] == [
        "indexed",
        "index",
        "search",
        "hello world",
        "--collection",
        "test-vault",
        "--json",
        "--limit",
        "5",
    ]
    # Content is data: never through a shell.
    assert recorded["kwargs"].get("shell") in (None, False)


def test_search_missing_binary_propagates(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    def _raise(*_args: object, **_kwargs: object) -> str:
        raise FileNotFoundError("indexed")

    monkeypatch.setattr(indexed_client, "_run_indexed_search", _raise)
    with pytest.raises(FileNotFoundError):
        indexed_client.search(cfg, "q")


def test_search_nonzero_exit_propagates(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    def _raise(*_args: object, **_kwargs: object) -> str:
        raise subprocess.CalledProcessError(1, ["indexed"])

    monkeypatch.setattr(indexed_client, "_run_indexed_search", _raise)
    with pytest.raises(subprocess.CalledProcessError):
        indexed_client.search(cfg, "q")


# --------------------------------------------------------------------------- #
# freshness: incremental_update / full_rebuild / reindex / hook                #
# --------------------------------------------------------------------------- #


def test_incremental_update_shells_update(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    recorded: dict[str, Any] = {}

    def fake_run(args: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
        recorded["args"] = args
        return subprocess.CompletedProcess(args, 0, stdout="", stderr="")

    monkeypatch.setattr(indexed_client.subprocess, "run", fake_run)
    target = vault / "notes" / "n-x.md"
    indexed_client.incremental_update(cfg, target)
    assert recorded["args"] == [
        "indexed",
        "index",
        "update",
        str(target),
        "--collection",
        "test-vault",
    ]


def test_full_rebuild_and_reindex_shell_create(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    recorded: list[list[str]] = []

    def fake_run(args: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
        recorded.append(args)
        return subprocess.CompletedProcess(args, 0, stdout="", stderr="")

    monkeypatch.setattr(indexed_client.subprocess, "run", fake_run)
    indexed_client.reindex(cfg)  # reindex delegates to full_rebuild
    assert recorded == [["indexed", "index", "create", str(vault), "--collection", "test-vault"]]


def test_full_rebuild_noop_without_collection(cfg: Config, monkeypatch: pytest.MonkeyPatch) -> None:
    cfg.search.collection = None

    def _boom(*_args: object, **_kwargs: object) -> subprocess.CompletedProcess[str]:
        raise AssertionError("no subprocess when collection is None")

    monkeypatch.setattr(indexed_client.subprocess, "run", _boom)
    indexed_client.full_rebuild(cfg)  # must be a silent no-op


def test_incremental_update_noop_without_collection(
    cfg: Config, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cfg.search.collection = None

    def _boom(*_args: object, **_kwargs: object) -> subprocess.CompletedProcess[str]:
        raise AssertionError("no subprocess when collection is None")

    monkeypatch.setattr(indexed_client.subprocess, "run", _boom)
    indexed_client.incremental_update(cfg, vault / "notes" / "n-x.md")


# --------------------------------------------------------------------------- #
# CLI — hybrid routing (search/tech.md degradation matrix)                    #
# --------------------------------------------------------------------------- #


def test_cli_hybrid_uses_indexed_when_daemon_up(
    shards_config: Path, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    hit = _seed(vault, "notes", entry_id="n-hit", title="Hybrid Note")
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    _patch_search(
        monkeypatch, _ndjson({"path": str(hit), "score": 0.91, "snippet": "indexed snip"})
    )
    result = _invoke(["search", "Hybrid Note"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.stdout)
    assert [h["id"] for h in arr] == ["n-hit"]
    assert arr[0]["score"] == 0.91  # indexed's rank, not the substring 1.0
    assert arr[0]["snippet"] == "indexed snip"
    assert _FALLBACK_NOTICE not in result.stderr  # hybrid worked — no degradation


def test_cli_hybrid_falls_back_on_called_process_error(
    shards_config: Path, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _seed(vault, "notes", entry_id="n-hit", title="Alpha Decision Record")
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)

    def _raise(*_a: object, **_k: object) -> str:
        raise subprocess.CalledProcessError(1, ["indexed"])

    monkeypatch.setattr(indexed_client, "_run_indexed_search", _raise)
    result = _invoke(["search", "Alpha Decision Record"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.stdout)
    assert [h["id"] for h in arr] == ["n-hit"]
    assert arr[0]["score"] == 1.0  # substring fallback ran
    assert _FALLBACK_NOTICE in result.stderr


def test_cli_hybrid_falls_back_on_missing_binary(
    shards_config: Path, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _seed(vault, "notes", entry_id="n-hit", title="Alpha Decision Record")
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)

    def _raise(*_a: object, **_k: object) -> str:
        raise FileNotFoundError("indexed")

    monkeypatch.setattr(indexed_client, "_run_indexed_search", _raise)
    result = _invoke(["search", "Alpha Decision Record"])
    assert result.exit_code == 0, result.output
    assert _FALLBACK_NOTICE in result.stderr


def test_cli_substring_when_daemon_down(
    shards_config: Path, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _seed(vault, "notes", entry_id="n-hit", title="Alpha Decision Record")
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: False)

    def _boom(*_a: object, **_k: object) -> str:
        raise AssertionError("indexed must not be shelled when the daemon is down")

    monkeypatch.setattr(indexed_client, "_run_indexed_search", _boom)
    result = _invoke(["search", "Alpha Decision Record"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.stdout)
    assert [h["id"] for h in arr] == ["n-hit"]
    assert _FALLBACK_NOTICE in result.stderr


def test_cli_hybrid_honours_status_filter(
    shards_config: Path, vault: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # shards search "q" --status open with hybrid active + daemon up must filter by
    # status, not silently return tasks of all statuses (the reported regression).
    open_task = _seed(
        vault, "tasks/open", entry_id="t-open", entry_type="task", status="open", title="Shared"
    )
    done_task = _seed(
        vault, "tasks/done", entry_id="t-done", entry_type="task", status="done", title="Shared"
    )
    monkeypatch.setattr("shards.core.search._daemon_up", lambda: True)
    _patch_search(
        monkeypatch,
        _ndjson(
            {"path": str(open_task), "score": 0.91, "snippet": "s"},
            {"path": str(done_task), "score": 0.85, "snippet": "s"},
        ),
    )
    result = _invoke(["search", "Shared", "--status", "open"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.stdout)
    assert [h["id"] for h in arr] == ["t-open"]
