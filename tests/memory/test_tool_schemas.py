"""agent-usability/2 — MCP tool schema self-description.

Acceptance coverage (``.spec/features/agent-usability/plan.md`` unit 2):

* Every parameter of every registered ``shards_*`` tool carries a non-empty
  ``description`` in the *generated* JSON Schema (not just the Python
  annotation) — the shape a calling agent actually sees via ``tools/list``.
* ``note_type``/``status`` enum-typed parameters present the full domain
  literal as a JSON Schema ``enum``, and that enum equals
  ``typing.get_args(NoteType)``/``get_args(TaskStatus)`` exactly — a schema
  drift (a literal added to or removed from ``schemas/`` without the tool
  catching up) fails this test rather than silently going stale.
* ``shards_task_list.status`` stays a plain ``str`` (a comma-separated union,
  not a single value) — pinned so it is never mistakenly narrowed to a
  single-value enum that would reject valid CSV input.
* Tool count and per-tool MCP annotation class (read-only / idempotent /
  write / destructive) are unchanged by this unit: descriptions only, no
  behaviour change to the registered surface.
"""

from __future__ import annotations

import asyncio
from typing import Any, get_args

import pytest

import shards.mcp.server as server
from shards.core.context import _DIRECTIONS
from shards.core.tasks import _PRIORITY_VALUES
from shards.schemas.note import NoteType
from shards.schemas.task import TaskStatus


def _registered() -> dict[str, Any]:
    """Map every registered tool name to its FunctionTool (via ``app.list_tools``)."""
    tools = asyncio.run(server.app.list_tools())
    return {tool.name: tool for tool in tools}


def _all_param_schemas() -> list[tuple[str, str, dict[str, Any]]]:
    """``(tool_name, param_name, schema)`` for every parameter of every registered tool."""
    out: list[tuple[str, str, dict[str, Any]]] = []
    for name, tool in _registered().items():
        props = tool.parameters.get("properties", {})
        for param_name, schema in props.items():
            out.append((name, param_name, schema))
    return out


def _schema_enum(schema: dict[str, Any]) -> list[Any] | None:
    """The ``enum`` list off a property schema, unwrapping a nullable ``anyOf``."""
    if "enum" in schema:
        return schema["enum"]
    for branch in schema.get("anyOf", []):
        if "enum" in branch:
            return branch["enum"]
    return None


# --------------------------------------------------------------------------- #
# Every parameter, every tool: a non-empty description                        #
# --------------------------------------------------------------------------- #


def test_every_parameter_of_every_tool_has_a_nonempty_description() -> None:
    """The load-bearing sweep: reads the *generated* schema, not the source —
    a description present in the ``Field(...)`` annotation but dropped by
    FastMCP/pydantic on the way to JSON Schema would not satisfy this."""
    params = _all_param_schemas()
    assert params, "no tools registered — introspection is broken"
    missing = [
        (tool_name, param_name)
        for tool_name, param_name, schema in params
        if not schema.get("description")
    ]
    assert missing == [], f"parameters missing a description: {missing}"


def test_no_tool_is_exempt() -> None:
    """Every registered tool has at least one parameter — none silently skips
    the sweep above by having an empty ``properties`` object."""
    registered = _registered()
    assert len(registered) >= 19
    for name, tool in registered.items():
        props = tool.parameters.get("properties", {})
        assert props, f"{name} has no parameters at all — nothing to describe"


# --------------------------------------------------------------------------- #
# note_type / status carry the schema-owned enum, not a re-typed literal      #
# --------------------------------------------------------------------------- #


@pytest.mark.parametrize(
    ("tool_name", "param_name"),
    [
        ("shards_note_new", "note_type"),
        ("shards_note_list", "note_type"),
        ("shards_note_update", "new_type"),
    ],
)
def test_note_type_schema_enum_matches_domain(tool_name: str, param_name: str) -> None:
    """The generated schema's enum equals ``get_args(NoteType)`` exactly — a
    literal added to or removed from ``schemas/note.py`` propagates here
    automatically; this test would only fail if the *tool* stopped using the
    schema's own ``NoteType``, i.e. drifted back to a hand-typed ``str``."""
    schema = _registered()[tool_name].parameters["properties"][param_name]
    enum = _schema_enum(schema)
    assert enum is not None, f"{tool_name}.{param_name} carries no enum"
    assert set(enum) == set(get_args(NoteType))


