"""agent-usability/6 — CLI flag contract and help truthfulness.

Brief: ``.superpowers/sdd/mesh-3track/agent-usability-6-brief.md``. Locks the
R6/R7 contract stated in ``.spec/features/agent-usability/tech.md`` § Surface C:

* **R6, ``--json``/``--quiet``** — accepted **before and after** the command
  name, with identical effect, on every non-admin command (the actual command
  tree walked below, not just the reported ``task list`` repro). Admin
  commands (``daemon``, ``status``, ``reindex``) are out of scope — a separate,
  later unit owns their reader.
* **R6, ``--owner``** — one meaning: *the identity this invocation acts as*.
  **Decision (settled here):** honoured wherever an owner is *written on
  creation* (``note new``/``task new``) or *filtered* (``note list``/
  ``task list``/``search``), and already honoured on the identity-resolution
  paths (``task claim``/``task release``/``session-start``, unchanged).
  Deliberately **not** coalesced into ``task update``'s reassignment
  ``--owner`` — an update only changes what's explicitly asked, and folding an
  ambient global flag into an unrelated ``--priority``/``--tags`` update would
  silently reassign accountability nobody asked to change (the same
  silent-mutation risk R3 rejected for bare-list tags). A local ``--owner``
  always wins over the global one, on every command that has a local one.
* **R7, help truthfulness** — ``--type``/``--status`` help text is generated
  from ``get_args(NoteType)``/``get_args(TaskStatus)``, never a hand-typed
  list, so help cannot omit a value the schema actually accepts.

Test tiers:

* **Tier A** (acceptance) — every non-admin command in the actual tree
  (``cli/__main__.py``'s ``_SUBAPPS``/``_LEAVES``) accepts ``--json``/
  ``--quiet`` both before and after the command name, exiting 0 both times.
  Covers every JSON-emitting command, not just the reported repro.
* **Tier B** (byte-identical output) — for commands whose result does not
  depend on invocation count (pure reads, and the genuinely idempotent
  no-op branches of ``claim``/``release``/``finish``/``cancel``), the
  ``--json``-before and ``--json``-after stdout are compared byte-for-byte.
  Mutating creates/updates/deletes are Tier-A-only: two separate invocations
  necessarily produce different ids/timestamps, so byte-identity would be a
  false promise there — the *acceptance* symmetry is what the contract
  actually claims for those.
"""

from __future__ import annotations

import json
import uuid
from collections.abc import Callable
from pathlib import Path
from typing import get_args

import pytest
import typer
from typer.core import TyperGroup, TyperOption
from typer.testing import CliRunner

from mesh.cli.__main__ import app
from mesh.cli.note import _NOTE_TYPES, note_app
from mesh.cli.task import _TASK_STATUSES, task_app
from mesh.core.notes import create_note
from mesh.core.tasks import create_task
from mesh.schemas.config import Config, load_config
from mesh.schemas.note import NoteType
from mesh.schemas.task import TaskStatus


@pytest.fixture
def cfg(mesh_config: Path) -> Config:
    return load_config()


def _invoke(args: list[str]):  # type: ignore[no-untyped-def]
    return CliRunner().invoke(app, args)


def _unique(prefix: str) -> str:
    return f"{prefix} {uuid.uuid4().hex[:8]}"


def _help_text(sub_app, command: str, option: str) -> str:  # type: ignore[no-untyped-def]
    """The exact help string typer/click stores for ``<sub_app> <command> <option>``."""
    click_group = typer.main.get_command(sub_app)
    assert isinstance(click_group, TyperGroup)
    click_command = click_group.commands[command]
    for param in click_command.params:
        if option in param.opts:
            assert isinstance(param, TyperOption)
            assert param.help is not None
            return param.help
    raise AssertionError(f"no {option} option on {sub_app} {command}")


# --------------------------------------------------------------------------- #
# Tier A — --json / --quiet accepted before AND after, every non-admin        #
# command in the actual tree (not just the reported task-list repro)          #
# --------------------------------------------------------------------------- #


