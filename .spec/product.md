---
type: entrypoint
scope: product
children: []
updated: 2026-06-10
---

# Brain — Product

Brain is a thin coordination layer over a single Tolaria Markdown folder. It gives a human operator and a fleet of agents one shared substrate for capturing knowledge and coordinating work, exposed through exactly three verbs: `note`, `task`, and `search`.

**One-liner:** Three verbs, one daemon, one folder, all agents — notes + search are the memory, tasks are the handoff.

---

## Story

In a travel-tech setup, multiple actors work the same problem space — a human operator, a flights-agent reasoning over NDC/GDS airline config, and a tolaria-agent maintaining the knowledge folder. Today they share no common substrate: every agent session starts cold, yesterday's decisions are lost, work-in-progress is invisible to the next agent, and "handoff" happens through the human copy-pasting context between sessions.

Brain fixes this by being a single broker over one Markdown folder that everyone reads and writes the same way. Knowledge written once is searchable by every agent in every later session. Work is represented as task files any agent can list, claim, and finish, so coordination is durable and inspectable rather than trapped in a chat transcript. The radical simplification is the point: a note *is* memory, `search` is how you recall it, and tasks *are* the handoff. Fewer concepts means fewer ways to do the wrong thing, and a surface small enough that an agent can use it correctly with almost no instruction.

---

## Requirements

At a project level, Brain must:

1. **Stay at three verbs.** `note`, `task`, and `search` fully cover capture, coordination, and recall over one Tolaria folder. Net-new top-level primitives require a spec change and explicit sign-off.
2. **Keep Markdown the source of truth.** Every note and task is a human-readable `.md` file with schema-valid frontmatter; Brain owns the interface, not the data.
3. **Make handoff work through tasks alone.** `owner`, `claimed_by`, `claim`/`finish`/`cancel`, and `blocks`/`blocked_by` are the entire coordination model — no separate handoff concept.
4. **Recall is hybrid search.** Embeddings merged with BM25/substring, re-ranked, across notes and tasks at once, returning JSON with a `path` to the underlying file.
5. **Identity is `$BRAIN_AGENT`.** A per-session environment variable drives `--owner` defaults and `--mine` filters with zero configuration ceremony.
6. **Degrade gracefully.** Every command reads and writes files even when the daemon is down; the daemon is an accelerator, never a gatekeeper.
7. **Stay inside the folder.** All file access is sandboxed to `tolaria_path`; agent content is treated as inert data, never instructions or shell input.

---

## Design Principles

1. **Markdown is the human-readable source of truth.** State lives in `.md` files a person can open, diff, and edit by hand. Brain never hides data behind an opaque store.
2. **One folder, one indexer, one search.** Notes and tasks share a folder and an index; `search` spans both. No second pipeline, no second store.
3. **Notes + search = memory.** Remembering is writing a note; recalling is searching. There is no third thing.
4. **Tasks are the coordination and handoff mechanism.** Ownership, claiming, finishing, and dependencies are how agents pass work.
5. **Thin broker: own the interface, not the data.** Brain defines verbs and schema; Tolaria owns the files and Git. Brain stays replaceable.
6. **The daemon is an accelerator, not a requirement.** Availability never depends on a running process.

---

## Target User

A single human operator running a travel-tech workflow alongside a small fleet of cooperating agents:

- **The human operator** captures decisions, sees what agents are doing, and hands them work without babysitting. Interacts through terminal `brain` commands and occasional hand-edits to `.md` files.
- **flights-agent** — a domain agent that recalls prior decisions, picks up and finishes work, and records findings. Interacts through MCP tools backed by the same daemon.
- **tolaria-agent** — a knowledge/PM-style agent that keeps notes structured, files tasks for others, and tracks dependencies.

Not "everyone" — a concrete operator-plus-agents team sharing one Tolaria vault.

---

## Features

| Feature | Covers |
|---|---|
| **[features/notes/](features/notes/product.md)** | The `note` verb — capture and recall of knowledge as Markdown notes |
| **[features/tasks/](features/tasks/product.md)** | The `task` verb — coordination and handoff via claimable, dependency-aware task files |
| **[features/daemon/](features/daemon/product.md)** | The warm accelerator — file watcher, indexer, socket server, graceful fallback |
| **[features/search/](features/search/product.md)** | The `search` verb — hybrid retrieval across notes and tasks, plus deterministic tag pulls |
| **[features/mcp/](features/mcp/product.md)** | The agent surface — MCP tools over the same daemon, plus the SessionStart context hook |

Feature-level UX and requirements live in `features/<name>/product.md` — not here.

## Implementation Phases

| Phase | Goal | Exit Criteria |
|---|---|---|
| **1: MVP** | `note`, `task`, `search`, and the daemon over the Tolaria folder | All three verbs usable from the CLI; coordination complete (claim/finish/blocks); hybrid search with graceful degradation |
| **2: Agent surface** | MCP tools + SessionStart context injection | Every safe CLI command exposed as an MCP tool returning JSON; warm-start hook injects top relevant snippets |
| **3: Richer retrieval** | Pluggable embedders and improved ranking | `embedder=openai\|local` options land **only if** recall proves to be the bottleneck in Phase 1/2 metrics |

## Non-Goals

- **No "memory" primitive.** Notes + search *are* memory.
- **No separate handoff primitive.** Tasks + claim/finish + blocks/blocked_by *are* handoff.
- **No Todoist or external task backend.** Tasks are Markdown files in the Tolaria folder.
- **No vector DB to operate.** The embedder backend (indexed.sh) owns embeddings and their storage.
- **No web dashboard, no RBAC.** The terminal, the files, and `$BRAIN_AGENT` are the whole access model.
- **No sequential IDs.** IDs are short content hashes.
- **No git sync.** The Tolaria folder already is a Git repo; sync is Tolaria's job.
- **Not a full agent platform, and not a replacement for Tolaria.** Brain owns the interface; Tolaria owns the folder and its history.

## Open Questions

1. **indexed.sh interface (blocking).** Exact CLI flags, query syntax, and output format are unconfirmed. *Default:* wrap it behind the `indexed` embedder adapter; assume it produces document embeddings the daemon caches; fall back to BM25-only if unavailable.
2. **MCP destructive-op surface.** Should any destructive op (`note delete`, `task delete`/`cancel`) be agent-callable? *Recommended default:* withhold all destructive ops from MCP; humans run them from a shell.
3. **`[tasks].collections` semantics.** Allow-list of valid agent identities vs. folder split vs. display grouping. *Default:* the known set of agent identities for validation and `--mine`/grouping; not a folder split.
