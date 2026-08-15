"""``shards search`` command surface — the recall verb.

``search`` is one leaf command (not a sub-verb group), so it is a Typer whose
callback *is* the command: ``shards search "<q>"`` and ``shards search --tags x``
both land here. Routing (product R2/R4):

* no query, ``--tags`` (or nothing) → :func:`~shards.index.tagpull.tagpull`
  (frontmatter-only, ``score=1.0``, meta-only);
* a query → hybrid recall via :func:`~shards.index.indexed_client.search` **when**
  ``[search].hybrid`` is on *and* the warm daemon is up (the search/tech.md
  degradation matrix); otherwise, or if ``indexed`` is unavailable
  (``FileNotFoundError`` / non-zero exit), the substring
  :func:`~shards.index.fallback.search_fallback`, which emits its own single stderr
  degradation notice.

Output is JSON by design (search is a machine-first path): a list of hits shaped
``{id, type, title, score, tags?, owner?, updated?, snippet?, path}``.
``--meta-only`` drops the body/snippet; ``--full`` swaps the snippet for the
complete Markdown body. Infrastructure notices go to stderr only, never into the
JSON payload.

``--health`` is a status check, not a recall path: it reports (JSON, via
:func:`~shards.core.search.search_health`) whether hybrid ``indexed`` is
actually reachable/in-use right now versus the substring fallback, so silent
degradation stops being silent. It short-circuits before any query/tag-pull
runs and never shells ``indexed`` itself, so it works with ``indexed`` absent.
"""

from __future__ import annotations

import json

import typer

from shards.cli._errors import cli_errors
from shards.core.search import hit_dict, query_search, search_health
from shards.index.tagpull import tagpull
from shards.schemas.config import load_config

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
        None,
        "--threshold",
        help="Min score to keep (unset: [search].threshold if explicit, else the "
        "fallback's own floor).",
    ),
    meta_only: bool = typer.Option(False, "--meta-only", help="Omit body/snippet from hits."),
    full: bool = typer.Option(False, "--full", help="Include the full Markdown body per hit."),
    health: bool = typer.Option(
        False,
        "--health",
        help="Report indexed reachability vs. substring fallback (JSON), then exit.",
    ),
) -> None:
    """Search notes + tasks. ``--tags`` (no query) pulls by tag; a query scores by match."""
    config = load_config()
    quiet = bool(getattr(ctx.obj, "quiet", False))

    if health:
        # Status check, not a recall path: report the gates and exit before any
        # query/tag-pull runs, regardless of what else was passed on the line.
        typer.echo(json.dumps(search_health(config)))
        return

    tag_list = list(tags) if tags else None

    with cli_errors():
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
            # ``None`` propagates when neither the flag nor the config key was set
            # explicitly, so the substring fallback applies its own floor rather than
            # a silently-defaulted cutoff (root tech.md § B5).
            if threshold is not None:
                effective_threshold = threshold
            elif config.search.threshold_explicit():
                effective_threshold = config.search.threshold
            else:
                effective_threshold = None
            results = query_search(
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
        payload = [hit_dict(result, meta_only=meta_only, full=full) for result in results]

    typer.echo(json.dumps(payload))
