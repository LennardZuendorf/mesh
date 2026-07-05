"""``brain search`` command surface — the recall verb.

``search`` is one leaf command (not a sub-verb group), so it is a Typer whose
callback *is* the command: ``brain search "<q>"`` and ``brain search --tags x``
both land here. Routing (product R2/R4):

* no query, ``--tags`` (or nothing) → :func:`~brain.index.tagpull.tagpull`
  (frontmatter-only, ``score=1.0``, meta-only);
* a query → hybrid recall via :func:`~brain.index.indexed_client.search` **when**
  ``[search].hybrid`` is on *and* the warm daemon is up (the search/tech.md
  degradation matrix); otherwise, or if ``indexed`` is unavailable
  (``FileNotFoundError`` / non-zero exit), the substring
  :func:`~brain.index.fallback.search_fallback`, which emits its own single stderr
  degradation notice.

Output is JSON by design (search is a machine-first path): a list of hits shaped
``{id, type, title, score, tags?, owner?, updated?, snippet?, path}``.
``--meta-only`` drops the body/snippet; ``--full`` swaps the snippet for the
complete Markdown body. Infrastructure notices go to stderr only, never into the
JSON payload.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any

import frontmatter
import typer
import yaml

from brain.daemon.client import DaemonClient
from brain.index import indexed_client
from brain.index.fallback import search_fallback
from brain.index.tagpull import tagpull
from brain.schemas.config import Config, load_config
from brain.schemas.search import SearchResult

search_app = typer.Typer(
    name="search",
    help="Recall across notes + tasks: tag pull (--tags) or substring fallback (query).",
    # ``search`` is a single leaf command whose args are its own — allow options to
    # follow the positional QUERY (a Typer callback group defaults to stopping option
    # parsing at the first non-option, which would read ``--limit`` as a subcommand).
    context_settings={"allow_interspersed_args": True},
)

# ``--tags`` is repeatable (``--tags a --tags b``, AND semantics). A ``list[str]``
# default calling ``typer.Option`` inline trips ruff B008 (mutable-annotated call in
# a default); the sanctioned fix is a module-level singleton referenced below.
_TAGS_OPTION = typer.Option(None, "--tags", help="Require all these tags (AND); repeatable.")


def _daemon_up() -> bool:
    """Whether the warm daemon answers a ping (gates hybrid per the degradation matrix).

    A down/absent daemon yields ``False`` so the query takes the substring fallback —
    the same behaviour a daemon-less run has always had, and what keeps recall correct
    when ``indexed``'s index may be stale. Kept as a module-level seam so tests can
    fake daemon liveness without a live socket.
    """
    return DaemonClient().is_up()


def _query_search(
    config: Config,
    query: str,
    *,
    type_filter: str | None,
    tags: list[str] | None,
    owner: str | None,
    status: str | None,
    limit: int,
    threshold: float,
    quiet: bool,
) -> list[SearchResult]:
    """Route a query to hybrid ``indexed`` recall or the substring fallback.

    Hybrid runs only when ``[search].hybrid`` is on *and* the daemon is up; any
    ``indexed`` failure (missing binary or non-zero exit) degrades to the substring
    fallback, which prints its own stderr notice. Every filter — ``type`` / ``tags``
    / ``owner`` / ``status`` — is applied post-retrieval by both paths; ``quiet`` is
    forwarded so a degradation notice honours ``--quiet`` whichever path emits it.
    """
    if config.search.hybrid and _daemon_up():
        try:
            return indexed_client.search(
                config,
                query,
                limit=limit,
                threshold=threshold,
                type_filter=type_filter,
                tags=tags,
                owner=owner,
                status=status,
                quiet=quiet,
            )
        except (subprocess.CalledProcessError, FileNotFoundError):
            pass  # indexed unavailable → fall through to the substring scan below
    return search_fallback(
        config,
        query,
        type_filter=type_filter,
        tags=tags,
        owner=owner,
        status=status,
        limit=limit,
        threshold=threshold,
        quiet=quiet,
    )


def _read_body(path: str) -> str:
    """Return the Markdown body at ``path`` (empty on a read/parse failure)."""
    try:
        return frontmatter.loads(Path(path).read_text(encoding="utf-8")).content
    except (OSError, yaml.YAMLError):
        return ""


def _hit_dict(result: SearchResult, *, meta_only: bool, full: bool) -> dict[str, Any]:
    """Render one hit to the JSON shape, honouring ``--meta-only`` / ``--full``.

    Optional keys appear only when populated: ``tags`` (non-empty), ``owner``,
    ``updated``. The ``snippet`` field carries an excerpt by default and the full
    Markdown body under ``--full``; ``--meta-only`` drops it entirely (keeping the
    single declared hit shape — there is no separate ``body`` key).
    """
    hit: dict[str, Any] = {
        "id": result.id,
        "type": result.type,
        "title": result.title,
        "score": result.score,
        "path": result.path,
    }
    if result.tags:
        hit["tags"] = result.tags
    if result.owner is not None:
        hit["owner"] = result.owner
    if result.updated is not None:
        hit["updated"] = result.updated.isoformat()
    if meta_only:
        return hit
    if full:
        hit["snippet"] = _read_body(result.path)
    elif result.snippet is not None:
        hit["snippet"] = result.snippet
    return hit


@search_app.callback(invoke_without_command=True)
def search_command(
    ctx: typer.Context,
    query: str | None = typer.Argument(None, help="Query text (omit with --tags for a tag pull)."),
    type_filter: str | None = typer.Option(None, "--type", help="Filter by note/task type."),
    tags: list[str] | None = _TAGS_OPTION,
    owner: str | None = typer.Option(None, "--owner", help="Filter by exact owner."),
    status: str | None = typer.Option(None, "--status", help="Filter by task status."),
    limit: int = typer.Option(10, "--limit", help="Cap the number of hits."),
    threshold: float | None = typer.Option(
        None, "--threshold", help="Min score to keep (default from [search].threshold)."
    ),
    meta_only: bool = typer.Option(False, "--meta-only", help="Omit body/snippet from hits."),
    full: bool = typer.Option(False, "--full", help="Include the full Markdown body per hit."),
) -> None:
    """Search notes + tasks. ``--tags`` (no query) pulls by tag; a query scores by match."""
    config = load_config()
    quiet = bool(getattr(ctx.obj, "quiet", False))
    tag_list = list(tags) if tags else None

    if query is None:
        # No query → frontmatter tag pull (meta-only by nature; score 1.0).
        results = tagpull(
            config,
            tags=tag_list,
            type_filter=type_filter,
            owner=owner,
            status=status,
            limit=limit,
        )
    else:
        # A query → hybrid indexed recall when available, else substring fallback.
        effective_threshold = threshold if threshold is not None else config.search.threshold
        results = _query_search(
            config,
            query,
            type_filter=type_filter,
            tags=tag_list,
            owner=owner,
            status=status,
            limit=limit,
            threshold=effective_threshold,
            quiet=quiet,
        )

    payload = [_hit_dict(result, meta_only=meta_only, full=full) for result in results]
    typer.echo(json.dumps(payload))
