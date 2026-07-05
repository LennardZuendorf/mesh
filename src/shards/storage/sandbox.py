"""Path sandboxing.

All file access must stay inside the configured ``tolaria_path``. Both the base
and the candidate are canonicalized with ``os.path.realpath`` (which resolves
symlinks and ``..`` segments); the candidate is rejected unless it is the base
itself or lives beneath it. Canonicalizing *both* sides matters on macOS, where
``$TMPDIR`` sits behind a ``/var -> /private/var`` symlink.
"""

from __future__ import annotations

import os
from pathlib import Path


def safe_resolve(base: Path, candidate: Path) -> Path:
    """Resolve ``candidate`` and confirm it stays within ``base``.

    Relative candidates are resolved against ``base``. Returns the canonical
    absolute path. Raises ``ValueError`` on traversal (``../../``) or on a
    symlink whose real target escapes the sandbox.
    """
    base_real = Path(os.path.realpath(base))

    if not candidate.is_absolute():
        candidate = base / candidate
    resolved = Path(os.path.realpath(candidate))

    if not resolved.is_relative_to(base_real):
        raise ValueError(f"path escapes sandbox {base_real}: {resolved}")
    return resolved
