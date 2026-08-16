"""FastMCP server — the agent-facing ``shards_*`` tool surface (memory/1).

This is the MCP mirror of the *safe* CLI verbs. It is deliberately thin: every
tool loads the config, routes straight through the existing ``core`` domain logic
(and ``daemon/client.py`` for the list-shaped reads, the recent-activity lens and
search), and returns a JSON-serialisable dict. There is **no** parallel
re-implementation of note, task, or search behaviour — the tools call the same
functions the CLI does, including the same warm-index-or-fallback client verbs, so
the two surfaces cannot drift.

Unlike the CLI, MCP parameters are *typed fields*, not flag strings: ``shards_note_get``
takes ``id: str``, not ``--id <id>``. Each tool carries an MCP annotation
declaring its effect, so an agent runtime can reason about safety before calling:

* **read-only** (``readOnlyHint``) — ``shards_note_get`` / ``shards_note_list`` /
  ``shards_task_get`` / ``shards_task_list`` / ``shards_search`` /
  ``shards_recent_activity`` / ``shards_build_context`` / ``shards_graph`` /
  ``shards_project`` / ``shards_session_start`` (the warm-start composite —
  team-awareness/10 — reads three lenses and writes nothing) /
  ``shards_health`` (agent-usability/4 — recall-path reachability, not vault
  contents or admin state; see below for why this one is exposed and
  ``status`` is not);
* **idempotent** (``idempotentHint``) — ``shards_note_update`` / ``shards_task_claim`` /
  ``shards_task_finish`` / ``shards_task_update`` / ``shards_task_release``
  (re-running lands the same state — a release re-applied to an already-open
  task is a no-op, mirroring ``shards_task_claim``'s same-agent no-op);
* **write** (no special hint) — ``shards_note_new`` / ``shards_note_append`` /
  ``shards_task_new`` / ``shards_task_append`` (each call is a genuine mutation,
  not idempotent replay — appending the same text twice appends it twice);
* **destructive** (``destructiveHint``) — ``shards_task_cancel`` (a one-way lifecycle move).

The unsafe / administrative surface is **withheld**: neither delete verb, no daemon
controls, no ``reindex`` or ``status``. Those never reach an agent through MCP.
``shards_task_release`` *does* ship here (team-awareness/10 — release is a shipped
verb, not graph work; see root ``AGENTS.md`` §6 and team-awareness/tech.md §
"MCP parity"), but its ``--force`` override does not: owner identity is trusted
local input, not an authorization boundary (root ``AGENTS.md`` §6), so breaking a
peer's claim stays a human/CLI action, never something an agent can trigger
through this surface.

``shards_health`` (agent-usability/4) is exposed despite the withheld-admin rule
above because it answers a different question than ``status``: not "what is in
the vault / is the daemon-as-admin-surface up", but "would my next search hit
real ``indexed`` recall or the substring fallback right now" — a fact an agent
needs in order to *interpret* its own search results correctly, the same
reachability signal the CLI already exposes via ``shards search --health``. It
carries no vault contents, no daemon control verb, and no way to mutate
anything — a pure read of :func:`~shards.core.search.search_health`'s four
static gates, used *without* running a query. ``shards_search`` results carry
a related but distinct signal per hit: a ``mode`` field (``"indexed"`` /
``"fallback"``) added only when a query ran a real recall (never on a
tag-only pull, which is warm-index-vs-cold-scan, an unrelated and
non-degrading distinction). Unlike ``shards_health``'s gate prediction, the
per-hit ``mode`` is *observed* — :func:`~shards.core.search.query_search`
reports the branch it actually took, not the branch the static gates predict
it would take (round-1 review, Finding 1: the gates alone cannot see a
genuine ``indexed`` runtime failure that isn't a missing binary, so a
prediction could confidently mislabel a substring hit as ranked recall) — so
an agent that already has a result set does not need a second
``shards_health`` call just to know whether to trust it, and gets a stronger
guarantee than that second call could give anyway. The marker is MCP-only:
the CLI's ``hit_dict`` shape (:func:`shards.core.search.hit_dict`) stays
unchanged for existing scripts, and a CLI caller already has ``shards search
--health`` and stderr degradation notices — this surface never had that
second channel, which is the whole reason the field exists here.

Tool functions are defined as plain module-level callables and registered on the
app afterwards, so they stay directly importable and unit-testable while the app
introspection still reports the correct names, schemas, and annotations.

The app also carries an ``instructions`` block (agent-usability/1) — built once at
import time by :func:`shards.mcp.instructions.build_instructions` from a guarded
config load (:func:`_startup_config`). It is the one artifact every MCP client
receives on connect, before any tool call, including Cowork sessions that never
read a local skill directory; see ``shards/mcp/instructions.py`` for its content
and phrasing constraints.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from functools import wraps
from pathlib import Path
from typing import Annotated, Any, Literal

from fastmcp import FastMCP
from fastmcp.exceptions import ToolError
from pydantic import Field

from shards.core.errors import ShardsError
from shards.core.lenses import (
    SESSION_SINCE,
    ProjectNotFoundError,
    SeedNotFoundError,
    as_effective_agent,
    build_context,
    graph_query,
    project_view,
    recent_activity,
    session_mentions,
    session_start_entries,
)
from shards.core.notes import (
    TAG_SPEC_SEMANTICS,
    AmbiguousSlugError,
    NoteNotFoundError,
    NoteView,
    append_note,
    create_note,
    get_note,
    update_note,
)
from shards.core.notes import (
    find_duplicate_title as find_duplicate_note_title,
)
from shards.core.search import hit_dict, query_search, resolve_effective_threshold, search_health
from shards.core.tasks import (
    ClaimConflictError,
    TaskNotFoundError,
    TaskView,
    append_task,
    cancel_task,
    claim_task,
    create_task,
    finish_task,
    get_task,
    release_task,
    update_task,
)
from shards.core.tasks import (
    find_duplicate_title as find_duplicate_task_title,
)
from shards.daemon.client import DaemonClient
from shards.index.warm import DEFAULT_RECENT_LIMIT
from shards.mcp.instructions import build_instructions
from shards.schemas.config import Config, ConfigMissingError, load_config
from shards.schemas.note import Note, NoteType
from shards.schemas.task import TaskStatus
from shards.storage.locks import LockError


def _startup_config() -> Config | None:
    """Guarded config load for the ``instructions`` block (agent-usability/1).

    A missing ``config.toml`` raises :class:`~shards.schemas.config.ConfigMissingError`
    (agent-usability/5 replaced the former bare ``SystemExit(2)`` — a
    ``BaseException`` neither this ``except Exception`` nor ``_guarded`` below
    could ever catch); a malformed one raises ``msgspec.ValidationError``.
    Neither may stop the server from starting — :func:`build_instructions`
    renders the fully-degraded block instead.

    The individual ``shards_*`` tools *do* still fail per the normal error
    contract once actually called: ``ConfigMissingError`` is a
    :class:`~shards.core.errors.ShardsError`, a plain ``Exception``, so every
    registered tool's ``_guarded`` wrapper catches it exactly like any other
    domain exception and raises a structured ``ToolError`` naming
    ``shards init`` — proven by driving real registered tools with a missing
    config in ``tests/memory/test_errors.py``, not merely asserted here (a
    prior version of this docstring made that claim without the coverage to
    back it; agent-usability/5 closes that gap).
    """
    try:
        return load_config()
    except Exception:
        return None


app = FastMCP("shards", instructions=build_instructions(_startup_config()))


# --------------------------------------------------------------------------- #
# Serialisation helpers                                                       #
# --------------------------------------------------------------------------- #


def _entry(model: Note, *, body: str | None, path: str) -> dict[str, Any]:
    """Render a note/task view as a JSON-safe dict: frontmatter (+ body) + path."""
    data: dict[str, Any] = model.model_dump(mode="json")
    if body is not None:
        data["body"] = body
    data["path"] = str(path)
    return data


def _note_get_dict(view: NoteView) -> dict[str, Any]:
    return _entry(view.note, body=view.body, path=str(view.path))


def _task_get_dict(view: TaskView) -> dict[str, Any]:
    return _entry(view.task, body=view.body, path=str(view.path))


# --------------------------------------------------------------------------- #
# Read-only tools                                                             #
# --------------------------------------------------------------------------- #


def shards_note_get(
    id: Annotated[
        str,
        Field(
            description=(
                "Note id (n-...) or a title slug (case/whitespace-normalized match). "
                "A slug matching more than one note raises an error naming the candidates."
            )
        ),
    ],
) -> dict[str, Any]:
    """Read one note by id or title slug: frontmatter, body, and path."""
    config = load_config()
    return _note_get_dict(get_note(config, id))


def shards_note_list(
    tags: Annotated[
        list[str] | None,
        Field(description="Keep only notes carrying every one of these tags (AND)."),
    ] = None,
    any_tag: Annotated[
        bool, Field(description="Match any tag in tags instead of requiring all of them.")
    ] = False,
    owner: Annotated[
        str | None,
        Field(
            description=(
                "Exact match on the note's owner field — trusted local input, "
                "not a verified caller identity."
            )
        ),
    ] = None,
    note_type: Annotated[
        NoteType | None, Field(description="Restrict to one note type; omit to list every type.")
    ] = None,
    since: Annotated[
        str | None,
        Field(
            description=(
                "Recency floor on updated: duration shorthand ('7d', '12h', '2w') "
                "or an ISO-8601 date/datetime; omit for no floor."
            )
        ),
    ] = None,
    sort: Annotated[
        str,
        Field(
            description=(
                "'updated'/'created' (newest first) or 'title' (A-Z); an "
                "unrecognised value is rejected."
            )
        ),
    ] = "updated",
    limit: Annotated[int, Field(description="Maximum rows returned.")] = 20,
) -> list[dict[str, Any]]:
    """List shards notes with tag/owner/type/recency filters, sorted and capped."""
    config = load_config()
    views = DaemonClient().note_list(
        config,
        tags=tags,
        any_tag=any_tag,
        owner=owner,
        note_type=note_type,
        since=since,
        sort=sort,
        limit=limit,
    )
    return [_entry(v.note, body=None, path=str(v.path)) for v in views]


def shards_task_get(
    id: Annotated[
        str, Field(description="Task id (t-...) — id-only; unlike notes, no title-slug match.")
    ],
) -> dict[str, Any]:
    """Read one task by id: frontmatter, body, and path."""
    config = load_config()
    return _task_get_dict(get_task(config, id))


def shards_task_list(
    status: Annotated[
        str | None,
        Field(
            description=(
                "Comma-separated status filter, e.g. 'open,claimed' (union match). "
                "Each token must be one of open/claimed/done/cancelled."
            )
        ),
    ] = None,
    owner: Annotated[
        str | None,
        Field(
            description=(
                "Exact match on the task's owner field (who it is accountable to) — "
                "trusted local input, not a verified caller identity."
            )
        ),
    ] = None,
    mine: Annotated[
        bool,
        Field(
            description=("Restrict to tasks where owner or claimed_by equals the configured agent.")
        ),
    ] = False,
    tags: Annotated[
        list[str] | None,
        Field(description="Keep only tasks carrying every one of these tags (AND)."),
    ] = None,
    any_tag: Annotated[
        bool, Field(description="Match any tag in tags instead of requiring all of them.")
    ] = False,
    project: Annotated[
        str | None,
        Field(
            description=(
                "Exact match on the task's project soft link — an unvalidated note id, "
                "never checked for existence."
            )
        ),
    ] = None,
    since: Annotated[
        str | None,
        Field(
            description=(
                "Recency floor on updated: duration shorthand ('7d', '12h', '2w') or an "
                "ISO-8601 date/datetime; keeps updated >= since."
            )
        ),
    ] = None,
    stale: Annotated[
        str | None,
        Field(
            description=(
                "Recency ceiling on updated — the inverse of since: keeps updated < stale. "
                "Conjunctive with since when both are given."
            )
        ),
    ] = None,
    available: Annotated[
        bool, Field(description="Takeable work only: status == 'open' and claimed_by is unset.")
    ] = False,
    sort: Annotated[
        str | None,
        Field(
            description=(
                "'updated'/'created' (newest first), 'title' (A-Z), or 'priority' "
                "(high, then normal, then low, then unprioritized). Omit to default to "
                "'priority' under available=True and 'updated' otherwise."
            )
        ),
    ] = None,
    limit: Annotated[int, Field(description="Maximum rows returned.")] = 20,
) -> list[dict[str, Any]]:
    """List shards tasks (open and done) with status/owner/mine/project filters, sorted.

    ``status`` accepts a comma-separated set for a union filter (e.g.
    ``"open,claimed"`` — team-awareness/4), passed straight through to the same
    parser the CLI's ``--status`` uses. ``since`` is a recency floor (updated
    within the window); ``stale`` (team-awareness/5) is its inverse — a ceiling,
    "not touched within the window" — and the two are conjunctive when both are
    given. ``available`` narrows to takeable work: ``status == "open"`` and
    unclaimed. ``sort`` is ``updated`` (default) / ``created`` / ``title`` /
    ``priority``; when omitted, it defaults to ``priority`` under ``available``
    and ``updated`` otherwise — the same default ``cli/task.py``'s ``list``
    applies, so an agent asking for "what's available" gets it priority-ordered
    without asking for the sort explicitly.
    """
    config = load_config()
    sort_field = sort if sort is not None else ("priority" if available else "updated")
    views = DaemonClient().task_list(
        config,
        status=status,
        owner=owner,
        mine=mine,
        tags=tags,
        any_tag=any_tag,
        project=project,
        since=since,
        stale=stale,
        available=available,
        sort=sort_field,
        limit=limit,
    )
    return [_entry(v.task, body=None, path=str(v.path)) for v in views]


def shards_search(
    query: Annotated[
        str | None,
        Field(
            description=(
                "Search text, scored and ranked. Omit for a tag-only pull (unscored, "
                "meta_only by nature) instead of a search."
            )
        ),
    ] = None,
    type_filter: Annotated[
        str | None,
        Field(
            description=(
                "Exact match on frontmatter type: a note type (note/log/decision/"
                "reference/project) or 'task'."
            )
        ),
    ] = None,
    tags: Annotated[
        list[str] | None,
        Field(description="Keep only rows carrying every one of these tags (AND)."),
    ] = None,
    owner: Annotated[
        str | None,
        Field(
            description=(
                "Exact match on owner — trusted local input, not a verified caller identity."
            )
        ),
    ] = None,
    status: Annotated[
        TaskStatus | None,
        Field(
            description=(
                "Exact task-status filter. Notes carry no status field, so this excludes "
                "every note hit whenever it is set."
            )
        ),
    ] = None,
    limit: Annotated[int, Field(description="Maximum hits returned.")] = 10,
    threshold: Annotated[
        float | None,
        Field(
            description=(
                "Minimum score (0-1) a hit must clear to be kept. Unset defers to "
                "[search].threshold, or the substring fallback's own floor when neither "
                "is set."
            )
        ),
    ] = None,
    meta_only: Annotated[
        bool, Field(description="Drop the snippet entirely (id/type/title/score/path only).")
    ] = False,
    full: Annotated[
        bool,
        Field(
            description=(
                "Return the whole body as the snippet instead of a short excerpt; "
                "ignored when meta_only is set."
            )
        ),
    ] = False,
) -> list[dict[str, Any]]:
    """Recall across notes + tasks: tag pull (no query) or scored match (query).

    A query hit carries a ``mode`` field (``"indexed"`` / ``"fallback"``,
    agent-usability/4) naming which engine actually answered — the MCP surface
    has no stderr an agent reads, so this is the only channel a degraded
    substring result has to identify itself as degraded (``query_search``
    already suppresses its own notice here via ``quiet=True``; without this
    field a fallback hit was indistinguishable from a ranked one). ``mode`` is
    the branch :func:`~shards.core.search.query_search` *actually took* for
    this call, not a prediction from :func:`~shards.core.search.search_health`'s
    static gates — those cannot see a genuine ``indexed`` runtime failure
    (round-1 review, Finding 1), so this stays observed, never recomputed
    independently. Call ``shards_health`` for the standalone reachability
    check (no query, no results, just the gates). A tag-only pull (no
    ``query``) never carries ``mode``: it is served from the warm daemon index
    or an equivalent cold folder scan, a daemon-liveness distinction that
    never degrades recall quality, unlike the indexed/fallback split a real
    query makes.
    """
    config = load_config()
    if query is None:
        results = DaemonClient().tag_pull(
            config,
            tags=tags,
            type_filter=type_filter,
            owner=owner,
            status=status,
            limit=limit,
        )
        return [hit_dict(result, meta_only=meta_only, full=full) for result in results]

    # ``None`` propagates when neither the caller nor the config key set
    # threshold explicitly, so the substring fallback applies its own floor
    # rather than a silently-defaulted cutoff (root tech.md § B5).
    effective_threshold = resolve_effective_threshold(threshold, config)
    # ``mode`` is the engine ``query_search`` actually took (agent-usability/4,
    # round-1 review Finding 1) — observed from its return value, not
    # predicted from ``search_health``'s static gates. The gates cannot see a
    # genuine ``indexed`` runtime failure (corrupt collection, resource
    # exhaustion, a non-"binary absent" non-zero exit); a value computed from
    # them alone could report "indexed" over a hit that actually came from the
    # substring fallback — a second, independent call to ``search_health``
    # here was exactly that bug.
    results, mode = query_search(
        config,
        query,
        type_filter=type_filter,
        tags=tags,
        owner=owner,
        status=status,
        limit=limit,
        threshold=effective_threshold,
        quiet=True,
    )
    hits = [hit_dict(result, meta_only=meta_only, full=full) for result in results]
    for hit in hits:
        hit["mode"] = mode
    return hits


def shards_health() -> dict[str, Any]:
    """Report indexed reachability vs. substring fallback recall, right now.

    A pure delegate to :func:`~shards.core.search.search_health` — the exact
    call ``shards search --health`` already makes, so the two surfaces read
    off one implementation and cannot drift apart. Returns ``mode``
    (``"indexed"`` only when every gate below is open, ``"fallback"``
    otherwise), the individual gates (``hybrid_configured``, ``collection``,
    ``daemon_up``, ``indexed_binary_available``), and — only when degraded — a
    terse ``reason`` naming the first closed gate. Never raises, even with
    ``indexed`` entirely absent from ``PATH``; never shells ``indexed``
    itself. Takes no parameters: the answer depends only on the current
    config and environment, nothing a caller supplies.

    Distinct from the withheld ``shards_status``: this reports recall-path
    reachability for *interpreting search results*, never vault contents or
    daemon admin state (see the module docstring's "unsafe / administrative
    surface" note for the line this stays on the safe side of).
    """
    config = load_config()
    return search_health(config)


def shards_recent_activity(
    since: Annotated[
        str | None,
        Field(
            description=(
                "Recency floor: duration shorthand ('7d', '12h', '2w') or an ISO-8601 "
                "date/datetime; omit for no floor."
            )
        ),
    ] = None,
    owner: Annotated[
        str | None,
        Field(
            description=(
                "Restrict to rows owned by this agent (exact match) — trusted local "
                "input, not a verified caller identity."
            )
        ),
    ] = None,
    mine: Annotated[
        bool,
        Field(
            description=(
                "Restrict to rows owned by (or, for tasks, claimed by) the configured agent."
            )
        ),
    ] = False,
    limit: Annotated[int, Field(description="Maximum rows returned.")] = DEFAULT_RECENT_LIMIT,
) -> list[dict[str, Any]]:
    """Recent vault changes (newest first), each row carrying identity.

    ``{id, type, title, path, mtime, owner, claimed_by}`` — team-awareness/6.
    """
    config = load_config()
    return recent_activity(config, since=since, owner=owner, mine=mine, limit=limit)


def shards_build_context(
    seed_id: Annotated[
        str,
        Field(
            description=(
                "Seed note id (n-...) or task id (t-...) to expand from; must resolve "
                "or the call errors."
            )
        ),
    ],
    depth: Annotated[
        int,
        Field(
            description=(
                "BFS hops from the seed (0 = seed only, 1 = seed plus its direct related "
                "entries). Each extra hop reads every newly discovered node off disk, so "
                "cost grows with the graph's branching factor — keep this small."
            )
        ),
    ] = 1,
) -> list[dict[str, Any]]:
    """Expand the ``related`` graph around a seed id (BFS to depth, seed first)."""
    config = load_config()
    return build_context(config, seed_id, depth=depth)


def shards_graph(
    seed_id: Annotated[
        str,
        Field(
            description=(
                "Seed note id (n-...) or task id (t-...) to query from; must resolve "
                "or the call errors."
            )
        ),
    ],
    depth: Annotated[
        int,
        Field(
            description=(
                "BFS hops from the seed (0 = seed only). Each extra hop reads every "
                "newly discovered node off disk, so cost grows with the graph's "
                "branching factor — keep this small."
            )
        ),
    ] = 1,
    direction: Annotated[
        Literal["out", "in", "both"],
        Field(
            description=(
                "'out' (forward related links) — 'in' (who links to this node — "
                "backlinks/notify) and 'both' additionally scan the whole vault once "
                "to build the backlink index."
            )
        ),
    ] = "out",
) -> dict[str, Any]:
    """Query what's connected to a seed id: ``{seed, nodes, edges}`` (BFS to depth).

    ``direction`` is ``"out"`` (default, forward ``related``), ``"in"`` (who
    mentions this node — backlinks/notify), or ``"both"``.
    """
    config = load_config()
    return graph_query(config, seed_id, depth=depth, direction=direction).to_dict()


def shards_project(
    project_id: Annotated[
        str,
        Field(
            description=(
                "Project note id (n-...) or title slug. Every task whose project field "
                "points at it is returned, regardless of status — project is a soft "
                "link, never validated against type: project."
            )
        ),
    ],
) -> dict[str, Any]:
    """Show a project note and the tasks scoped to it: ``{project, tasks}``."""
    config = load_config()
    return project_view(config, project_id).to_dict()


def shards_session_start(
    owner: Annotated[
        str | None,
        Field(
            description=(
                "Show this agent's warm start instead of the caller's own — substitutes "
                "the effective identity for every source (tasks, mentions, activity)."
            )
        ),
    ] = None,
    team: Annotated[
        bool,
        Field(
            description=(
                "Widen the recent-activity section to the whole team instead of just the "
                "effective agent's rows. The task queue and mentions always stay scoped "
                "to the effective agent."
            )
        ),
    ] = False,
    meta_only: Annotated[
        bool,
        Field(
            description=(
                "Drop task bodies from the task section. Mentions and activity rows "
                "never carry a body regardless."
            )
        ),
    ] = False,
) -> list[dict[str, Any]]:
    """Warm-start payload: my open/claimed tasks + mentions of me + recent activity.

    The MCP mirror of ``shards session-start`` (team-awareness/10) — the highest-
    value tool in this parity sweep, since it is the only way an MCP-only agent
    (one with no CLI stdout to read) sees its own queue and the mentions
    delivered to it. Composes the same three read-only lenses the CLI command
    does — ``task_list(mine=True)``, :func:`~shards.core.lenses.session_mentions`
    (inbound links to nodes the caller owns or has claimed), and
    ``recent_activity(since=7d)`` — via the shared
    :func:`~shards.core.lenses.session_start_entries` composer, so the ordering
    (tasks, then mentions, then remaining activity, newest-first, deduped by id)
    and the ``reason`` key on every entry match the CLI exactly.

    ``owner`` substitutes the effective identity for every source (via
    :func:`~shards.core.lenses.as_effective_agent`) — "what would that agent's
    warm start show" — the same substitution ``--owner`` performs on the CLI
    side; ``team`` drops the identity filter on the activity half only (the task
    half, and the mentions target set built from it, always stay the effective
    agent's own). ``meta_only`` omits task bodies for the token-budget path;
    mentions and activity rows never carry a body regardless. Entirely
    daemon-independent (or degrades transparently), so this reads identically
    with the daemon down.
    """
    config = load_config()
    effective_config = as_effective_agent(config, owner)
    me = effective_config.agent

    task_views = DaemonClient().task_list(effective_config, mine=True, limit=None)
    note_views = DaemonClient().note_list(effective_config, owner=me, limit=None) if me else []
    mentions = session_mentions(
        effective_config, task_views, note_views, me=me, since=SESSION_SINCE
    )
    activity = recent_activity(
        effective_config,
        since=SESSION_SINCE,
        owner=None,
        mine=not team,
        limit=DEFAULT_RECENT_LIMIT,
    )
    return session_start_entries(task_views, activity, mentions, meta_only=meta_only)


# --------------------------------------------------------------------------- #
# Write tools (no special hint)                                               #
# --------------------------------------------------------------------------- #


def _with_warnings(payload: dict[str, Any], existing_id: str | None) -> dict[str, Any]:
    """Attach the structured ``warnings`` key (R9) — empty unless a title collided.

    The MCP mirror of the CLI's stderr line (``cli/note.py``/``cli/task.py``):
    there is no stream an agent reads on this surface, so the same advisory
    travels in the JSON result instead — never silently dropped, never mixed
    into the payload's own fields.
    """
    payload["warnings"] = (
        [f"duplicate title, also used by {existing_id}"] if existing_id is not None else []
    )
    return payload


def shards_note_new(
    title: Annotated[
        str,
        Field(
            description=(
                "Note title. A slug-normalized duplicate against an existing note "
                "returns a non-blocking warning, not an error."
            )
        ),
    ],
    note_type: Annotated[
        NoteType,
        Field(
            description=(
                "Note kind — also selects the storage folder: notes/ (note) or "
                "notes/{logs,decisions,references,projects}/ for the others."
            )
        ),
    ] = "note",
    tags: Annotated[list[str] | None, Field(description="Initial tag list.")] = None,
    owner: Annotated[
        str | None,
        Field(
            description=(
                "Defaults to the configured agent. An explicit value must be in "
                "[tasks].collections when that roster is non-empty — a value check, "
                "not a verified identity."
            )
        ),
    ] = None,
    body: Annotated[
        str,
        Field(description="Initial Markdown body; [[wikilinks]] inside populate related."),
    ] = "",
) -> dict[str, Any]:
    """Create a note (routed by type) and return its frontmatter plus any warnings."""
    config = load_config()
    # Checked before the write, so a hit names the *prior* id (R9); non-blocking
    # either way — see shards.core.notes.find_duplicate_title.
    existing_id = find_duplicate_note_title(config, title)
    note = create_note(config, title, note_type=note_type, tags=tags, owner=owner, body=body)
    return _with_warnings(note.model_dump(mode="json"), existing_id)


def shards_note_append(
    target: Annotated[str, Field(description="Note id (n-...) or title slug to append to.")],
    text: Annotated[str, Field(description="Text appended verbatim; stored, never interpreted.")],
    section: Annotated[
        str | None,
        Field(
            description=(
                "Append under this '## {section}' heading, creating it at the end of "
                "the body if absent. Omit to append at the very end of the body instead."
            )
        ),
    ] = None,
    timestamp: Annotated[
        bool,
        Field(
            description=(
                "Prefix the appended block with '<iso> — <agent>', naming the agent "
                "making this call — not the note's owner."
            )
        ),
    ] = False,
) -> dict[str, Any]:
    """Append text to a note's body (optionally under a section / timestamped)."""
    config = load_config()
    note = append_note(config, target, text, section=section, timestamp=timestamp)
    return note.model_dump(mode="json")


def shards_task_new(
    title: Annotated[
        str,
        Field(
            description=(
                "Task title. A slug-normalized duplicate against an existing task "
                "returns a non-blocking warning, not an error."
            )
        ),
    ],
    priority: Annotated[
        Literal["high", "normal", "low"] | None,
        Field(description="Sort weight; unset ranks last under sort='priority'."),
    ] = None,
    tags: Annotated[list[str] | None, Field(description="Initial tag list.")] = None,
    owner: Annotated[
        str | None,
        Field(
            description=(
                "Defaults to the configured agent. An explicit value must be in "
                "[tasks].collections when that roster is non-empty — a value check, "
                "not a verified identity."
            )
        ),
    ] = None,
    body: Annotated[
        str,
        Field(description="Initial Markdown body; [[wikilinks]] inside populate related."),
    ] = "",
    project: Annotated[
        str | None,
        Field(
            description=(
                "Optional soft link to a project note's id — a plain string, never "
                "validated or checked for existence."
            )
        ),
    ] = None,
    blocks: Annotated[
        list[str] | None,
        Field(
            description=(
                "Task ids this task blocks. Recorded but inert in v1 — no readiness "
                "or gating logic reads this yet."
            )
        ),
    ] = None,
    blocked_by: Annotated[
        list[str] | None,
        Field(
            description=(
                "Task ids blocking this task. Recorded but inert in v1 — does not "
                "prevent claim/finish/cancel."
            )
        ),
    ] = None,
) -> dict[str, Any]:
    """Create a task in tasks/open/ (status open, unclaimed) and return it plus any warnings."""
    config = load_config()
    # Checked before the write, so a hit names the *prior* id (R9); non-blocking
    # either way — see shards.core.tasks.find_duplicate_title.
    existing_id = find_duplicate_task_title(config, title)
    task = create_task(
        config,
        title,
        priority=priority,
        tags=tags,
        owner=owner,
        body=body,
        project=project,
        blocks=blocks,
        blocked_by=blocked_by,
    )
    return _with_warnings(task.model_dump(mode="json"), existing_id)


def shards_task_append(
    task_id: Annotated[str, Field(description="Task id (t-...) — id-only.")],
    text: Annotated[str, Field(description="Text appended verbatim; stored, never interpreted.")],
    section: Annotated[
        str | None,
        Field(
            description=(
                "Append under this '## {section}' heading, creating it at the end of "
                "the body if absent. Omit to append at the very end of the body instead."
            )
        ),
    ] = None,
    timestamp: Annotated[
        bool,
        Field(
            description=(
                "Prefix the appended block with '<iso> — <agent>', naming the agent "
                "making this call — not the task's owner."
            )
        ),
    ] = False,
) -> dict[str, Any]:
    """Append text to a task's body (no status/folder change; mirrors note append)."""
    config = load_config()
    task = append_task(config, task_id, text, section=section, timestamp=timestamp)
    return task.model_dump(mode="json")


# --------------------------------------------------------------------------- #
# Idempotent tools                                                            #
# --------------------------------------------------------------------------- #


def shards_note_update(
    target: Annotated[str, Field(description="Note id (n-...) or title slug.")],
    tags: Annotated[str | None, Field(description=TAG_SPEC_SEMANTICS)] = None,
    new_type: Annotated[
        NoteType | None,
        Field(
            description=(
                "Moves the file into the matching folder; omit to leave the type unchanged."
            )
        ),
    ] = None,
) -> dict[str, Any]:
    """Update a note's fields (tags, type — moving its folder) and bump updated."""
    config = load_config()
    note = update_note(config, target, tags=tags, new_type=new_type)
    return note.model_dump(mode="json")


def shards_task_claim(
    task_id: Annotated[str, Field(description="Task id (t-...) to claim.")],
    claimer: Annotated[
        str | None,
        Field(
            description=(
                "Acting agent identity; defaults to [core].agent. A same-agent reclaim "
                "is a no-op; a different agent already holding it raises a conflict; "
                "claiming a terminal (done/cancelled) task is also a no-op."
            )
        ),
    ] = None,
) -> dict[str, Any]:
    """Claim a task for an agent (atomic test-and-set; same-owner reclaim is a no-op)."""
    config = load_config()
    who = claimer if claimer is not None else config.agent
    if not who:
        raise ValueError("no agent identity: pass claimer or set [core].agent")
    task = claim_task(config, task_id, who)
    return task.model_dump(mode="json")


def shards_task_release(
    task_id: Annotated[str, Field(description="Task id (t-...) to release.")],
    owner: Annotated[
        str | None,
        Field(
            description=(
                "Acting agent identity; defaults to [core].agent. Releasing your own "
                "claim, an already-open task, or a terminal task are all idempotent "
                "no-ops. Releasing someone else's live claim raises a conflict — this "
                "surface carries no force override."
            )
        ),
    ] = None,
) -> dict[str, Any]:
    """Release a claim, returning the task to open (atomic compare-and-clear; idempotent).

    Ships as of team-awareness/10 (previously withheld as a Phase-3 item — see
    the module docstring). ``owner`` names the acting agent, falling back to
    ``[core].agent`` exactly like :func:`shards_task_claim`'s ``claimer``.

    Deliberately has **no** ``force`` parameter: the CLI's ``--force`` breaks a
    claim held by a *different* agent, and owner identity is trusted local
    input, not a verified authorization boundary (root ``AGENTS.md`` §6) — so
    overriding someone else's claim stays a human/CLI action, never something an
    agent can trigger through this surface. Released *by* the holder (or when
    already unclaimed/terminal) is unaffected and stays idempotent.
    """
    config = load_config()
    who = owner if owner is not None else config.agent
    if not who:
        raise ValueError("no agent identity: pass owner or set [core].agent")
    task = release_task(config, task_id, who)
    return task.model_dump(mode="json")


def shards_task_finish(
    task_id: Annotated[str, Field(description="Task id (t-...) to finish.")],
    outcome: Annotated[
        str | None,
        Field(
            description=(
                "Optional outcome text appended under a new '## Outcome' section "
                "before the task moves to tasks/done/. Idempotent: a re-finish never "
                "adds a second section."
            )
        ),
    ] = None,
) -> dict[str, Any]:
    """Finish a task: append an outcome and move it to tasks/done/ (idempotent)."""
    config = load_config()
    task = finish_task(config, task_id, outcome)
    return task.model_dump(mode="json")


def shards_task_update(
    task_id: Annotated[str, Field(description="Task id (t-...) to update.")],
    priority: Annotated[
        Literal["high", "normal", "low"] | None,
        Field(description="Sort weight; unset ranks last under sort='priority'."),
    ] = None,
    tags: Annotated[str | None, Field(description=TAG_SPEC_SEMANTICS)] = None,
    title: Annotated[
        str | None,
        Field(description="New title. Only renames the task; the id never changes."),
    ] = None,
    project: Annotated[
        str | None,
        Field(
            description=(
                "New soft link to a project note's id — a plain string, never "
                "validated or checked for existence."
            )
        ),
    ] = None,
    owner: Annotated[
        str | None,
        Field(
            description=(
                "Reassigns accountability; must be in [tasks].collections when that "
                "roster is non-empty. Never touches claimed_by — use task_claim/"
                "task_release for the execution handle."
            )
        ),
    ] = None,
    blocks: Annotated[
        list[str] | None,
        Field(
            description=(
                "Task ids this task blocks; replaces the whole list verbatim. "
                "Recorded but inert in v1 — no readiness or gating logic reads this yet."
            )
        ),
    ] = None,
    blocked_by: Annotated[
        list[str] | None,
        Field(
            description=(
                "Task ids blocking this task; replaces the whole list verbatim. "
                "Recorded but inert in v1 — does not prevent claim/finish/cancel."
            )
        ),
    ] = None,
) -> dict[str, Any]:
    """Update a task's fields (priority, tags, title, project, owner, blocks/blocked_by).

    ``owner`` reassigns accountability (a validated identity — must be in
    ``[tasks].collections``) and never touches ``claimed_by``: use
    ``shards_task_claim``/``shards_task_release`` for the execution handle.
    """
    config = load_config()
    task = update_task(
        config,
        task_id,
        priority=priority,
        tags=tags,
        title=title,
        project=project,
        owner=owner,
        blocks=blocks,
        blocked_by=blocked_by,
    )
    return task.model_dump(mode="json")


# --------------------------------------------------------------------------- #
# Destructive tools                                                           #
# --------------------------------------------------------------------------- #


def shards_task_cancel(
    task_id: Annotated[str, Field(description="Task id (t-...) to cancel.")],
    reason: Annotated[
        str | None,
        Field(
            description=(
                "Optional reason text appended under a new '## Cancelled' section "
                "before the task moves to tasks/done/. Idempotent: a re-cancel never "
                "adds a second section."
            )
        ),
    ] = None,
) -> dict[str, Any]:
    """Cancel a task: append a reason and move it to tasks/done/ (idempotent)."""
    config = load_config()
    task = cancel_task(config, task_id, reason)
    return task.model_dump(mode="json")


# --------------------------------------------------------------------------- #
# Registration — bind each tool with its MCP annotation class                 #
# --------------------------------------------------------------------------- #

_READ_ONLY: dict[str, Any] = {"readOnlyHint": True}
_IDEMPOTENT: dict[str, Any] = {"idempotentHint": True}
_DESTRUCTIVE: dict[str, Any] = {"destructiveHint": True}


# --------------------------------------------------------------------------- #
# Structured error mapping (agent-usability/5)                                #
# --------------------------------------------------------------------------- #

# One discriminant per exception *class*, checked in this order (most specific
# first) so a subclass never falls through to a less precise ancestor's kind.
# ``kind`` is the field an agent branches on programmatically; it is deliberately
# a closed, small vocabulary rather than the exception's class name, so it stays
# stable even if a class gets renamed/split later.
_KIND_BY_TYPE: tuple[tuple[type[ShardsError], str], ...] = (
    (ConfigMissingError, "config_missing"),
    (ClaimConflictError, "claim_conflict"),
    (LockError, "lock_conflict"),
    (AmbiguousSlugError, "ambiguous_slug"),
    (NoteNotFoundError, "not_found"),
    (TaskNotFoundError, "not_found"),
    (SeedNotFoundError, "not_found"),
    (ProjectNotFoundError, "not_found"),
)
# Fallback for a ShardsError not named above (e.g. a future subclass): derive a
# kind from its exit-code tier rather than leaving it unclassified.
_KIND_BY_CODE: dict[int, str] = {2: "validation", 3: "not_found", 4: "conflict"}

# A terse, actionable next step per kind — never an authorization decision (root
# AGENTS.md §6: identity fields here are trusted local input, not verified), and
# never a restatement of infrastructure internals (.spec/design.md — structured
# fields, not prose, carry the machine-readable part; this is the one prose field
# the shape allows, kept short and purely actionable).
_NEXT_ACTION_BY_KIND: dict[str, str] = {
    "config_missing": "run `shards init` to create a config, then retry",
    "claim_conflict": "pick a different task, wait, or ask the named agent to release it",
    "lock_conflict": "retry shortly — another process is mid-write on this entity",
    "ambiguous_slug": "retry using one of the listed ids instead of the slug",
    "not_found": "check the id and retry, or list to find the right one",
    "validation": "fix the input and retry",
    "conflict": "resolve the conflict and retry",
}

# Exception attributes worth surfacing as their own structured fields when
# present — the domain facts an agent needs to branch on (e.g. ``task_id``,
# ``existing_owner`` from a claim conflict), never folded into the prose
# ``message``. ``Path`` values are stringified for JSON.
_STRUCTURED_ATTRS: tuple[str, ...] = (
    "task_id",
    "existing_owner",
    "id_or_slug",
    "slug",
    "ids",
    "seed_id",
    "project_id",
    "cfg_path",
)


def _error_kind(exc: ShardsError) -> str:
    for cls, kind in _KIND_BY_TYPE:
        if isinstance(exc, cls):
            return kind
    return _KIND_BY_CODE.get(exc.code, "error")


def _structured_fields(exc: Exception) -> dict[str, Any]:
    fields: dict[str, Any] = {}
    for name in _STRUCTURED_ATTRS:
        if not hasattr(exc, name):
            continue
        value = getattr(exc, name)
        fields[name] = str(value) if isinstance(value, Path) else value
    return fields


def _tool_error(kind: str, message: str, **fields: Any) -> ToolError:
    """Build a ``ToolError`` whose message is the JSON-encoded structured payload.

    ``fastmcp.exceptions.ToolError`` carries only a text message — there is no
    separate structured-data channel on a raised tool error — so the
    ``{kind, message, next_action, ...fields}`` contract travels as JSON text,
    parseable by any agent that reads the tool error string. ``kind`` is the
    field to branch on; ``message`` is the same one-line wording the CLI prints
    to stderr for the identical exception; ``next_action`` is a short,
    non-authoritative suggestion (never a command the server executes).
    """
    payload = {
        "kind": kind,
        "message": message,
        "next_action": _NEXT_ACTION_BY_KIND.get(kind, "fix the input and retry"),
        **fields,
    }
    return ToolError(json.dumps(payload))


def _guarded(fn: Callable[..., Any]) -> Callable[..., Any]:
    """The one MCP-boundary exception mapper (core-hardening/3, structured payload
    per agent-usability/5), applied at registration.

    Same exception families and catch order as the CLI mapper
    (``shards.cli._errors.cli_errors``) — ``ShardsError`` (``LockError`` and
    ``ConfigMissingError`` included) first, then a bare ``ValueError`` (also
    covers msgspec's ``ValidationError``, a ``ValueError`` subclass — e.g. an
    unknown owner or an unknown token in a ``status``/``sort`` filter; most
    enum-shaped parameters are schema-typed and rejected by FastMCP's own
    validation before the tool body runs, per the agent-usability/2 sweep —
    then any other ``OSError``. Unlike the CLI, MCP has no process exit code to
    map to, so every branch raises a clean ``fastmcp.exceptions.ToolError``
    whose *message* is a JSON object — ``{kind, message, next_action}`` plus
    whatever of the exception's own fields matter (``task_id``,
    ``existing_owner``, ...; see :data:`_STRUCTURED_ATTRS`) — instead of a bare
    English sentence an agent would have to parse, and instead of letting
    FastMCP's own generic catch-all re-wrap an arbitrary traceback string.

    ``ConfigMissingError`` (agent-usability/5) is what actually closes the
    ``BaseException`` escape this unit fixes: it replaces the former
    ``load_config`` ``SystemExit(2)``, which was a ``BaseException`` neither
    this wrapper's ``except Exception``-rooted branches nor FastMCP's own
    dispatcher (`server.py`'s ``except Exception`` around ``tool._run``) could
    ever have caught — a missing config on an MCP-only machine would have
    escaped the first real tool call as an unhandled crash instead of a clean
    tool error naming ``shards init``.

    The module-level ``shards_*`` functions are left unwrapped so they stay
    directly importable/unit-testable (see the module docstring) — only the
    registered tool goes through this wrapper.
    """

    @wraps(fn)
    def wrapper(*args: Any, **kwargs: Any) -> Any:
        try:
            return fn(*args, **kwargs)
        except ShardsError as exc:
            raise _tool_error(_error_kind(exc), str(exc), **_structured_fields(exc)) from exc
        except ValueError as exc:
            raise _tool_error("validation", str(exc)) from exc
        except OSError as exc:
            raise _tool_error("io_error", f"io error: {exc}") from exc

    return wrapper


def _register() -> None:
    """Register every ``shards_*`` tool with its annotation; called once at import."""
    # Read-only.
    app.tool(_guarded(shards_note_get), annotations=_READ_ONLY)
    app.tool(_guarded(shards_note_list), annotations=_READ_ONLY)
    app.tool(_guarded(shards_task_get), annotations=_READ_ONLY)
    app.tool(_guarded(shards_task_list), annotations=_READ_ONLY)
    app.tool(_guarded(shards_search), annotations=_READ_ONLY)
    app.tool(_guarded(shards_health), annotations=_READ_ONLY)
    app.tool(_guarded(shards_recent_activity), annotations=_READ_ONLY)
    app.tool(_guarded(shards_build_context), annotations=_READ_ONLY)
    app.tool(_guarded(shards_graph), annotations=_READ_ONLY)
    app.tool(_guarded(shards_project), annotations=_READ_ONLY)
    app.tool(_guarded(shards_session_start), annotations=_READ_ONLY)
    # Write (no special hint).
    app.tool(_guarded(shards_note_new))
    app.tool(_guarded(shards_note_append))
    app.tool(_guarded(shards_task_new))
    app.tool(_guarded(shards_task_append))
    # Idempotent.
    app.tool(_guarded(shards_note_update), annotations=_IDEMPOTENT)
    app.tool(_guarded(shards_task_claim), annotations=_IDEMPOTENT)
    app.tool(_guarded(shards_task_release), annotations=_IDEMPOTENT)
    app.tool(_guarded(shards_task_finish), annotations=_IDEMPOTENT)
    app.tool(_guarded(shards_task_update), annotations=_IDEMPOTENT)
    # Destructive.
    app.tool(_guarded(shards_task_cancel), annotations=_DESTRUCTIVE)


_register()


def main() -> None:  # pragma: no cover - process entry point
    """Serve the shards MCP tools (stdio transport). Wired as ``python -m shards.mcp.server``."""
    app.run()


if __name__ == "__main__":  # pragma: no cover - process entry point
    main()
