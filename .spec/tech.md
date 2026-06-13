---
type: entrypoint
scope: technical
children: []
updated: 2026-06-10
---

# Brain — Technical Architecture

Brain is a thin coordination layer over one Tolaria Markdown folder. The architecture is daemon-centric but never daemon-dependent: a single local daemon owns the file watcher, the index, and search, while both the `brain` CLI and the MCP server are thin clients that talk to it over a unix domain socket. Write primitives (atomic file ops, locks, path sandboxing) live in the shared `core`/`storage` layer, not the daemon, so every command works with degraded search when the daemon is down. Feature-level implementation detail lives under `.spec/features/<name>/`.

---

## Design Philosophy

1. **Daemon is an accelerator, not a gatekeeper.** The watcher and index make things fast; the CLI degrades to direct file ops + lexical search when the daemon is down. Availability never depends on a running process.
2. **All writes are atomic and idempotent.** Every write is temp-file + `os.replace`. `task claim` is an atomic test-and-set (`O_EXCL`); `task finish` is atomic and idempotent across files — crash recovery is "just run it again."
3. **Markdown stays clean.** Only agreed frontmatter keys; round-trip unknown keys untouched; never inject machinery into bodies.
4. **One folder, one index, one search.** No second pipeline, no storage abstraction beyond "markdown files on disk." Ranked retrieval is delegated to the first-party `indexed` engine; Brain keeps only the thin wrapper, tag-pull, and substring fallback.
5. **Agent content is data.** Titles, bodies, tags, and outcomes are inert — written to frontmatter/body, never `eval`'d or shell-interpolated.

---

## Architecture Overview

```
brain/
├── pyproject.toml            # uv/pipx; entry point: brain = brain.cli:app
├── README.md
├── .spec/                    # Design docs (source of truth)
└── src/brain/
    ├── cli/                  # typer app: note, task, search, daemon, status (thin)   → features/notes, tasks, search, daemon
    ├── mcp/                  # FastMCP memory server over the same daemon (thin)       → features/memory
    ├── daemon/               # asyncio unix-socket server: watcher + warm index        → features/daemon
    ├── core/                 # domain logic: ids, notes, tasks, wikilinks, activity, context → features/notes, tasks, memory
    ├── index/                # indexed client, tag-pull, substring fallback, watch     → features/search, daemon
    ├── schemas/              # pydantic models (note, task, config)                   → cross-cutting (this doc)
    └── storage/              # atomic writes, O_EXCL locks, path sandbox              → cross-cutting (this doc)
```

The daemon holds warm state that is expensive to rebuild per invocation: the parsed frontmatter index and the wikilink/ID resolution graph. Ranking state (lexical + vectors) is owned by `indexed`, not the daemon. A `watchdog` observer watches `tolaria_path` so edits by humans, Tolaria, or other agents are reflected without an explicit reindex, and fires `indexed index update` to keep the search engine current.

---

## Tech Stack

**Inherited / integrated:** Python 3.11+, `uv` (dev tooling), the Tolaria vault + its filesystem-direct MCP (the one Markdown folder, Git-managed by Tolaria), and `indexed` (first-party hybrid-search engine: ingest + embeddings + ranked retrieval, CLI/MCP) that Brain's `search` wraps.

**Added:** `typer` (CLI), `python-frontmatter` (YAML frontmatter I/O), `pydantic` v2 (schemas/validation), `watchdog` (file watching), `FastMCP` (MCP server). Search is delegated to `indexed` (first-party hybrid engine); Brain keeps only a deterministic tag-pull + a substring fallback in-process. Packaging via `uv` (dev) / `pipx` (end-user install).

---

## State / Data Contracts

Cross-cutting contracts that span every feature. Feature folders consume these; they are defined here once.

