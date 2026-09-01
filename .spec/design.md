---
type: entrypoint
scope: design
design_format: google-labs-code/design.md-compatible
children:
  - plan.md
updated: 2026-09-01
---

# Mesh — Design

CLI + MCP, no UI. **Tone:** quiet, deterministic, instant. Humans get terse text; agents get strict JSON. Same behaviour with or without daemon.

**Product:** [product.md](product.md) · **Tech:** [tech.md](tech.md)

---

## Global flags

| Flag | Effect |
|---|---|
| `--json` / `--quiet` | Machine output / IDs only |
| `--owner` / `--mine` | Set or filter by agent (`--mine` = owner or claimed_by) |
| `--tags` / `--any-tag` | Tag filter (AND / OR). On `update`, `--tags` is a *grammar*: bare `x,y` **merges** (additive, idempotent), `=x,y` replaces the whole list, `+x,-y` is a per-token delta. A mixed spec is rejected (exit 2) rather than guessed at |
| `--meta-only` / `--full` | Token budget vs full body |
| `--since` | Recency (`7d`, ISO) |
| `--status`, `--type`, `--limit`, `--threshold` | Task/search filters (`--status` takes a CSV; an unknown value is exit 2, never a silent empty result) |
| `--stale` | Recency *ceiling* — the exact inverse of `--since`, for finding abandoned work |
| `--available` | Open and unclaimed; defaults `--sort` to `priority` |
| `--direction in\|out\|both` | `graph` traversal: `out` follows `related`, `in` walks backlinks, `both` either |
| `--team` | `session-start` only: widens the *activity* half to every agent. The task-ownership and mention halves always stay yours |

**Defaults:** `search` and `task list` JSON-friendly; `note get` = frontmatter + 200 chars; search always returns `path`. Every timestamp renders as UTC with a `Z` suffix, on the human and JSON surfaces alike — one field never has two spellings.

**Flag placement:** `--json`, `--quiet`, `--owner` and `--mine` are accepted both *before* and *after* the command name on every non-admin command, with identical effect. `--owner` is the identity the invocation acts as: it defaults the written `owner` on create and filters on list, and is left alone by verbs that already read it.

**Degradation:** substring-scan + one stderr line (hidden with `--quiet`).

**Exit codes:** see [tech.md](tech.md).

---

## Rules

- Terse, parseable output; IDs and paths over prose.
- No fourth verb; no per-command bespoke formats.
- Infrastructure on stderr, never in JSON payloads.
- No spinners, colour, or editor prompts on machine paths.
- Every MCP tool parameter carries a description and enums show real domain literals; failures
  cross as `{kind, message, next_action}`, never a stack trace. An agent should be able to act
  without reading this document.
