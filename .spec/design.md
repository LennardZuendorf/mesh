---
type: entrypoint
scope: design
design_format: google-labs-code/design.md-compatible
children:
  - plan.md
updated: 2026-09-05
---

# Mesh — Design

CLI + MCP, no UI. **Tone:** quiet, deterministic, instant. Humans get terse text; agents get
strict JSON. Nothing runs in the background, so behaviour never depends on what else is up.

**Product:** [product.md](product.md) · **Tech:** [tech.md](tech.md)

---

## Global flags

| Flag | Effect |
|---|---|
| `--json` / `--quiet` | Machine output / IDs only |
| `--owner` / `--mine` | Set or filter by agent (`--mine` = owner or claimed_by) |
| `--config PATH` | Config file location; wins over `$MESH_CONFIG_PATH` and the default |
| `--vault PATH` | Vault root; wins over `$MESH_VAULT` and the config file |
| `--space CSV` | Which spaces a search or lens reads. Defaults to the configured search spaces |
| `--engine auto\|indexed\|builtin\|substring` | Which search engine to use; `substring` restores the legacy scoring exactly |
| `--tags` / `--any-tag` | Tag filter (AND / OR). On `update`, `--tags` is a *grammar*: bare `x,y` **merges** (additive, idempotent), `=x,y` replaces the whole list, `+x,-y` is a per-token delta. A mixed spec is rejected (exit 2) rather than guessed at |
| `--meta-only` / `--full` | Token budget vs full body |
| `--since` / `--stale` | Recency floor (`7d`, ISO) and its exact inverse |
| `--status`, `--type`, `--kind`, `--limit`, `--threshold` | Task / note / memory / search filters (`--status` takes a CSV; an unknown value is exit 2, never a silent empty result) |
| `--available` / `--ready` / `--blocked` | Open-and-unclaimed / also unblocked / has an unsatisfied blocker. `--available` is deliberately dependency-blind |
| `--direction in\|out\|both` | `graph` traversal: `out` follows `related`, `in` walks backlinks, `both` either |
| `--team` | `session-start` only: widens the *activity* half to every agent. The task-ownership and mention halves always stay yours |
| `--budget N` | `session-start` only: character budget; trims bodies before entries |
| `--force` | The delete guard on every removal verb |

**Defaults:** `search` and `task list` JSON-friendly; `note get` = frontmatter + 200 characters;
search always returns `path`. Every timestamp renders as UTC with a `Z` suffix on the human and
JSON surfaces alike — one field never has two spellings.

**Flag placement:** `--json`, `--quiet`, `--owner` and `--mine` are accepted both *before* and
*after* the command name on every non-admin command, with identical effect. `--owner` is the
identity the invocation acts as: it defaults the written `owner` on create and filters on list,
and is left alone by verbs that already read it.

---

## Output classes

Every command declares one class, so precedence is never an implementer's guess.

| Class | Commands | Rule |
|---|---|---|
| **M** — mutation | create, append, update, claim, release, finish, cancel, delete, block, unblock, set, add, attach, detach, `config set` | `--quiet` beats `--json`. Quiet prints the id (or name) alone; JSON is `{"id", <fields>, "updated"}` in that order |
| **L** — listing, lens, object, admin | `get`, `list`, the five lenses, `task next`, `asset path`, `asset gc`, `init`, `status`, `reindex`, `watch`, `config` | `--json` beats `--quiet` |
| **S** — search | `search`, `memory recall` | Output is *always* one JSON array; `--json` is accepted and inert; `--quiet` suppresses only the stderr degradation notice |

---

## Errors and degradation

- Human mode: one plain-text line on stderr, the same message it has always been.
- Machine mode (`--json`): exactly one JSON object on stderr —
  `{kind, message, next_action, <structured fields>}`, plus `candidates` on a not-found and
  `retry_after_ms` on a lock conflict. Exit codes are identical either way, and no `next_action`
  may read as an authorisation decision. MCP renders the identical object.
- Degradation (substring fallback, a missing search binary, a blocked claim, an unblock report, a
  duplicate title, a dangling rename) is one stderr line, suppressed by `--quiet`, and **never**
  enters a payload.

**Exit codes:** see [tech.md](tech.md).

---

## Which space wins

Guidance for agents, stated in the playbook and the MCP instructions block; mesh enforces nothing
here, and the duplicate-title advisory is same-space only by construction.

- **note** — durable knowledge about the world or the work; the operator reads it.
- **memory** — an agent's belief about the operator or the fleet; another agent recalls it.
- **scratch** — this session's working state; nobody else should ever need it, and no lens or
  default search will ever show it.
- **asset** — bytes that are not Markdown; attach them to the entity they belong to.

---

## Rules

- Terse, parseable output; IDs and paths over prose.
- One verb family per space; no per-command bespoke formats.
- Infrastructure on stderr, never in JSON payloads.
- No spinners, colour, or editor prompts on machine paths.
- Read verbs never write — no touch-on-read, no use counters, no lens that mutates.
- Every MCP tool parameter carries a description and enums show real domain literals; failures
  cross as the same structured envelope the CLI emits, never a stack trace. An agent should be
  able to act without reading this document.
