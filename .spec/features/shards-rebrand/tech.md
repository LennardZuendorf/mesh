---
type: feature-tech
feature: shards-rebrand
sibling: product.md
parent: ../../tech.md
updated: 2026-08-16
---

# Feature: Shards Rebrand — Architecture

A guarded, scripted token sweep plus a hand-authored narrative reframe. The mechanical part
(`brain`→`shards` in code/tests/docs/config) is a deterministic `git mv` + `sed` pass with an
explicit exclusion list; the root-spec narrative and README are edited by hand because they
carry meaning, not just a token. The test suite is the safety net that proves the sweep was
purely nominal.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Token map (old → new)

| Old | New | Where |
|---|---|---|
| `src/brain/` package dir | `src/shards/` | `git mv` |
| `import brain.` / `from brain` | `shards` | all `src/`, `tests/` |
| CLI cmd `brain`, `brain-mcp` | `shards`, `shards-mcp` | `pyproject.toml [project.scripts]` |
| `name = "brain"` (project), wheel `packages`, `coverage source` | `shards` | `pyproject.toml` |
| MCP `FastMCP("brain")`, CLI `name="brain"` | `"shards"` | `mcp/server.py`, `cli/__main__.py` |
| MCP tools `brain_*` (~20 fns + `app.tool` registrations) | `shards_*` | `mcp/server.py`, docstrings |
| env `BRAIN_AGENT`, `BRAIN_CONFIG_PATH` | `SHARDS_AGENT`, `SHARDS_CONFIG_PATH` | `schemas/config.py`, docstrings |
| `~/.brain/config.toml`, `~/.brain/run/` | `~/.shards/config.toml`, `~/.shards/run/` | `schemas/config.py`, `daemon/`, `cli/admin.py` |
| `brain.sock`, `brain.pid` | `shards.sock`, `shards.pid` | `daemon/client.py`, `cli/admin.py` |
| internal `_BRAIN_ID_PREFIX`, `brain_id()`, `_is_brain_id`, `brain_files` | `_SHARDS_ID_PREFIX`, `shards_id()`, `_is_shards_id`, `shards_files` | `core/wikilinks.py`, `index/tagpull.py`, `index/watch.py`, `core/notes.py` |
| README / AGENTS.md brand + narrative | `shards`, "mesh for multi-agent collaboration" | docs |
| GitHub `LennardZuendorf/brain`, dir `Development/brain` | `…/shards`, `Development/shards` | `gh repo rename`, `git remote set-url`, `mv` |

Case variants swept in one pass: `brain`→`shards`, `Brain`→`Shards`, `BRAIN`→`SHARDS`.

---

## Excluded from the sweep (must NOT change)

- **`.spec/**`** — never `sed`'d. Hand-authored via the `spec` skill (repo law: no hand-editing the spec). Root spec rename + reframe is unit `shards-rebrand/1`.
- **`.agents/skills/spec/**`** — vendored spec skill; its only `brain` hits are the word *brainstorming*.
- **`GBrain` / `github.com/garrytan/gbrain`** in `.spec/product.md` — external project reference.
- **`uv.lock`** — regenerated with `uv lock`, never `sed`'d.

---

## Execution order (irreversible / outward-facing steps last)

1. **Root specs** (`shards-rebrand/1`) — `spec` skill: rename + mesh reframe of `product.md`, `tech.md`, `design.md`, `plan.md`, `lessons.md`.
2. **Code sweep** (`shards-rebrand/2`) — `git mv src/brain src/shards`; guarded `sed` over `src/` + `tests/`; then `pyproject.toml` by hand (scripts, name, wheel packages, coverage source).
3. **Docs + lockfile** (`shards-rebrand/3`) — `README.md`, `AGENTS.md` (only — `CLAUDE.md` is a symlink and follows), `hooks/session_start.json`; then `uv lock`.
4. **Verify gate** (`shards-rebrand/4`) — must be green before commit.
5. **Commit** on `feat/phase-1-mvp`, then **repo rename + dir move** (`shards-rebrand/5`) — `gh repo rename shards`, `git remote set-url`, and the `Development/brain` → `Development/shards` move as the **very last** action (it breaks the shell cwd — done last or handed to the user).

The guarded sweep is the recommended mechanism over hand-editing 59 files or delegating to a
subagent: it is deterministic, produces one reviewable diff, and the test suite catches any
over-match.

---

## Verification gate

- `uv run pytest -q` → green (the coverage suite is the real proof the rename stayed nominal).
- `uv run ruff check .` and `uv run ty check src/` → clean. (As-built correction,
  core-hardening/9: the toolchain is `ty`, not `mypy` — `mypy` is not and has never been a
  project dependency; this gate could not literally pass as originally written.)
- `git grep -in brain` → **expected residual = exactly** the `GBrain`/`gbrain` link in `product.md` and `brainstorming` under `.agents/skills/spec/`. Anything else is a miss or an over-match to fix before commit.
- `shards --help` and `shards-mcp` resolve after `uv sync`.

---

## Accepted side effects

- `~/.brain/` → `~/.shards/` orphans any existing daemon socket, PID, and config on this
  machine. Pre-release (v0.1.0); no migration shim — restart the daemon after the rename.
- `shards_*` tool names and `$SHARDS_AGENT` break any downstream cowork-agent config that
  referenced the old names. Accepted: the user approved a full rebrand.
