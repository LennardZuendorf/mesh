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

CLI + MCP, no UI. **Tone:** quiet, deterministic, instant. Humans get terse text; agents get strict JSON. Same behaviour with or without daemon.

**Product:** [product.md](product.md) · **Tech:** [tech.md](tech.md)

---

## Global flags

| Flag | Effect |
|---|---|
| `--json` / `--quiet` | Machine output / IDs only |
| `--owner` / `--mine` | Set or filter by agent (`--mine` = owner or claimed_by) |
| `--tags` / `--any-tag` | Tag filter (AND / OR) |
| `--meta-only` / `--full` | Token budget vs full body |
| `--since` | Recency (`7d`, ISO) |
| `--status`, `--type`, `--limit`, `--threshold` | Task/search filters |

**Defaults:** `search` and `task list` JSON-friendly; `note get` = frontmatter + 200 chars; search always returns `path`.

**Degradation:** substring-scan + one stderr line (hidden with `--quiet`).

**Exit codes:** see [tech.md](tech.md).

---

## Rules

- Terse, parseable output; IDs and paths over prose.
- No fourth verb; no per-command bespoke formats.
- Infrastructure on stderr, never in JSON payloads.
- No spinners, colour, or editor prompts on machine paths.
