# 🧠 Brain CLI

> A thin, local-first broker that gives AI agents and a human **one shared interface** over
> **Notes, Tasks, Memory, and Search** — plus first-class **agent handoffs**.

**Status:** 📋 Specification / pre-implementation · **Stack:** Python · **Surface:** CLI-first (MCP later)

Brain CLI is the unification layer for a small knowledge mesh around Cowork agents (a Jira Agent,
a PM Agent, a Notes Assistant, …). It doesn't replace the tools you already use — it standardizes
*how* every agent reads, writes, searches, and hands off context, while the underlying systems
remain the source of truth.

```
        Cowork agents  +  human operator
                       │
                  ┌────▼────┐
                  │  brain  │   one interface · stable verbs · shared schemas
                  └────┬────┘
        ┌──────────┬───┴────┬───────────┬──────────────┐
     Notes        Tasks   Memory      Search        Handoffs
   Tolaria/      Todoist  local md   indexed.sh    md artifacts
   Obsidian                store                   (the wedge)
```

## Why

The bottleneck isn't a lack of agents — it's the lack of a **shared substrate**. Agents don't
preserve structure, memory, or write-discipline across sessions, so autonomy feels fragile and
fragmented. Brain CLI gives every agent the **same verbs, schemas, and routing rules** so they
stop reinventing storage and start sharing context.

## What it does

| Domain | Backend | Brain commands |
|---|---|---|
| **Notes** | Tolaria + Obsidian vault (Markdown + frontmatter) | `brain notes search\|open\|upsert\|append` |
| **Tasks** | Todoist | `brain tasks list\|add\|complete\|sync` |
| **Memory** | local files-first Markdown store *(new)* | `brain memory remember\|recall` |
| **Search** | `indexed.sh` (your own indexed search) | `brain search <query>` |
| **Handoffs** | structured Markdown artifacts | `brain handoff create\|list\|claim` |
| Cross-cutting | — | `brain recent`, `brain graph link`, `brain config`, `brain doctor` |

```sh
# Capture, recall, and hand off — across backends, one interface
brain notes upsert "Onboarding Runbook" --frontmatter status=draft --stdin < runbook.md
brain memory remember "API base URL is api.dib.internal" --scope dibtravel --tag infra
brain tasks add "Review PRD section" --due tomorrow --project Brain
brain handoff create --to data-agent --summary "ETL ready for QA" --ref note:Onboarding-Runbook
brain search "vendor migration" --domain notes,memory --json
```

Human-readable by default; `--json` for agent consumption; atomic, idempotent, schema-validated writes.

## Principles

- **Markdown stays the human-readable source of truth.**
- **Brain owns the interface, not the data** — backends are swappable behind stable commands.
- **Thin broker, not a platform** — no required LLM, graph DB, daemon, or cloud.
- **Local-first & privacy-first** — only Todoist (by design) leaves your machine.
- **Retrieval added incrementally** — start simple, add hybrid search only if recall is the bottleneck.

## What makes it different

Surveyed tools (basic-memory, the official MCP memory server, Obsidian MCP servers, mem0, Letta,
Graphiti, …) do memory or notes — **none offer a first-class handoff primitive**, and the closest
competitor is AGPL. Brain's wedge: **agent handoffs + four-domain unification + a permissive
license + a zero-infra, headless, files-first core.**

## Architecture (at a glance)

Hexagonal: thin entrypoints (CLI now, MCP later) → shared core services → **ports** (capability
interfaces) → swappable **adapters** → backends. The CLI and the future MCP server call the same
core, so they never drift.

```
src/brain/
├── cli/        # Typer app (thin)
├── mcp/        # FastMCP server (thin, future)
├── core/       # services — all domain logic + adapter registry
├── ports/      # NotesPort, TasksPort, MemoryPort, SearchPort, HandoffPort
├── adapters/   # tolaria/ todoist/ memory/ search/ handoffs/
├── schemas/    # Pydantic models + frontmatter contracts
└── storage/    # atomic writes, frontmatter I/O
```

## Roadmap

- **Phase 1 (MVP):** thin broker over Tolaria/Obsidian + Todoist; memory store; `indexed.sh`
  search; first-class handoffs; `--json`, `--dry-run`, `doctor`.
- **Phase 2:** `brain-mcp` MCP server (same core) + optional local hybrid search
  (SQLite FTS5 + sqlite-vec + RRF) — only if retrieval becomes the bottleneck.
- **Phase 3:** GBrain (graph/entity reasoning) — only if scale/complexity demand it, as another
  swappable backend.

## Specification

The full product requirements & design live in **[`spec/README.md`](spec/README.md)** — personas
and use cases, the complete command surface, architecture, note schemas, NFRs, security, the
competitive landscape, and open questions.

## Status & open questions

This repo is currently a **specification**. Before implementation we need to resolve a few things
(see [the spec's Open Questions](spec/README.md#15-open-questions--assumptions)), notably: the
exact **Tolaria** and **`indexed.sh`** interfaces, **Todoist** auth, where **memory/handoff**
files live, and final **naming** (`brain` vs the `memory` repo) and **license** (MIT vs Apache-2.0).

## License

To be decided — a **permissive** license (MIT or Apache-2.0) is recommended as a deliberate
differentiator vs. AGPL-licensed alternatives. See the spec's *Security, Privacy & Trust* section.
