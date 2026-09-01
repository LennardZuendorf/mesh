"""Deterministic hash IDs for notes and tasks.

An id is a type prefix (``n-`` note, ``t-`` task) followed by a Crockford
base-32 rendering of a SHA-256 hash of ``created_iso + "\\0" + title``. The
digest is treated as a big-endian integer and rendered most-significant digit
first, so the same inputs always yield the same id (deterministic), and IDs are
never sequential and never UUIDs.

Collision extension
-------------------
The default id uses ``MIN_LENGTH`` (4) leading base-32 digits. When a caller
supplies an ``exists`` predicate and the candidate is already taken, the
generator **appends** the next digit from the same digest (5 chars, then 6, …)
until it finds a free id or exhausts the digest. Because each longer id is the
next prefix of the full digest rendering, a 5-char id always begins with its
4-char sibling: extension only ever appends, never rewrites.
"""

from __future__ import annotations

import hashlib
from collections.abc import Callable

# Crockford base-32 alphabet (no I, L, O, U — avoids visual/typo ambiguity).
_CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
_NOTE_PREFIX = "n-"
_TASK_PREFIX = "t-"

MIN_LENGTH = 4


def _crockford(value: int) -> str:
    """Render a non-negative integer in Crockford base-32, MSB first."""
    if value == 0:
        return _CROCKFORD[0]
    digits: list[str] = []
    while value > 0:
        value, rem = divmod(value, 32)
        digits.append(_CROCKFORD[rem])
    return "".join(reversed(digits))


def _digest_b32(created_iso: str, title: str) -> str:
    material = f"{created_iso}\0{title}".encode()
    digest = hashlib.sha256(material).digest()
    return _crockford(int.from_bytes(digest, "big"))


def _generate(
    prefix: str,
    created_iso: str,
    title: str,
    exists: Callable[[str], bool] | None,
    min_length: int,
) -> str:
    full = _digest_b32(created_iso, title)
    max_length = len(full)
    length = min(min_length, max_length)
    while True:
        candidate = prefix + full[:length]
        if exists is None or not exists(candidate):
            return candidate
        if length >= max_length:
            # Digest exhausted; return the longest id we can form.
            return candidate
        length += 1


def generate_note_id(
    created_iso: str,
    title: str,
    *,
    exists: Callable[[str], bool] | None = None,
    min_length: int = MIN_LENGTH,
) -> str:
    """Return an ``n-`` id for ``(created_iso, title)``.

    Deterministic for the same inputs. Pass ``exists`` to extend the id on
    collision (see module docstring).
    """
    return _generate(_NOTE_PREFIX, created_iso, title, exists, min_length)


def generate_task_id(
    created_iso: str,
    title: str,
    *,
    exists: Callable[[str], bool] | None = None,
    min_length: int = MIN_LENGTH,
) -> str:
    """Return a ``t-`` id for ``(created_iso, title)`` (same algorithm as notes)."""
    return _generate(_TASK_PREFIX, created_iso, title, exists, min_length)
