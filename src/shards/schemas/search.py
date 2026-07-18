"""Search result schema.

A :class:`SearchResult` is the single, uniform hit shape returned by every recall
path — the ``indexed`` hybrid wrapper (search/2), the frontmatter tag-pull, and
the substring fallback. It is deliberately looser than :class:`~shards.schemas.note.Note`:
the corpus includes coexisting non-shards (foreign) Markdown files, so ``id`` /
``type`` / ``title`` are all optional (a foreign file surfaces with ``id: None``).
``path`` is always present — the design contract is that search results are
addressable on disk. It carries no foreign keys, so it needs no ``extra`` stash.
"""

from __future__ import annotations

from datetime import datetime

import msgspec


class SearchResult(msgspec.Struct, kw_only=True):
    """One recall hit: identity (nullable for foreign files), score, and location.

    ``score`` is engine-defined (``1.0`` for an exact tag pull, the substring
    matrix tier for the fallback, or ``indexed``'s rank). ``tags`` / ``owner`` /
    ``updated`` / ``snippet`` are optional enrichments the CLI includes or omits
    per ``--meta-only`` / ``--full``; ``path`` is always populated.
    """

    id: str | None
    type: str | None
    title: str | None
    score: float
    tags: list[str] | None = None
    owner: str | None = None
    updated: datetime | None = None
    snippet: str | None = None
    path: str
