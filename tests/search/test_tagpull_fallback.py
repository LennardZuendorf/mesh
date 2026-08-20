"""search/1 — tag-pull + substring fallback (R2, R3, R4).

Exercises the daemon-independent recall path: :func:`shards.index.tagpull.tagpull`
(frontmatter-only tag pull, AND semantics, zero body cost) and
:func:`shards.index.fallback.search_fallback` (substring scoring matrix + a single
stderr degradation notice), plus the ``shards search`` CLI surface that routes
between them (``--tags`` without a query → tag pull; a query → fallback because no
``indexed`` engine is available in this unit).

Corpus for both is **every** ``*.md`` under ``notes/`` and ``tasks/`` (full
recursive walk, broader than ``note list`` / ``task list``): typed note subfolders,
both task folders, and coexisting non-shards (foreign) files, which surface with
``id: None``.

Scoring matrix (highest matching tier wins): title exact ``1.0`` · title
substring ``0.8`` · tag contains ``0.6`` · body substring ``0.4``. Results below
the effective threshold are dropped; ties break on ``updated`` descending.
``threshold`` filters only when a caller sets it explicitly — with no explicit
value (``None``, the default) the fallback uses its own floor, the lowest tier
(``0.4``), so every tier is reachable at default configuration (core-hardening/4,
root tech.md § B5). The tests here pass an explicit low threshold (``0.1``) to
isolate the scoring matrix from that application rule; the rule itself is
covered by ``tests/index/test_fallback_threshold.py`` and the CLI cases below,
whose ``shards_config`` fixture sets an *explicit* ``[search].threshold = 0.65``.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta
from pathlib import Path

import frontmatter
import pytest
from typer.testing import CliRunner

from shards.cli.__main__ import app
from shards.index.fallback import search_fallback
from shards.index.tagpull import tagpull
from shards.schemas.config import Config, load_config
from shards.schemas.search import SearchResult

_NOW = datetime(2026, 6, 20, 12, 0, 0, tzinfo=UTC)


# --------------------------------------------------------------------------- #
# Fixtures & seeding helpers                                                   #
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
    created: datetime = _NOW,
) -> Path:
    """Write a shards file (valid ``n-``/``t-`` id) at ``notes|tasks/<rel>/<id>.md``."""
    meta: dict[str, object] = {
        "id": entry_id,
        "type": entry_type,
        "title": title,
        "tags": list(tags or []),
        "owner": owner,
        "created": created,
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


def _seed_foreign(
    vault: Path,
    rel: str,
    name: str,
    *,
    title: str | None = None,
    tags: list[str] | None = None,
    body: str = "Foreign body.",
    extra: dict[str, object] | None = None,
) -> Path:
    """Write a non-shards Markdown file (no ``n-``/``t-`` id)."""
    meta: dict[str, object] = {}
    if title is not None:
        meta["title"] = title
    if tags is not None:
        meta["tags"] = list(tags)
    if extra:
        meta.update(extra)
    folder = vault / rel
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{name}.md"
    post = frontmatter.Post(body)
    post.metadata = meta
    path.write_text(frontmatter.dumps(post), encoding="utf-8")
    return path


def _seed_malformed(vault: Path, rel: str, name: str) -> Path:
    """Write an ``.md`` whose frontmatter block is unparseable YAML."""
    folder = vault / rel
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{name}.md"
    path.write_text("---\ntitle: [unclosed\n---\nbody\n", encoding="utf-8")
    return path


# --------------------------------------------------------------------------- #
# SearchResult schema                                                          #
# --------------------------------------------------------------------------- #


def test_search_result_shape() -> None:
    r = SearchResult(
        id="n-abcd",
        type="note",
        title="Hello",
        score=0.8,
        tags=["a"],
        owner="me",
        updated=_NOW,
        snippet="body...",
        path="/vault/notes/n-abcd.md",
    )
    assert r.id == "n-abcd"
    assert r.score == 0.8
    assert r.path.endswith("n-abcd.md")


def test_search_result_optionals_default_none() -> None:
    r = SearchResult(id=None, type=None, title=None, score=1.0, path="/x.md")
    assert r.id is None
    assert r.tags is None
    assert r.owner is None
    assert r.updated is None
    assert r.snippet is None


# --------------------------------------------------------------------------- #
# tag-pull                                                                     #
# --------------------------------------------------------------------------- #


def test_tagpull_and_semantics(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-both", tags=["alpha", "beta"], title="Both")
    _seed(vault, "notes", entry_id="n-alpha", tags=["alpha"], title="AlphaOnly")
    results = tagpull(cfg, tags=["alpha", "beta"])
    ids = {r.id for r in results}
    assert ids == {"n-both"}
    assert all(r.score == 1.0 for r in results)


def test_tagpull_single_tag_all_matches(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-one", tags=["alpha", "beta"])
    _seed(vault, "notes/logs", entry_id="n-two", entry_type="log", tags=["alpha"])
    results = tagpull(cfg, tags=["alpha"])
    assert {r.id for r in results} == {"n-one", "n-two"}


def test_tagpull_recursive_corpus_notes_and_tasks(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes/decisions", entry_id="n-deep", entry_type="decision", tags=["x"])
    _seed(vault, "tasks/open", entry_id="t-open", entry_type="task", status="open", tags=["x"])
    _seed(vault, "tasks/done", entry_id="t-done", entry_type="task", status="done", tags=["x"])
    results = tagpull(cfg, tags=["x"])
    assert {r.id for r in results} == {"n-deep", "t-open", "t-done"}


def test_tagpull_foreign_file_id_none(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-real", tags=["shared"])
    _seed_foreign(vault, "notes", "othertool-note", title="Foreign", tags=["shared"])
    results = tagpull(cfg, tags=["shared"])
    by_id = {r.id for r in results}
    assert "n-real" in by_id
    assert None in by_id  # the foreign file appears with id=None


def test_tagpull_type_filter(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-note", entry_type="note", tags=["q"])
    _seed(vault, "tasks/open", entry_id="t-task", entry_type="task", status="open", tags=["q"])
    results = tagpull(cfg, tags=["q"], type_filter="task")
    assert {r.id for r in results} == {"t-task"}


def test_tagpull_owner_filter(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-mine", tags=["q"], owner="me")
    _seed(vault, "notes", entry_id="n-yours", tags=["q"], owner="you")
    results = tagpull(cfg, tags=["q"], owner="me")
    assert {r.id for r in results} == {"n-mine"}


def test_tagpull_status_filter(cfg: Config, vault: Path) -> None:
    _seed(vault, "tasks/open", entry_id="t-open", entry_type="task", status="open", tags=["q"])
    _seed(vault, "tasks/done", entry_id="t-done", entry_type="task", status="done", tags=["q"])
    results = tagpull(cfg, tags=["q"], status="open")
    assert {r.id for r in results} == {"t-open"}


def test_tagpull_meta_only_no_snippet(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-x", tags=["q"], body="lots of body text here")
    results = tagpull(cfg, tags=["q"])
    assert results and all(r.snippet is None for r in results)


def test_tagpull_limit(cfg: Config, vault: Path) -> None:
    for i in range(5):
        _seed(vault, "notes", entry_id=f"n-{i}", tags=["q"], updated=_NOW - timedelta(days=i))
    results = tagpull(cfg, tags=["q"], limit=2)
    assert len(results) == 2


def test_tagpull_malformed_skipped(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-ok", tags=["q"])
    _seed_malformed(vault, "notes", "broken")
    # Must not raise; the malformed file is silently skipped.
    results = tagpull(cfg, tags=["q"])
    assert {r.id for r in results} == {"n-ok"}


# --------------------------------------------------------------------------- #
# substring fallback — scoring matrix                                          #
# --------------------------------------------------------------------------- #


def test_fallback_title_exact_score(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-t", title="Exact Title")
    results = search_fallback(cfg, "exact title", threshold=0.1)
    assert results[0].id == "n-t"
    assert results[0].score == 1.0


def test_fallback_title_substring_score(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-t", title="Prefixed Title Suffix")
    results = search_fallback(cfg, "Title", threshold=0.1)
    assert results[0].id == "n-t"
    assert results[0].score == 0.8


def test_fallback_tag_contains_score(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-t", title="Nothing Here", tags=["keyword"], body="nope")
    results = search_fallback(cfg, "keyword", threshold=0.1)
    assert results[0].id == "n-t"
    assert results[0].score == 0.6


def test_fallback_body_substring_score(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-t", title="Zzz", tags=[], body="a needle in the body")
    results = search_fallback(cfg, "needle", threshold=0.1)
    assert results[0].id == "n-t"
    assert results[0].score == 0.4


def test_fallback_sort_score_then_updated(cfg: Config, vault: Path) -> None:
    # Two title-substring hits (0.8) — newer 'updated' wins the tie; one exact (1.0) leads.
    _seed(vault, "notes", entry_id="n-exact", title="Report", updated=_NOW - timedelta(days=9))
    _seed(
        vault, "notes", entry_id="n-old", title="Old Report Draft", updated=_NOW - timedelta(days=5)
    )
    _seed(
        vault, "notes", entry_id="n-new", title="New Report Draft", updated=_NOW - timedelta(days=1)
    )
    results = search_fallback(cfg, "Report", threshold=0.1)
    assert [r.id for r in results] == ["n-exact", "n-new", "n-old"]


def test_fallback_no_explicit_threshold_uses_lowest_tier_floor(cfg: Config, vault: Path) -> None:
    # No threshold passed (None) → no explicit value → the fallback's own floor
    # is the lowest matrix tier (0.4), so the body-only hit is now returned
    # (core-hardening/4, root tech.md § B5) rather than silently dropped.
    _seed(vault, "notes", entry_id="n-body", title="Unrelated", body="the needle hides here")
    results = search_fallback(cfg, "needle")
    assert {r.id for r in results} == {"n-body"}
    assert results[0].score == 0.4


def test_fallback_threshold_excludes_below(cfg: Config, vault: Path) -> None:
    # An explicit threshold above the body tier still filters it out.
    _seed(vault, "notes", entry_id="n-body", title="Unrelated", body="the needle hides here")
    assert search_fallback(cfg, "needle", threshold=0.65) == []
    # A lower explicit threshold lets it through.
    low = search_fallback(cfg, "needle", threshold=0.3)
    assert {r.id for r in low} == {"n-body"}


def test_fallback_foreign_file_id_none(cfg: Config, vault: Path) -> None:
    _seed_foreign(vault, "notes", "foreign", title="Foreign Report", body="x")
    results = search_fallback(cfg, "Foreign Report", threshold=0.1)
    assert results and results[0].id is None
    assert results[0].score == 1.0


def test_fallback_type_and_owner_and_status_filters(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-note", title="Shared Report", owner="me")
    _seed(
        vault,
        "tasks/open",
        entry_id="t-mine",
        entry_type="task",
        status="open",
        title="Shared Report",
        owner="me",
    )
    _seed(
        vault,
        "tasks/done",
        entry_id="t-done",
        entry_type="task",
        status="done",
        title="Shared Report",
        owner="me",
    )
    assert {
        r.id for r in search_fallback(cfg, "Shared Report", type_filter="task", threshold=0.1)
    } == {
        "t-mine",
        "t-done",
    }
    assert {r.id for r in search_fallback(cfg, "Shared Report", status="open", threshold=0.1)} == {
        "t-mine",
    }
    assert {
        r.id for r in search_fallback(cfg, "Shared Report", owner="nobody", threshold=0.1)
    } == set()


def test_fallback_tags_and_filter(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-both", title="Tagged Report", tags=["a", "b"])
    _seed(vault, "notes", entry_id="n-a", title="Tagged Report", tags=["a"])
    results = search_fallback(cfg, "Report", tags=["a", "b"], threshold=0.1)
    assert {r.id for r in results} == {"n-both"}


def test_fallback_limit(cfg: Config, vault: Path) -> None:
    for i in range(5):
        _seed(vault, "notes", entry_id=f"n-{i}", title="Report", updated=_NOW - timedelta(days=i))
    results = search_fallback(cfg, "Report", threshold=0.1, limit=2)
    assert len(results) == 2


def test_fallback_malformed_skipped(cfg: Config, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-ok", title="Report")
    _seed_malformed(vault, "notes", "broken")
    results = search_fallback(cfg, "Report", threshold=0.1)
    assert {r.id for r in results} == {"n-ok"}


# --------------------------------------------------------------------------- #
# substring fallback — stderr degradation notice                              #
# --------------------------------------------------------------------------- #

_NOTICE = "search: using substring fallback (indexed unavailable)"


def test_fallback_emits_single_stderr_notice(
    cfg: Config, vault: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    _seed(vault, "notes", entry_id="n-a", title="Report")
    _seed(vault, "notes", entry_id="n-b", title="Report Two")
    capsys.readouterr()  # clear
    search_fallback(cfg, "Report", threshold=0.1)
    err = capsys.readouterr().err
    assert err.count(_NOTICE) == 1  # exactly once, regardless of hit count


def test_fallback_quiet_suppresses_notice(
    cfg: Config, vault: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    _seed(vault, "notes", entry_id="n-a", title="Report")
    capsys.readouterr()
    search_fallback(cfg, "Report", threshold=0.1, quiet=True)
    assert _NOTICE not in capsys.readouterr().err


# --------------------------------------------------------------------------- #
# CLI — shards search                                                          #
# --------------------------------------------------------------------------- #


def test_cli_help_reachable(shards_config: Path) -> None:
    result = _invoke(["search", "--help"])
    assert result.exit_code == 0, result.output
    assert "search" in result.output.lower()


def test_cli_tags_routes_to_tagpull(shards_config: Path, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-hit", tags=["ndc"], title="NDC Note")
    _seed(vault, "notes", entry_id="n-miss", tags=["other"], title="Other")
    result = _invoke(["search", "--tags", "ndc"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.stdout)
    assert {h["id"] for h in arr} == {"n-hit"}
    assert all(h["score"] == 1.0 for h in arr)
    # tag pull is meta-only: no snippet / body in the payload
    assert all("snippet" not in h and "body" not in h for h in arr)


def test_cli_query_degraded_json_and_notice(shards_config: Path, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-hit", title="Alpha Decision Record", body="full detail")
    result = _invoke(["search", "Alpha Decision Record"])
    assert result.exit_code == 0, result.output
    arr = json.loads(result.stdout)
    assert arr and arr[0]["id"] == "n-hit"
    assert arr[0]["score"] == 1.0
    assert _NOTICE in result.stderr


def test_cli_quiet_suppresses_notice(shards_config: Path, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-hit", title="Alpha Decision Record")
    result = _invoke(["--quiet", "search", "Alpha Decision Record"])
    assert result.exit_code == 0, result.output
    assert _NOTICE not in result.stderr


def test_cli_json_hit_shape(shards_config: Path, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-hit", title="Shape Report", tags=["t"], owner="me")
    result = _invoke(["search", "Shape Report"])
    assert result.exit_code == 0, result.output
    hit = json.loads(result.stdout)[0]
    for key in ("id", "type", "title", "score", "path"):
        assert key in hit
    assert hit["path"].endswith("n-hit.md")


def test_cli_meta_only_omits_snippet(shards_config: Path, vault: Path) -> None:
    _seed(vault, "notes", entry_id="n-hit", title="Meta Report", body="BODYMARK")
    result = _invoke(["search", "Meta Report", "--meta-only"])
    assert result.exit_code == 0, result.output
    hit = json.loads(result.stdout)[0]
    assert "snippet" not in hit
    assert "body" not in hit


def test_cli_full_includes_body(shards_config: Path, vault: Path) -> None:
    # --full carries the complete Markdown body in the declared ``snippet`` field
    # (the hit shape has no separate ``body`` key).
    _seed(vault, "notes", entry_id="n-hit", title="Full Report", body="UNIQUE-BODY-MARK")
    result = _invoke(["search", "Full Report", "--full"])
    assert result.exit_code == 0, result.output
    hit = json.loads(result.stdout)[0]
    assert "UNIQUE-BODY-MARK" in hit["snippet"]
    assert "body" not in hit


def test_cli_threshold_excludes(shards_config: Path, vault: Path) -> None:
    # shards_config sets an explicit [search].threshold = 0.65 → the body-only
    # hit (0.4) is excluded, same as before this fix (explicit is honoured).
    _seed(vault, "notes", entry_id="n-body", title="Nothing", body="the needle here")
    excluded = _invoke(["search", "needle"])
    assert excluded.exit_code == 0, excluded.output
    assert json.loads(excluded.stdout) == []
    included = _invoke(["search", "needle", "--threshold", "0.3"])
    assert {h["id"] for h in json.loads(included.stdout)} == {"n-body"}


def test_cli_limit_caps_results(shards_config: Path, vault: Path) -> None:
    for i in range(4):
        _seed(vault, "notes", entry_id=f"n-{i}", title="Report", updated=_NOW - timedelta(days=i))
    result = _invoke(["search", "Report", "--limit", "2"])
    assert result.exit_code == 0, result.output
    assert len(json.loads(result.stdout)) == 2