| Contract | Location | Invariant |
|---|---|---|
| **Note/task frontmatter** | `schemas/note.py`, `schemas/task.py` | A task is a note with `type: task`. Only agreed keys are written; unknown keys round-trip untouched. `created`/`updated` are ISO-8601 UTC. `updated` bumps on any **content or frontmatter write** (create, append, field update, claim, finish, cancel); a pure index reconcile or folder-move that the watcher applies to match frontmatter does **not** bump `updated`. |
| **Hash IDs** | `core/ids.py` | `<prefix>-<hash>`, `hash = crockford_b32(sha256(created_iso + "\0" + title))[:4]`, lowercased — **Crockford base32** (alphabet `0-9a-z` minus `i,l,o,u`), so IDs match `[0-9a-hjkmnp-tv-z]`. `created_iso` is the canonical UTC timestamp to seconds with a `Z` suffix (e.g. `2026-06-13T19:15:15Z`). `n-` for any note type, `t-` for tasks. Never sequential. Collisions extend the hash one char at a time until unique. |
| **Folder routing** | `storage/files.py` | File location is *derived* from `type`/`status`, never authoritative on its own; frontmatter and folder must agree, reconciled on watch events. Note `type → folder`: `note → notes/` (root), `log → notes/logs/`, `decision → notes/decisions/`, `reference → notes/references/`. Tasks route by `status`: `open`/`claimed → tasks/open/`, `done`/`cancelled → tasks/done/`. |
| **Atomic write** | `storage/files.py` | Every write is temp-file + `os.replace`. No partial writes are ever observable. |
| **Atomic entity lock** | `storage/locks.py` | One `O_EXCL` lockfile primitive (`<kind>/.locks/<id>.lock`) serializes read-modify-write per entity. `task claim` uses it to re-read frontmatter, verify `claimed_by == ~`, write, release — two agents cannot both win. `note append`/`note update` use the same primitive so concurrent edits to one note never lose a write. Runs identically in daemon and fallback mode. |
| **Path sandbox** | `storage/sandbox.py` | Every resolved path must remain within `tolaria_path` after `realpath`; `..` traversal, absolute escapes, and symlink escapes are rejected. IDs/slugs map to files through the index, never raw user paths. |
| **Exit codes** | `cli/` | `0` success · `1` generic error · `2` usage/validation · `3` not found · `4` already claimed · `5` blocked. `4` is emitted only by `task claim` (lost race). `5` is emitted only by the opt-in strict gate `task claim --strict` on a task with unfinished `blocked_by`; the default `claim`/`finish` never emit `5`. |
| **Configuration** | `~/.brain/config.toml` + `$BRAIN_AGENT` | `[core]` (`tolaria_path`, `agent_name`), `[search]` (`collection` = the `indexed` collection name for this vault · `hybrid` = bool, default `true`; `false` forces the built-in substring fallback · `threshold` = float `0.0–1.0`, default `0.65`), `[tasks]` (`collections` = the known set of valid agent identities, used to validate `--owner`/`claimed_by` and to group `--mine`; **not** a folder split). `$BRAIN_AGENT` overrides `agent_name` per session and drives `--owner`/`--mine`. Embedder API keys come from env/keyring — never the vault or `config.toml`. |

---

## Build vs Inherit

| Source | Approx. Lines | What |
|---|---|---|
| **Tolaria vault + MCP** (inherited) | n/a | The Markdown folder and its Git history — the data and its versioning — plus Tolaria's filesystem-direct MCP read tools, which Brain coexists with on the same folder |
| **`indexed`** (first-party, integrated) | n/a | The search engine: ingest, embeddings, and hybrid ranked retrieval over the vault. Brain's `search` cluster is a thin wrapper; the brain↔indexed contract is co-designed |
| **Brain** (this project) | small Python surface | The three-verb CLI, the daemon (watcher + warm frontmatter index; drives `indexed index update`), the memory MCP surface, and shared core/storage primitives. Brain **owns all writes** (atomic + `O_EXCL` claim + hash IDs + wikilinks); it **delegates ranking** to `indexed` and **coexists with Tolaria** for vault reads |

---

## Build Sequence

| Order | Component | Feature |
|---|---|---|
| 1 | Note schema, CRUD, wikilinks, `brain note` | notes |
| 2 | Task schema, atomic claim/finish, cancel, list (v1; dependency graph deferred), `brain task` | tasks |
| 3 | Socket server, watcher, warm frontmatter index, fallback shim, admin commands | daemon |
| 4 | `indexed` wrapper + tag-pull + substring fallback + freshness bridge, `brain search` | search |
| 5 | FastMCP memory server (`brain_*` tools + `recent_activity` + `build_context`) + SessionStart hook | memory |

Map build order to features. Unit-level detail lives in feature `plan.md` — not here.

---

## Features

| Feature | Covers |
|---|---|
| **[features/notes/](features/notes/tech.md)** | Note schema, CRUD/append/sections, direct reads, wikilink resolution; `core/notes.py`, `core/wikilinks.py`, `cli/note.py` |
| **[features/tasks/](features/tasks/tech.md)** | Task lifecycle, atomic claim/finish, cancel, list (v1); `core/tasks.py`, `storage/locks.py`, `cli/task.py` |
| **[features/daemon/](features/daemon/tech.md)** | Socket server, watchdog observer, warm frontmatter index, drives `indexed`, daemon-down fallback; `daemon/`, `index/watch.py` |
| **[features/search/](features/search/tech.md)** | `indexed` wrapper + result mapping, tag-pull, substring fallback, freshness bridge; `index/indexed_client.py`, `index/tagpull.py`, `index/fallback.py` |
| **[features/memory/](features/memory/tech.md)** | FastMCP tool mapping + annotations, `recent_activity`/`build_context`, SessionStart hook; `mcp/server.py`, `core/activity.py`, `core/context.py` |

Feature-level files, APIs, and algorithms live in `features/<name>/tech.md` — not here.

## Risks & Mitigations

| Risk | Mitigation |
|---|---|
| `indexed` search-result contract | `indexed` is first-party — the CLI flags/JSON field names are co-defined, not reverse-engineered; Brain wraps it and falls back to its built-in substring scan if `indexed` is absent |
| Windows lacks native unix domain sockets | Fall back to loopback TCP (127.0.0.1, token-gated) or a named pipe when a unix socket is unavailable |
| Concurrent claim race | `O_EXCL` lockfile test-and-set; the same path runs with no daemon, so the guarantee holds in fallback mode |
| Partial multi-file `finish` on crash | Each step is individually atomic and the whole op is idempotent; re-running `finish` is a no-op |
| Untrusted agent content | Treated as inert data; path access sandboxed to `tolaria_path`; socket is `0600`, per-user runtime dir |
