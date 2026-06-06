# 🧠 Brain CLI

> A thin coordination layer over a single Tolaria Markdown folder.
> **Three verbs — `note`, `task`, `search`. One daemon. One folder. All agents.**

**Status:** 📋 Specification / pre-implementation · **Stack:** Python · **Surface:** CLI-first (MCP over the same daemon)

Brain CLI gives a human operator and a fleet of agents (a flights-agent, a tolaria-agent, …) one
shared substrate for capturing knowledge and coordinating work. Notes and tasks are both plain
`.md` files in the same folder — a task is just a note with `type: task`. One local daemon watches
the folder, indexes it, and serves both the CLI and an MCP server from a single process.

```
        agents  +  human operator
                │
           ┌────▼────┐
           │  brain  │   note · task · search
           └────┬────┘
           ┌────▼─────┐
           │  daemon  │   watcher + indexer + search
           └────┬─────┘
        ┌───────┴────────┐
   Tolaria folder     indexed.sh
   notes/ tasks/      (embeddings)
```

## The whole idea

- **Notes + search = memory.** Remembering is writing a note; recalling is searching. No separate memory store.
- **Tasks = coordination = handoff.** Agents pass work by creating tasks, claiming them, finishing them, and wiring `blocks`/`blocked_by` — no bespoke handoff protocol.
- **Markdown is the source of truth.** Every artifact is a human-readable `.md` file in Tolaria. Brain owns the *interface*, not the data. Git stays Tolaria's job.

## Three verbs

```bash
# notes — capture & recall
brain note new "Use CLID fallback for Lufthansa GDS" --type decision --tags ndc,lufthansa
brain note append n-a3f2 "Confirmed: only needed for J/C class" --section Follow-ups --timestamp

# tasks — coordinate & hand off
brain task new "Audit NDC fallback logic" --owner flights-agent --priority high
brain task ready --owner flights-agent          # what's unblocked & unclaimed?
brain task claim t-c7d1                          # atomic — only one agent wins
brain task finish t-c7d1 --outcome "All J/C fares resolved via CLID."   # unblocks dependents

# search — hybrid (embeddings + BM25), JSON by default, always returns a path
brain search "how did we handle the CLID fallback decision"
brain search --tags ndc,flights --type note --meta-only   # deterministic tag pull, no embedding
```

Identity comes from `$BRAIN_AGENT` (drives `--owner` defaults and `--mine`). Config lives in
`~/.brain/config.toml`. IDs are short hashes (`n-a3f2`, `t-c7d1`).

## Architecture in one breath

A daemon-centric but never daemon-*dependent* design: the `brain` CLI and the MCP server are thin
clients talking to one local daemon over a unix socket; the daemon owns the file watcher, the
index, and hybrid search. **If the daemon is down, the CLI falls back to direct file ops + lexical
search** — it is never hard-blocked. The only pluggable piece is the embedder (`indexed` |
`openai` | `local`).

## What Brain deliberately does NOT do

No vector DB to manage (indexed.sh handles it) · no web dashboard, no RBAC · **no memory
primitive** (notes + search = memory) · **no separate handoff primitive** (tasks = handoff) · no
Todoist or external task backend · no sequential IDs (hash IDs) · no git sync (Tolaria is already git).

## Roadmap

- **Phase 1 (MVP):** `note` + `task` + `search` over the Tolaria folder; daemon watcher + indexed.sh hybrid search; full claim/finish/blocks coordination; `$BRAIN_AGENT`; graceful daemon-down fallback.
- **Phase 2:** MCP server over the same daemon + a Claude Code `SessionStart` hook for context injection.
- **Phase 3:** pluggable embedders (`openai`/`local`) and richer ranking — only if recall becomes the bottleneck.

## Specification

The full product & technical spec lives in **[`spec/README.md`](spec/README.md)** — personas and
use cases, the complete command surface, daemon architecture, the note/task data model, search
internals, coordination/concurrency guarantees, the MCP surface, NFRs, security, and open questions.

## Status & open questions

This repo is currently a **specification**. The main blocking unknown is the exact **`indexed.sh`**
interface (flags, query syntax, output format). Other decisions to confirm: a `task release`
command, whether to withhold all destructive ops from MCP, `[tasks].collections` semantics, and
Windows transport. See [the spec's Open Questions](spec/README.md#17-open-questions--decisions).

## License

To be decided — a **permissive** license (MIT or Apache-2.0) is recommended.
