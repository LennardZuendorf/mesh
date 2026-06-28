# AGENTS.md — Brain Engineering Guide

> **`brain`** — a thin coordination layer over a single Tolaria Markdown folder.
> Three verbs (`note`, `task`, `search`), one daemon, one folder, all agents.

---

## 0. Spec-Driven Workflow (read this first)

**This is a spec-driven repository. The spec is the source of truth — code follows the spec, never the other way around.**

The canonical product & technical contract lives in **[`.spec/`](.spec/)** — start at
[`.spec/product.md`](.spec/product.md), [`.spec/tech.md`](.spec/tech.md), and [`.spec/plan.md`](.spec/plan.md).

Before you touch anything:

1. **Confirm the `spec` skill is installed and use it.** Spec work in this repo *must* go through
   the `spec` skill — do not hand-author or hand-edit the spec.
   - Verify it's available: it should appear in your in-session skill list, or under
     `.claude/skills/spec/` (project) or `~/.claude/skills/spec/` (user). Invoke it with `/spec`.
   - **If the `spec` skill is missing, stop and install it before proceeding.** Tell the user it's
     not installed rather than working around it.
2. **Read the `.spec/` root layer** (`product.md`, `tech.md`, `design.md`, `plan.md`, `lessons.md`) before
   writing code or docs; follow links down into `.spec/features/<name>/` as needed.
3. **Any change that contradicts or extends the spec updates the spec first** — via the `spec`
   skill, with user confirmation — *then* the implementation follows.
4. Keep the surface honest: this project's whole thesis is staying at **three verbs**. New
   top-level primitives need a spec change and an explicit sign-off.

---

## 1. Core Operating Principles

Follow the **ASK → PLAN → CONFIRM → EXECUTE** loop:

1. **ASK** — clarify requirements and constraints before assuming.
2. **PLAN** — break the task down and present the approach (and which spec sections it touches).
3. **CONFIRM** — get explicit user approval before implementing.
4. **EXECUTE** — implement incrementally with clear explanations.

Quality over speed. Simplicity (KISS) wins. Strict typing (mypy), `ruff`-clean, meaningful test
coverage, and an instant-feeling CLI (heavy work lives in the warm daemon, not at startup).

---

## 2. Repository Goals & Connections

**Goal.** Give a human operator and a fleet of agents one shared substrate for capturing
knowledge and coordinating work over a single Tolaria Markdown folder — without a database, a
memory subsystem, or an external task tracker.

- **Notes + search = memory.** No separate memory store; recall delegates to `indexed`.
- **Tasks = coordination = handoff.** v1: `owner` / `claimed_by` / `claim` / `finish` / `cancel` / `list`. The dependency graph (`blocks` / `blocked_by` readiness, `release`, strict gate) is a deferred later phase.
- **Markdown is the source of truth.** Brain owns the *interface* (and writes), not the data.

**Connections (what brain talks to):**

| System | Role | Boundary |
|---|---|---|
| **Tolaria vault + MCP** | The one Markdown folder (`notes/`, `tasks/`) — source of truth — plus Tolaria's filesystem-direct MCP read tools | Brain **owns writes** and cheap direct reads; **coexists** with Tolaria on the same folder; Git/sync and the vault are Tolaria's job |
| **`indexed`** | First-party hybrid-search engine (ingest + embeddings + ranked retrieval, CLI/MCP) | Brain's `search` is a thin wrapper; the brain↔indexed contract is co-designed; falls back to a built-in substring scan if absent |
| **Cowork agents** | Consumers (flights-agent, tolaria-agent, …) | Call brain via CLI (`--json`) and the `memory` MCP tools |
| **`$BRAIN_AGENT`** | Per-session agent identity | Drives `--owner` defaults and `--mine` |
| **The daemon** | Watcher + warm frontmatter index; drives `indexed index update`; shared by CLI and MCP | An accelerator, never a hard dependency — CLI degrades gracefully when it's down |

---

## 3. Tech Stack

**Language & runtime:** Python 3.11+, `uv`.

**Core libraries:** `typer` (CLI), `python-frontmatter` (YAML frontmatter), `pydantic` v2
(schemas/validation), `watchdog` (file watching), `FastMCP` (MCP server). Search is delegated to
the first-party `indexed` engine (hybrid lexical + vector); Brain keeps only a deterministic
tag-pull and a substring fallback in-process.

