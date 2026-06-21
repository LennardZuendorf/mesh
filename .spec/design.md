---
type: entrypoint
scope: design
design_format: google-labs-code/design.md-compatible
children:
  - features/notes/plan.md
  - features/tasks/plan.md
  - features/daemon/plan.md
  - features/search/plan.md
  - features/memory/plan.md
updated: 2026-06-21
---

# Brain — Design

Cross-cutting design language for Brain. Brain has **no visual interface** — it is a CLI plus an MCP surface — so this document carries no visual tokens. It defines the interaction conventions, output contracts, and tone that every command and tool must preserve.

**Product:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

---

## Overview

Brain should feel like an instant, predictable broker. Two audiences share one surface: a human at a terminal who wants terse, readable output, and an agent over MCP that wants strict, structured JSON. The personality is *quiet and deterministic* — no spinners, no chatty prose, no surprises. The same three verbs behave identically whether driven by a person or a model, and identical state is visible through both. When making design decisions, preserve: the three-verb surface, JSON-by-default for machine paths, stable exit codes, and graceful degradation messaging.

**Three-verb carve-out:** Phase 2 adds `recent-activity` and `build-context` as signed-off read-only memory-lens commands (not a fourth verb). Admin commands (`daemon`, `status`, `reindex`) are human-only.

## Interaction Conventions

Global flags exist on every command so behaviour is uniform across the surface:

| Flag | Effect |
|---|---|
| `--json` | Machine-readable output. Always available. |
| `--quiet` | Emit only IDs / paths. |
| `--tags <t,t>` | Filter by tags. AND semantics by default; on `note list` and `task list` also enables deterministic tag-pull when used with `search` (see search feature). |
| `--any-tag` | Switch `--tags` to OR semantics. |
| `--owner <name>` | Filter / set owner. Defaults to `$BRAIN_AGENT`. |
| `--mine` | Filter to items where `owner == $BRAIN_AGENT` or `claimed_by == $BRAIN_AGENT` (tasks; `recent-activity` task leg). |
| `--meta-only` | Drop bodies/snippets — frontmatter (+ title) only. |
| `--full` | Return whole bodies. |
| `--since <ISO\|duration>` | Recency filter (`24h`, `7d`, or ISO timestamp). |
| `--status <s>` | Task status filter (`open`/`claimed`/`done`/`cancelled`). |
| `--limit <n>` | Cap result count (default `20` for list commands, `10` for `search`). |
| `--threshold <f>` | Relevance floor for `search` (`0.0–1.0`, default `0.65`). |
| `--type <t>` | Note/task type filter where applicable. |

Conventions agents and humans can rely on:

- **JSON is the lingua franca.** `search` and `task list` are JSON-friendly by default; every command accepts `--json`. MCP tools are always JSON.
- **`path` is always returned** by search results, so a caller can open the file directly or pass it to `note get --full`.
- **Identity is implicit.** `$BRAIN_AGENT` supplies the default `--owner` and powers `--mine`; the user rarely types an owner.
- **Tags are lowercase**, AND by default, OR via `--any-tag`; `search --tags` alone (no query) is a deterministic, zero-cost pull.
- **Degradation is announced, not hidden.** When hybrid search is unavailable, `search` returns substring-scan results and prints a one-line stderr notice (suppressed under `--quiet`); writes behave identically.

## Output & Status Contract

- **Exit codes are stable and meaningful** — see [tech.md](tech.md) State / Data Contracts: `0` success, `1` generic, `2` usage/validation, `3` not found, `4` already claimed, `5` blocked. Agents branch on these.
- **Token-budget friendliness is a design value.** `--meta-only` drops bodies/snippets for context injection; `--full` returns whole bodies. Default `note get` shows frontmatter + first 200 chars.
- **One artifact, one shape.** A note and a task render through the same frontmatter-first shape; a task is just a note with `type: task` and extra fields.

## Do's and Don'ts

- Do keep output terse and parseable; prefer IDs and paths over prose.
- Do make every command answer the same way with or without the daemon.
- Don't add a fourth verb or a bespoke output format for one command.
- Don't print progress chatter, colour, or interactive prompts on machine paths.
- Don't surface infrastructure errors as data — keep notices on stderr.