def test_search_status_schema_enum_matches_task_status_domain() -> None:
    """``shards_search.status`` is an *exact*-match single-value filter (see
    ``index/tagpull.py::matches_filters``), so — unlike ``task_list.status``
    below — it can be typed as a single ``TaskStatus`` literal without
    rejecting any input the core layer would have accepted anyway."""
    schema = _registered()["shards_search"].parameters["properties"]["status"]
    enum = _schema_enum(schema)
    assert enum is not None, "shards_search.status carries no enum"
    assert set(enum) == set(get_args(TaskStatus))


def test_task_priority_schema_enum_matches_domain() -> None:
    """``priority`` (review round 2, Finding 1): the *schema* field
    (``schemas/task.py::Task.priority``) stays ``str | None`` deliberately —
    tolerant of legacy free-form values already on disk (see
    ``core/tasks.py``'s module docstring) — but the *write* boundary
    (``create_task``/``update_task`` via ``_validate_priority``) already hard-
    rejects anything outside ``_PRIORITY_VALUES``, exactly the closed
    vocabulary a ``Literal`` enforces. There is no public schema-owned type for
    this (unlike ``NoteType``/``TaskStatus``), so the tool's hand-typed
    ``Literal`` is pinned directly against the private write-boundary tuple
    that already enforces the same values, rather than re-derived
    independently — the same anti-drift shape ``get_args(...)`` gives the
    schema-owned enums above."""
    for tool_name in ("shards_task_new", "shards_task_update"):
        schema = _registered()[tool_name].parameters["properties"]["priority"]
        enum = _schema_enum(schema)
        assert enum is not None, f"{tool_name}.priority carries no enum"
        assert set(enum) == set(_PRIORITY_VALUES)


def test_graph_direction_schema_enum_matches_domain() -> None:
    """``direction`` (review round 2, optional fix taken): a genuinely
    unambiguous 3-value private tuple (``core/context.py::_DIRECTIONS``) used
    only by ``graph_query`` — unlike ``sort``, which differs in arity between
    notes (3 values) and tasks (4 values) and stays untyped for that reason."""
    schema = _registered()["shards_graph"].parameters["properties"]["direction"]
    enum = _schema_enum(schema)
    assert enum is not None, "shards_graph.direction carries no enum"
    assert set(enum) == set(_DIRECTIONS)


def test_task_list_status_stays_a_csv_string_not_a_single_literal() -> None:
    """``shards_task_list.status`` accepts a comma-separated union
    (``"open,claimed"`` — team-awareness/4's ``_parse_status_csv``), so typing
    it as a single-value ``TaskStatus`` literal would reject valid multi-value
    input — a real behaviour change this unit must not make. It stays ``str``
    with a description naming the vocabulary instead; pinned here so a future
    edit does not silently narrow it and break CSV filtering."""
    schema = _registered()["shards_task_list"].parameters["properties"]["status"]
    assert _schema_enum(schema) is None
    assert schema.get("description")


# --------------------------------------------------------------------------- #
# Descriptions only — surface shape unchanged                                 #
# --------------------------------------------------------------------------- #


def test_tool_count_unchanged() -> None:
    """20 tools at this point in the plan (memory/1's 17 plus team-awareness/10's
    ``session_start``/``task_append``/``task_release`` parity additions) — the
    brief's stale "17" reflects the spec as originally written, before those
    later units shipped; this pins the actual current count instead."""
    assert len(_registered()) == 20


@pytest.mark.parametrize(
    "name",
    [
        "shards_note_get",
        "shards_note_list",
        "shards_task_get",
        "shards_task_list",
        "shards_search",
        "shards_recent_activity",
        "shards_build_context",
        "shards_graph",
        "shards_project",
        "shards_session_start",
    ],
)
def test_read_only_annotation_unchanged(name: str) -> None:
    tool = _registered()[name]
    assert tool.annotations is not None
    assert tool.annotations.readOnlyHint is True


@pytest.mark.parametrize(
    "name",
    [
        "shards_note_update",
        "shards_task_claim",
        "shards_task_release",
        "shards_task_finish",
        "shards_task_update",
    ],
)
def test_idempotent_annotation_unchanged(name: str) -> None:
    tool = _registered()[name]
    assert tool.annotations is not None
    assert tool.annotations.idempotentHint is True


@pytest.mark.parametrize(
    "name",
    ["shards_note_new", "shards_note_append", "shards_task_new", "shards_task_append"],
)
def test_write_annotation_unchanged(name: str) -> None:
    assert _registered()[name].annotations is None


def test_destructive_annotation_unchanged() -> None:
    tool = _registered()["shards_task_cancel"]
    assert tool.annotations is not None
    assert tool.annotations.destructiveHint is True
