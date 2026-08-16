"""Instructions block builder — the MCP ``instructions`` payload (agent-usability/1).

A pure function of :class:`~shards.schemas.config.Config` (or its absence): no I/O,
no process, no socket, so it is testable by construction alone. ``server.py`` calls
this once at import time — after a guarded config load — and passes the result as
``FastMCP("shards", instructions=...)``. Every MCP client (Cowork included, which
never reads ``~/.claude/skills/``) receives it on connect, before any tool call —
the one artifact a static skill file cannot replicate, because only the running
server knows *this* operator's resolved identity, roster, and vault path.

Section order follows ``.spec/features/agent-usability/tech.md`` § Composition:
what shards is, identity, valid owners, vault, recall mode, the tag-mutation trap,
the coordination protocol, then how to read a result.

**Phrasing.** Ownership language throughout is a *cooperation* convention, never an
authorization claim. ``owner`` is checked against ``[tasks].collections`` when that
roster is non-empty (``core/notes.py::_validate_owner``) — a value/spelling check,
not proof of who is calling; ``claimed_by`` is never checked against it either way.
Neither field verifies the identity of the agent actually making the call (root
``AGENTS.md`` § 6). If a change here reads like "you are not permitted" or "access
denied", that is the wrong register — say what the values mean and what a
cooperative agent does with them instead.

**Budget.** <= 2 KiB (2048 bytes, UTF-8) — this is prepended to every session's
context, not a manual; anything longer belongs in the (separate, agent-usability/8)
skill. ``tests/memory/test_instructions.py`` asserts the byte ceiling directly.
"""

from __future__ import annotations

from shards.core.notes import TAG_SPEC_SEMANTICS
from shards.schemas.config import Config

BUDGET_BYTES = 2048
_MAX_ROSTER_SHOWN = 8

_WHAT_SHARDS_IS = (
    "# shards\n"
    "Three verbs over one shared Markdown vault (Tolaria): note, task, search. "
    "Markdown is the source of truth; shards owns writes and fast reads — no "
    "separate memory store, no external task tracker."
)

_TAG_TRAP = (
    "## Tag mutation\n"
    "`tags` is a list on note_new/task_new. On note_update/task_update it is a "
    "comma string: " + TAG_SPEC_SEMANTICS
)

_COORDINATION = (
    "## Coordination protocol\n"
    "1. Check claimed_by before task_claim; pick another task if someone "
    "already holds it.\n"
    "2. Claim before starting work; release/finish/cancel when you stop.\n"
    "3. owner must match the roster when [tasks].collections is set (a value "
    "check, not an identity check) — claimed_by is never checked against it "
    "either way; neither proves who is actually calling.\n"
    "4. A `warnings` entry on creation flags a duplicate title — check the "
    "named id first.\n"
    "5. Prefer *_append over rewriting a body you did not write; use graph("
    'direction="in") or session_start to see who mentioned you.'
)

_READING_RESULTS = (
    "## Reading results\n"
    "Every note/task has `owner`; tasks add `claimed_by` (null = open), "
    "`status`, `path`. Search hits add a score; creation responses add "
    "`warnings` (duplicate-title only). Withheld: delete, daemon controls, "
    "reindex, status, init, and task_release's --force."
)


def _identity_section(config: Config | None) -> str:
    if config is None:
        return (
            "## Your identity\n"
            "No config could be loaded — run `shards init`, then restart this MCP "
            "session. No identity, roster, or vault path are known until then; pass "
            "an explicit claimer/owner to calls that need one."
        )
    if not config.agent:
        return (
            "## Your identity\n"
            "No agent identity configured ([core].agent / $SHARDS_AGENT unset) — run "
            "`shards init`, or pass an explicit claimer/owner to calls that need one "
            "(task_claim, task_release, note/task creation)."
        )
    return (
        "## Your identity\n"
        f"You are `{config.agent}` this session (from [core].agent / "
        "$SHARDS_AGENT) — tools with a claimer/owner param default to it when "
        "omitted."
    )


def _roster_section(config: Config | None) -> str:
    if config is None:
        return "## Valid owners\nNot known — see identity above."
    roster = config.tasks.collections
    if not roster:
        return (
            "## Valid owners\n"
            "No roster configured ([tasks].collections is empty) — any owner string "
            "is accepted, so a typo'd identity will not be caught."
        )
    shown = ", ".join(roster[:_MAX_ROSTER_SHOWN])
    if len(roster) > _MAX_ROSTER_SHOWN:
        shown += f" (+{len(roster) - _MAX_ROSTER_SHOWN} more)"
    return f"## Valid owners ([tasks].collections)\n{shown}"


def _vault_section(config: Config | None) -> str:
    if config is None:
        return "## Vault\nNot known — see identity above."
    return f"## Vault\n{config.core.tolaria_path}"


def _recall_section(config: Config | None) -> str:
    if config is None:
        return "## Recall\nUnknown — assume substring-only until config loads."
    if not config.search.hybrid:
        return (
            "## Recall\n"
            "Substring fallback only ([search].hybrid = false) — search never "
            "calls indexed."
        )
    collection = config.search.collection or "(unset)"
    return (
        "## Recall\n"
        f"Hybrid configured (collection {collection}), but search degrades "
        "silently to a substring scan when the daemon or indexed is "
        "unreachable — no field says which path answered; treat hits as "
        "possibly substring-only."
    )


def build_instructions(config: Config | None) -> str:
    """Render the MCP ``instructions`` block for ``config``.

    ``config`` is ``None`` when the server could not load one at startup (missing
    or malformed ``config.toml``) — every dynamic section then renders a named
    "run ``shards init``" degradation instead of guessing. Pure: no I/O, and the
    same config always renders the same text, so two different configs are
    expected to render different identity/roster/vault text (see
    ``tests/memory/test_instructions.py``).
    """
    sections = [
        _WHAT_SHARDS_IS,
        _identity_section(config),
        _roster_section(config),
        _vault_section(config),
        _recall_section(config),
        _TAG_TRAP,
        _COORDINATION,
        _READING_RESULTS,
    ]
    return "\n\n".join(sections)
