---
type: feature-plan
feature: shards-rebrand
sibling: tech.md
parent: ../../plan.md
updated: 2026-07-05
---

# Feature: Shards Rebrand — Implementation Plan

Rename `brain` → `shards` across the whole tree and reframe the product story as a
multi-agent collaboration mesh, in five units ordered so every reversible edit is validated
before the irreversible repo/dir rename. Lands on `feat/phase-1-mvp`.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** Independent of the deferred `tasks-graph` feature; operates on the
delivered Phase 1–2 surface.

---

## Problem Frame

The brand is spread across 59 files, config keys, env vars, on-disk paths, and the MCP tool
surface. A single guarded sweep plus a hand-authored spec/README reframe changes all of it
without touching behaviour; the existing 578-test suite proves the change stayed nominal. The
only ordering constraint is that the outward-facing, hard-to-reverse steps (GitHub repo
rename, local directory move) run last, after the in-repo work is green and committed.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Single brand token](product.md#requirement-single-brand-token) | shards-rebrand/2, shards-rebrand/3 |
| R2 | [Narrative reframe](product.md#requirement-narrative-reframe) | shards-rebrand/1, shards-rebrand/3 |
| R3 | [Behaviour parity](product.md#requirement-behaviour-parity) | shards-rebrand/4 |
| R4 | [Repo identity](product.md#requirement-repo-identity) | shards-rebrand/5 |

---

## Key Technical Decisions

1. **`.spec/` is hand-authored, never `sed`'d.** Repo law forbids hand-editing the spec outside the `spec` skill; the narrative reframe is genuine authoring anyway. → tech.md § Excluded from the sweep.
2. **Guarded scripted sweep for code/tests/docs.** Deterministic, one reviewable diff, test suite catches over-matches. → tech.md § Execution order.
3. **Irreversible steps last.** Repo rename + dir move after a green commit; the dir move breaks the shell cwd, so it is the final action. → tech.md § Execution order.
4. **No `~/.brain/` migration shim.** Pre-release; restart the daemon after. → tech.md § Accepted side effects.

---

## Unit IDs

Units are `shards-rebrand/n`, assigned once. Cite in commits (`chore(rebrand): shards-rebrand/1 ...`).

---

### shards-rebrand/1 — Root spec rename + mesh reframe

**Goal:** Rename `brain`→`shards` and lead the narrative with the multi-agent collaboration mesh across the five root specs, via the `spec` skill.

**Requirements:** R2

**Dependencies:** —

**Files:**

```
.spec/product.md    # brand + one-liner + Idea reframed to collaboration mesh (keep GBrain link)
.spec/tech.md       # brand + src/shards layout + brain_* → shards_* + BRAIN_ env/paths
.spec/design.md     # brand
.spec/plan.md       # brand + this feature row → DONE at wrap-up
.spec/lessons.md    # brand in command/env references (BRAIN_CONFIG_PATH, brain status, ...)
```

**Test scenarios:**

- Root specs open with the collaboration-mesh framing; verbs/phases/non-goals unchanged in substance.
- `GBrain`/`gbrain` link preserved.

**Verification:** `bash .agents/skills/spec/scripts/validate.sh` passes; manual read confirms reframe + preserved link.

---

### shards-rebrand/2 — Code sweep

**Goal:** Rename the package and every `brain` token in `src/` + `tests/` + `pyproject.toml`.

**Requirements:** R1

**Dependencies:** —

**Files:**

```
src/brain/ → src/shards/          # git mv
src/**, tests/**                  # guarded sed: imports, brain_*, BRAIN_ env, paths, socket/pid, internal ids
pyproject.toml                    # name, [project.scripts], wheel packages, coverage source
```

**Test scenarios:**

- `import shards` resolves; no `brain` import remains.
- MCP tools registered as `shards_*`; env `SHARDS_AGENT`/`SHARDS_CONFIG_PATH`; paths `~/.shards/`, `shards.sock`, `shards.pid`.

**Verification:** `uv run pytest -q` green after `uv sync`; `git grep -in brain -- src tests pyproject.toml` empty.

---

### shards-rebrand/3 — Docs + lockfile

**Goal:** Rebrand `README.md`, `AGENTS.md`, `hooks/session_start.json`; regenerate the lockfile.

**Requirements:** R1, R2

**Dependencies:** shards-rebrand/2

**Files:**

```
README.md                 # brand + collaboration-mesh framing
AGENTS.md                 # brand + narrative (CLAUDE.md is a symlink — follows automatically)
hooks/session_start.json  # brand
uv.lock                   # regenerated via `uv lock`
```

**Test scenarios:**

- `CLAUDE.md` resolves to the rebranded `AGENTS.md` (symlink intact).
- Lockfile references `shards`, not `brain`.

**Verification:** `git grep -in brain -- README.md AGENTS.md hooks uv.lock` empty; `readlink CLAUDE.md` → `AGENTS.md`.

---

### shards-rebrand/4 — Verification gate

**Goal:** Prove the rename stayed purely nominal and no residual brand token survives.

**Requirements:** R3

**Dependencies:** shards-rebrand/1, shards-rebrand/2, shards-rebrand/3

**Files:** — (no edits; a gate)

**Test scenarios:**

- Full suite green; lint/type clean.
- `git grep -in brain` residual is exactly the GBrain link + `brainstorming` under `.agents/skills/spec/`.

**Verification:** `uv run pytest -q` + `uv run ruff check .` + `uv run mypy src/` all pass; `git grep -in brain` matches the expected residual set only. Commit on `feat/phase-1-mvp`.

---

### shards-rebrand/5 — Repo rename + dir move

**Goal:** Rename the GitHub repo and local directory to `shards`.

**Requirements:** R4

**Dependencies:** shards-rebrand/4

**Files:** — (repo/remote/dir operations)

**Test scenarios:**

- `git remote -v` → `LennardZuendorf/shards`.
- Working tree at `Development/shards`.

**Verification:** `gh repo rename shards` succeeds; `git remote set-url` updated; `mv Development/brain Development/shards` as the final action (breaks cwd — done last or handed to the user).

---

## Dependencies

| Unit | Blocks | Blocked by |
|---|---|---|
| shards-rebrand/1 | shards-rebrand/4 | — |
| shards-rebrand/2 | shards-rebrand/3, shards-rebrand/4 | — |
| shards-rebrand/3 | shards-rebrand/4 | shards-rebrand/2 |
| shards-rebrand/4 | shards-rebrand/5 | 1, 2, 3 |
| shards-rebrand/5 | — | shards-rebrand/4 |

---

## Progress

| Unit | Status |
|---|---|
| shards-rebrand/1 | NOT STARTED |
| shards-rebrand/2 | NOT STARTED |
| shards-rebrand/3 | NOT STARTED |
| shards-rebrand/4 | NOT STARTED |
| shards-rebrand/5 | NOT STARTED |
