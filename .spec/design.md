---
type: entrypoint
scope: design
design_format: google-labs-code/design.md-compatible
children: []
updated: 2026-06-10
---

# Brain — Design

Cross-cutting design language for Brain. Brain has **no visual interface** — it is a CLI plus an MCP surface — so this document carries no visual tokens. It defines the interaction conventions, output contracts, and tone that every command and tool must preserve.

**Product:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

---

## Overview

Brain should feel like an instant, predictable broker. Two audiences share one surface: a human at a terminal who wants terse, readable output, and an agent over MCP that wants strict, structured JSON. The personality is *quiet and deterministic* — no spinners, no chatty prose, no surprises. The same three verbs behave identically whether driven by a person or a model, and identical state is visible through both. When making design decisions, preserve: the three-verb surface, JSON-by-default for machine paths, stable exit codes, and graceful degradation messaging.

## Interaction Conventions

Global flags exist on every command so behaviour is uniform across the surface:

| Flag | Effect |
|---|---|
| `--json` | Machine-readable output. Always available. |
| `--quiet` | Emit only IDs / paths. |
| `--tags <t,t>` | Filter by tags, AND semantics. |
| `--any-tag` | Switch `--tags` to OR semantics. |
| `--owner <name>` | Filter / set owner. |

Conventions agents and humans can rely on:

- **JSON is the lingua franca.** `search` and `task ready` are JSON-friendly by default; every command accepts `--json`. MCP tools are always JSON.
- **`path` is always returned** by search results, so a caller can open the file directly or pass it to `note get --full`.
- **Identity is implicit.** `$BRAIN_AGENT` supplies the default `--owner` and powers `--mine`; the user rarely types an owner.
- **Tags are lowercase**, AND by default, OR via `--any-tag`; `--tags` alone (no query) is a deterministic, zero-cost pull.
- **Degradation is announced, not hidden.** When the daemon is down, `search` returns lexical-only results and prints a one-line stderr notice (suppressed under `--quiet`); writes behave identically.

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