def _command_table(cfg: Config) -> list[tuple[str, Callable[[], list[str]]]]:
    """``(name, argv_factory)`` for every non-admin command that emits output.

    Each factory is safe to call twice (once per flag position) — mutating
    commands mint a fresh, uniquely-titled entity per call so the two
    invocations never collide; read-only commands return fixed, reusable argv.
    """
    seed_note = create_note(cfg, "Flag Contract Seed Note", body="seed body").id
    seed_task = create_task(cfg, "Flag Contract Seed Task").id
    project_note = create_note(
        cfg, "Flag Contract Project", note_type="project", body="project body"
    ).id

    return [
        ("note new", lambda: ["note", "new", _unique("Note New"), "--body", "x"]),
        (
            "note append",
            lambda: ["note", "append", create_note(cfg, _unique("Append"), body="x").id, "more"],
        ),
        (
            "note update",
            lambda: [
                "note",
                "update",
                create_note(cfg, _unique("Update"), body="x").id,
                "--tags",
                "x",
            ],
        ),
        ("note get", lambda: ["note", "get", seed_note]),
        ("note list", lambda: ["note", "list"]),
        (
            "note delete",
            lambda: [
                "note",
                "delete",
                create_note(cfg, _unique("Delete"), body="x").id,
                "--force",
            ],
        ),
        ("task new", lambda: ["task", "new", _unique("Task New")]),
        (
            "task update",
            lambda: [
                "task",
                "update",
                create_task(cfg, _unique("Update")).id,
                "--priority",
                "high",
            ],
        ),
        (
            "task append",
            lambda: ["task", "append", create_task(cfg, _unique("Append")).id, "more"],
        ),
        ("task claim", lambda: ["task", "claim", create_task(cfg, _unique("Claim")).id]),
        ("task release", lambda: ["task", "release", create_task(cfg, _unique("Release")).id]),
        ("task finish", lambda: ["task", "finish", create_task(cfg, _unique("Finish")).id]),
        ("task cancel", lambda: ["task", "cancel", create_task(cfg, _unique("Cancel")).id]),
        ("task get", lambda: ["task", "get", seed_task]),
        ("task list", lambda: ["task", "list"]),
        (
            "task delete",
            lambda: [
                "task",
                "delete",
                create_task(cfg, _unique("Delete")).id,
                "--force",
            ],
        ),
        ("search", lambda: ["search", "--tags", "flag-contract-no-such-tag"]),
        ("recent-activity", lambda: ["recent-activity"]),
        ("build-context", lambda: ["build-context", seed_note]),
        ("graph", lambda: ["graph", seed_note]),
        ("project", lambda: ["project", project_note]),
        ("session-start", lambda: ["session-start"]),
    ]


def test_json_and_quiet_accepted_before_and_after_every_command(cfg: Config, vault: Path) -> None:
    """The R6 test scenario: every non-admin command, both flags, both positions."""
    table = _command_table(cfg)
    assert len(table) == 22, "command table drifted from the actual tree — update it, not this"

    failures: list[str] = []
    for name, make_argv in table:
        for flag in ("--json", "--quiet"):
            before = _invoke([flag, *make_argv()])
            if before.exit_code != 0:
                failures.append(
                    f"{name}: `{flag} <cmd>` exited {before.exit_code}: {before.output}"
                )
            after = _invoke([*make_argv(), flag])
            if after.exit_code != 0:
                failures.append(f"{name}: `<cmd> {flag}` exited {after.exit_code}: {after.output}")

    assert not failures, "\n".join(failures)


def test_command_table_covers_the_actual_tree() -> None:
    """The table above is a manual mirror of ``cli/__main__``'s wiring — assert it
    hasn't silently gone stale against a newly-added command."""
    from mesh.cli.__main__ import _LEAVES, _SUBAPPS

    admin_leaves = {"init", "status", "reindex"}
    admin_subapps = {"daemon"}
    non_admin_leaves = set(_LEAVES) - admin_leaves
    non_admin_subapps = set(_SUBAPPS) - admin_subapps

    assert non_admin_leaves == {
        "recent-activity",
        "build-context",
        "graph",
        "project",
        "session-start",
    }
    assert non_admin_subapps == {"note", "task", "search"}

    note_commands = {c.name for c in note_app.registered_commands}
    task_commands = {c.name for c in task_app.registered_commands}
    assert note_commands == {"new", "append", "update", "get", "list", "delete"}
    assert task_commands == {
        "new",
        "update",
        "append",
        "claim",
        "release",
        "finish",
        "cancel",
        "get",
        "list",
        "delete",
    }


# --------------------------------------------------------------------------- #
# Tier B — byte-identical stdout: reads, and the idempotent no-op branches    #
# --------------------------------------------------------------------------- #