**Dev tools:** `ruff` (lint/format), `mypy` (strict), `pytest` + `pytest-cov`, `pre-commit`.

---

## 4. Development Workflow

### Common commands

```bash
uv sync --all-groups              # Install all dependencies
uv run ruff check . --fix         # Lint with auto-fix
uv run ruff format .              # Format
uv run mypy src/                  # Type-check (strict)
uv run pytest -q                  # Run tests
uv run pytest -q --cov=src        # With coverage
uv run brain --help               # CLI help
uv run brain daemon start         # Start the local daemon
```

> The repo is currently at **spec stage** (no implementation yet). The stack and commands above
> are the agreed target; honour them as code lands.

### Git commit standards

Format: **`type(scope): subject`** — imperative, lowercase, ≤ 50 chars, no trailing period.

Allowed types: `feat`, `fix`, `refactor`, `perf`, `style`, `test`, `docs`, `build`, `ci`, `chore`, `revert`.

```
feat(task): add atomic claim via O_EXCL lockfile
fix(search): fall back to substring scan when indexed is down
docs(spec): wrap indexed for ranked retrieval
```

### Branching & pushing

- Develop on the assigned feature branch; never push to the default branch without permission.
- Run the full test suite **before** any push.
- Do **not** open a pull request unless the user explicitly asks.

---

## 5. Repository Structure

```
brain/
├── AGENTS.md            # this guide  (CLAUDE.md is a symlink to it)
├── CLAUDE.md -> AGENTS.md
├── README.md            # short overview
├── .spec/               # THE SPEC — source of truth (managed via the `spec` skill)
│   ├── product.md       # root: mini PRD            ├── tech.md     # root: architecture
│   ├── design.md        # root: CLI design language  ├── plan.md     # root: feature sequence
│   ├── lessons.md       # root: accumulated lessons
│   └── features/        # notes, tasks, daemon, search, memory (product + tech + plan each)
└── src/brain/
    ├── cli/             # typer app: note, task, search, daemon, status (thin)
    ├── mcp/             # FastMCP memory server over the same daemon (thin)
    ├── daemon/          # asyncio unix-socket server: watcher + warm frontmatter index (drives indexed)
    ├── core/            # domain logic: ids, notes, tasks, wikilinks, activity, context
    ├── index/           # indexed client, tag-pull, substring fallback, watch
    ├── schemas/         # pydantic models (note, task, config)
    └── storage/         # atomic writes, O_EXCL locks, path sandbox
```

---

## 6. Key Design Constraints

- **Daemon is an accelerator, not a gatekeeper.** Every command works (with degraded search) when
  the daemon is down. Write primitives live in `core`/`storage`, not the daemon.
- **All writes are atomic** (temp-file + `os.replace`) and **idempotent**. `task claim` is an
  atomic test-and-set (`O_EXCL`); `task finish` is atomic and idempotent (the multi-file
  unblock-cascade arrives with the deferred dependency-graph phase).
- **Markdown stays clean.** Only agreed frontmatter keys; round-trip unknown keys; never inject machinery into bodies.
- **Path sandboxing.** All file access stays inside `tolaria_path`; reject traversal/symlink escapes.
- **Agent content is data**, never instructions or shell input.
- **Hash IDs** (`n-…`, `t-…`), never sequential.

---

## 7. Claude Code Skills

- **`spec`** — *required* for all spec work (see §0). Invoke with `/spec`.
- **`feature-dev`** (if available) — multi-file features needing architectural guidance.
- **`find-skills`** — discover additional capabilities.

---

## 8. DO / DON'T

**DO ✅**
- Read the spec and route spec changes through the `spec` skill, first.
- Ask → plan → confirm → execute; get approval before coding.
- Keep the surface at three verbs; use comprehensive type hints and Pydantic validation.
- Make writes atomic and idempotent; keep the CLI usable without the daemon.

**DON'T ❌**
- Never hand-edit the spec or skip the `spec` skill.
- Never use `pip`/`pipenv`/`poetry` directly, or activate venvs manually (use `uv run`).
- Never reintroduce a separate memory store, a Todoist/external task backend, or a standalone handoff primitive.
- Never add sequential IDs, a vector DB to operate, or git-sync logic.
- Never push to the default branch or open a PR without explicit permission.

---

**Remember:** spec first, then ASK → PLAN → CONFIRM → EXECUTE. Quality and simplicity over speed.
