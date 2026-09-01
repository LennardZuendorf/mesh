"""The one CLI-boundary exception → exit-code mapper (core-hardening/3).

Domain exceptions carry their own exit code (``mesh.core.errors.MeshError.code``
— ``storage.locks.LockError`` inherits it too), per the fixed convention: ``2``
validation, ``3`` not found, ``4`` claim conflict / contended lock. Every CLI
command wraps its body in :func:`cli_errors`, the single place that turns those
(plus a bare ``ValueError`` and any other ``OSError``) into a ``typer.Exit`` and a
one-line stderr message — no handler hardcodes a numeric exit code, and no
handler re-implements this mapping.

``ValueError`` has no ``code`` of its own (msgspec's ``ValidationError`` is a
``ValueError`` subclass, so this branch covers both): a caller passing a bad
type, a bad ``--sort`` field, an owner outside ``[tasks].collections``, or an
unreadable ``--file`` is a validation failure on the caller's input, mapped to
``2``. Any other ``OSError`` reaching this boundary (``ENOSPC``, a read-only
vault, a permission error) is an infrastructure failure outside the caller's
control — mapped to ``1`` with an ``io error:`` line, never a bare traceback.

``0`` (ok) is never touched here — it is simply the absence of an exception.
"""

from __future__ import annotations

import contextlib
from collections.abc import Iterator

import typer

from mesh.core.errors import MeshError

_VALIDATION_CODE = 2
_IO_ERROR_CODE = 1


@contextlib.contextmanager
def cli_errors() -> Iterator[None]:
    """Wrap a CLI command body; map domain/infrastructure exceptions once.

    Order matters: a domain :class:`MeshError` (which ``LockError`` is also
    one of) is checked first so it keeps its own ``code`` and message rather
    than falling into the broader ``ValueError``/``OSError`` catch-alls below.
    Anything this boundary does not recognise propagates unchanged — it never
    swallows a genuine bug.
    """
    try:
        yield
    except MeshError as exc:
        typer.echo(str(exc), err=True)
        raise typer.Exit(exc.code) from None
    except ValueError as exc:
        typer.echo(str(exc), err=True)
        raise typer.Exit(_VALIDATION_CODE) from None
    except OSError as exc:
        typer.echo(f"io error: {exc}", err=True)
        raise typer.Exit(_IO_ERROR_CODE) from None