def test_note_get_json_identical_before_and_after(cfg: Config, vault: Path) -> None:
    note_id = create_note(cfg, "Byte Identical Note", body="x").id
    before = _invoke(["--json", "note", "get", note_id])
    after = _invoke(["note", "get", note_id, "--json"])
    assert before.exit_code == after.exit_code == 0
    assert before.stdout == after.stdout


def test_note_list_json_identical_before_and_after(cfg: Config, vault: Path) -> None:
    create_note(cfg, "Byte Identical List Note", body="x")
    before = _invoke(["--json", "note", "list"])
    after = _invoke(["note", "list", "--json"])
    assert before.stdout == after.stdout


def test_task_get_json_identical_before_and_after(cfg: Config, vault: Path) -> None:
    task_id = create_task(cfg, "Byte Identical Task").id
    before = _invoke(["--json", "task", "get", task_id])
    after = _invoke(["task", "get", task_id, "--json"])
    assert before.stdout == after.stdout


def test_task_list_json_identical_before_and_after(cfg: Config, vault: Path) -> None:
    create_task(cfg, "Byte Identical List Task")
    before = _invoke(["--json", "task", "list"])
    after = _invoke(["task", "list", "--json"])
    assert before.stdout == after.stdout


def test_search_json_identical_before_and_after(cfg: Config, vault: Path) -> None:
    create_note(cfg, "Byte Identical Search Note", tags=["byte-identical-tag"], body="x")
    before = _invoke(["--json", "search", "--tags", "byte-identical-tag"])
    after = _invoke(["search", "--tags", "byte-identical-tag", "--json"])
    assert before.stdout == after.stdout
    assert json.loads(before.stdout)  # not an empty/degenerate fixture


def test_recent_activity_json_identical_before_and_after(cfg: Config, vault: Path) -> None:
    create_note(cfg, "Byte Identical Activity Note", body="x")
    before = _invoke(["--json", "recent-activity"])
    after = _invoke(["recent-activity", "--json"])
    assert before.stdout == after.stdout


def test_build_context_json_identical_before_and_after(cfg: Config, vault: Path) -> None:
    note_id = create_note(cfg, "Byte Identical Context Note", body="x").id
    before = _invoke(["--json", "build-context", note_id])
    after = _invoke(["build-context", note_id, "--json"])
    assert before.stdout == after.stdout


def test_graph_json_identical_before_and_after(cfg: Config, vault: Path) -> None:
    note_id = create_note(cfg, "Byte Identical Graph Note", body="x").id
    before = _invoke(["--json", "graph", note_id])
    after = _invoke(["graph", note_id, "--json"])
    assert before.stdout == after.stdout


def test_project_json_identical_before_and_after(cfg: Config, vault: Path) -> None:
    project_id = create_note(cfg, "Byte Identical Project", note_type="project", body="x").id
    before = _invoke(["--json", "project", project_id])
    after = _invoke(["project", project_id, "--json"])
    assert before.stdout == after.stdout


def test_session_start_json_identical_before_and_after(cfg: Config, vault: Path) -> None:
    create_task(cfg, "Byte Identical Session Task")
    before = _invoke(["--json", "session-start"])
    after = _invoke(["session-start", "--json"])
    assert before.stdout == after.stdout


def test_task_claim_idempotent_noop_identical_before_and_after(cfg: Config, vault: Path) -> None:
    task_id = create_task(cfg, "Byte Identical Claim Task").id
    _invoke(["task", "claim", task_id])  # establish the claim once (not measured)
    before = _invoke(["--json", "task", "claim", task_id])
    after = _invoke(["task", "claim", task_id, "--json"])
    assert before.exit_code == after.exit_code == 0
    assert before.stdout == after.stdout


def test_task_release_idempotent_noop_identical_before_and_after(cfg: Config, vault: Path) -> None:
    task_id = create_task(cfg, "Byte Identical Release Task").id  # never claimed
    before = _invoke(["--json", "task", "release", task_id])
    after = _invoke(["task", "release", task_id, "--json"])
    assert before.exit_code == after.exit_code == 0
    assert before.stdout == after.stdout


def test_task_finish_idempotent_noop_identical_before_and_after(cfg: Config, vault: Path) -> None:
    task_id = create_task(cfg, "Byte Identical Finish Task").id
    _invoke(["task", "finish", task_id])  # establish the terminal state once
    before = _invoke(["--json", "task", "finish", task_id])
    after = _invoke(["task", "finish", task_id, "--json"])
    assert before.exit_code == after.exit_code == 0
    assert before.stdout == after.stdout


