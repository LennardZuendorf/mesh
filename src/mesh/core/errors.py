"""The one base class every CLI/MCP-facing domain exception carries a code on.

Root ``tech.md`` states the contract: "codes live on the domain exception
classes; the CLI boundary maps them once." :class:`MeshError` is that
attribute's single home. Every exception the CLI boundary mapper
(:mod:`mesh.cli._errors`) and the MCP tool boundary (:mod:`mesh.mcp.server`)
need to turn into a process exit code / tool error inherits from it *in place* —
``core.notes.NoteError``, ``core.tasks.TaskError``, ``core.context.SeedNotFoundError``,
``core.lenses.ProjectNotFoundError``, and ``storage.locks.LockError`` — rather than
being relocated here. Nothing moves; only the ancestry changes.

Fixed exit-code convention (root ``tech.md`` § exit codes): ``0`` ok, ``1``
error/infrastructure, ``2`` validation, ``3`` not found, ``4`` claim conflict.
``5`` (blocked) is reserved for the deferred Phase-3 dependency-graph feature and
is never assigned via this base.
"""

from __future__ import annotations


class MeshError(Exception):
    """Base for every domain exception that carries a CLI/MCP exit code.

    Subclasses set ``code`` as a class attribute (2 validation, 3 not found, 4
    claim conflict/contended lock). The default (``1``) is the generic-error
    tier, used only if a subclass never overrides it — none of the concrete
    subclasses in this codebase leave it at the default; each names its own
    tier explicitly.
    """

    code: int = 1