def test_task_cancel_idempotent_noop_identical_before_and_after(cfg: Config, vault: Path) -> None:
    task_id = create_task(cfg, "Byte Identical Cancel Task").id
    _invoke(["task", "cancel", task_id])  # establish the terminal state once
    before = _invoke(["--json", "task", "cancel", task_id])
    after = _invoke(["task", "cancel", task_id, "--json"])
    assert before.exit_code == after.exit_code == 0
    assert before.stdout == after.stdout


# --------------------------------------------------------------------------- #
# R6 — --owner: the identity this invocation acts as                          #
# --------------------------------------------------------------------------- #


def test_global_owner_honoured_on_note_new(cfg: Config, vault: Path) -> None:
    """The defect this unit fixes: ``mesh --owner X note new`` used to silently
    write the configured agent's owner instead of ``X``."""
    result = _invoke(
        ["--owner", "other-agent", "note", "new", "Owned By Other", "--body", "x", "--json"]
    )
    assert result.exit_code == 0, result.output
    new_id = json.loads(result.stdout)["id"]
    obj = json.loads(_invoke(["note", "get", new_id, "--json"]).stdout)
    assert obj["owner"] == "other-agent"
    assert obj["owner"] != cfg.agent


def test_global_owner_honoured_on_task_new(cfg: Config, vault: Path) -> None:
    result = _invoke(["--owner", "other-agent", "task", "new", "Owned By Other Task", "--json"])
    assert result.exit_code == 0, result.output
    new_id = json.loads(result.stdout)["id"]
    obj = json.loads(_invoke(["task", "get", new_id, "--json"]).stdout)
    assert obj["owner"] == "other-agent"
    assert obj["owner"] != cfg.agent


def test_local_owner_wins_over_global_owner_on_new(cfg: Config, vault: Path) -> None:
    """The local flag always wins — coalescing never overrides an explicit local value."""
    result = _invoke(
        [
            "--owner",
            "other-agent",
            "note",
            "new",
            "Local Wins",
            "--owner",
            "test-agent",
            "--body",
            "x",
            "--json",
        ]
    )
    assert result.exit_code == 0, result.output
    new_id = json.loads(result.stdout)["id"]
    obj = json.loads(_invoke(["note", "get", new_id, "--json"]).stdout)
    assert obj["owner"] == "test-agent"


def test_global_owner_filters_note_list(cfg: Config, vault: Path) -> None:
    create_note(cfg, "Mine Note", owner="test-agent", body="x")
    create_note(cfg, "Theirs Note", owner="other-agent", body="x")
    result = _invoke(["--owner", "other-agent", "note", "list", "--json"])
    assert result.exit_code == 0, result.output
    owners = {row["owner"] for row in json.loads(result.stdout)}
    assert owners == {"other-agent"}


def test_global_owner_filters_task_list(cfg: Config, vault: Path) -> None:
    create_task(cfg, "Mine Task", owner="test-agent")
    create_task(cfg, "Theirs Task", owner="other-agent")
    result = _invoke(["--owner", "other-agent", "task", "list", "--json"])
    assert result.exit_code == 0, result.output
    owners = {row["owner"] for row in json.loads(result.stdout)}
    assert owners == {"other-agent"}


def test_global_owner_filters_search(cfg: Config, vault: Path) -> None:
    create_note(cfg, "Mine Search Note", owner="test-agent", tags=["owner-filter-tag"], body="x")
    create_note(cfg, "Theirs Search Note", owner="other-agent", tags=["owner-filter-tag"], body="x")
    result = _invoke(["--owner", "other-agent", "search", "--tags", "owner-filter-tag"])
    assert result.exit_code == 0, result.output
    hits = json.loads(result.stdout)
    assert hits and all(hit.get("owner") == "other-agent" for hit in hits)


def test_global_owner_not_coalesced_into_task_update_reassignment(cfg: Config, vault: Path) -> None:
    """Deliberate narrowing: an unrelated ``task update --priority`` under a
    global ``--owner`` must not silently reassign accountability."""
    task = create_task(cfg, "Update Owner Guard")
    assert task.owner == cfg.agent
    result = _invoke(["--owner", "other-agent", "task", "update", task.id, "--priority", "high"])
    assert result.exit_code == 0, result.output
    obj = json.loads(_invoke(["task", "get", task.id, "--json"]).stdout)
    assert obj["owner"] == cfg.agent  # unchanged — reassignment stayed opt-in
    assert obj["priority"] == "high"  # the actual request still took effect


def test_explicit_local_owner_still_reassigns_task_update(cfg: Config, vault: Path) -> None:
    """The narrowing above is about the *global* flag only — an explicit local
    ``--owner`` on ``task update`` still reassigns, exactly as documented."""
    task = create_task(cfg, "Update Owner Explicit")
    result = _invoke(["task", "update", task.id, "--owner", "other-agent"])
    assert result.exit_code == 0, result.output
    obj = json.loads(_invoke(["task", "get", task.id, "--json"]).stdout)
    assert obj["owner"] == "other-agent"


def test_global_owner_still_resolves_claimer_identity(cfg: Config, vault: Path) -> None:
    """Unchanged behaviour: ``task claim``/``task release`` have no local
    ``--owner`` of their own — the global flag is their only identity input."""
    task = create_task(cfg, "Claim Identity Task")
    result = _invoke(["--owner", "other-agent", "task", "claim", task.id])
    assert result.exit_code == 0, result.output
    obj = json.loads(_invoke(["task", "get", task.id, "--json"]).stdout)
    assert obj["claimed_by"] == "other-agent"


# --------------------------------------------------------------------------- #
# R7 — help text is generated from the schema literal, not restated           #
# --------------------------------------------------------------------------- #


def test_note_new_type_help_lists_every_note_type() -> None:
    help_text = _help_text(note_app, "new", "--type")
    for value in get_args(NoteType):
        assert value in help_text, f"{value!r} missing from `note new --type` help: {help_text!r}"


def test_note_update_type_help_lists_every_note_type() -> None:
    help_text = _help_text(note_app, "update", "--type")
    for value in get_args(NoteType):
        assert value in help_text, (
            f"{value!r} missing from `note update --type` help: {help_text!r}"
        )


def test_note_type_help_matches_the_cli_module_constant() -> None:
    """Locks the derivation itself: the CLI's ``_NOTE_TYPES`` *is*
    ``get_args(NoteType)`` — not a separately-maintained copy that happens to
    agree today. If the schema grows a type and this constant is not re-derived
    from it, this assertion is what catches it."""
    assert get_args(NoteType) == _NOTE_TYPES
    assert "project" in _NOTE_TYPES  # the exact value the original defect omitted


def test_task_list_status_help_lists_every_task_status() -> None:
    help_text = _help_text(task_app, "list", "--status")
    for value in get_args(TaskStatus):
        assert value in help_text, (
            f"{value!r} missing from `task list --status` help: {help_text!r}"
        )


def test_task_status_help_matches_the_cli_module_constant() -> None:
    assert get_args(TaskStatus) == _TASK_STATUSES


def test_creating_a_project_note_by_following_help_succeeds(cfg: Config, vault: Path) -> None:
    """The concrete regression: ``project`` is a real, help-advertised value —
    following the help text (not tribal knowledge) creates a project note."""
    result = _invoke(
        ["--json", "note", "new", "Project Via Help", "--type", "project", "--body", "x"]
    )
    assert result.exit_code == 0, result.output
    assert json.loads(result.stdout)["type"] == "project"


# --------------------------------------------------------------------------- #
# Existing exit codes / output shapes unchanged for already-working paths     #
# --------------------------------------------------------------------------- #


def test_global_side_flags_still_work_unchanged(cfg: Config, vault: Path) -> None:
    """The pre-existing global-only invocations (root callback side) are
    untouched by adding the local leaf flags."""
    note_id = create_note(cfg, "Global Side Still Works", body="x").id
    result = _invoke(["--json", "note", "get", note_id])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["id"] == note_id

    quiet_result = _invoke(["--quiet", "note", "get", note_id])
    assert quiet_result.exit_code == 0
    assert quiet_result.stdout.strip() == note_id


def test_invalid_note_type_still_exits_2(cfg: Config, vault: Path) -> None:
    """Adding local --json/--quiet must not change validation behaviour."""
    result = _invoke(["note", "new", "Bad Type", "--type", "bogus", "--body", "x"])
    assert result.exit_code == 2, result.output
