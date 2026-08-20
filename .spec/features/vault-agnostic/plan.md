---
type: feature-plan
feature: vault-agnostic
sibling: tech.md
parent: ../../plan.md
updated: 2026-08-18
---

# Feature: Vault-Agnostic — Implementation Plan

Seven units. `vault-agnostic/1` is the only one that touches behaviour-bearing code — an atomic
config-key rename behind a widened alias chain — and everything after it edits strings, docstrings,
fixtures, and specs. Units 2–5 are mutually independent; 6 and 7 restate the contract last, once
the surfaces they describe are final.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** No upstream feature gate. Lands on the same branch as the squashed
`core-hardening` / `team-awareness` / `agent-usability` rollup and must not regress it.

---

## Problem Frame

The tree has no Tolaria dependency to remove — no package, import, subprocess, or MCP call — so
there is nothing to decouple and no behaviour to port. What exists is a name (`tolaria_path`)
threaded through one msgspec field, 24 read sites, 15 test fixtures and the generated config, plus
a set of shipped strings and spec statements that assert a dependency the code never had. The
sequencing problem is that the rename is atomic: `tests/conftest.py:63` writes the TOML key for
nearly all 1326 tests, so field, aliases, writer, example config and fixtures move together or the
suite is red in between. Everything else is separable and ordered only so the prose is written
against final surfaces.

---

## Global Constraints

- Python 3.11+, `uv` only — never `pip`/`poetry`, never a manually activated venv.
- Every command runs through `uv run`.
- Gates: `uv run ruff check . --fix`, `uv run ruff format .`, `uv run ty check src/ tests/`, `uv run pytest -q`.
- Coverage floor is 97 (CI-enforced); the branch baseline is 98.28% over 1326 tests.
- Commit format: `type(scope): subject` — imperative, lowercase, ≤ 50 chars, no trailing period. Cite the unit ID in the body.
- No behaviour changes in this feature. Delete, claim, finish, search, and daemon semantics are untouched.
- Never edit `.spec/lessons.md` or `.spec/features/shards-rebrand/product.md` — frozen historical records (R6).
- Obsidian may be named only in `README.md` and `config.example.toml` comments (R5).

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [Tool-neutral vault key](product.md#requirement-tool-neutral-vault-key) | vault-agnostic/1 |
| R2 | [No shipped artifact names Tolaria as a dependency](product.md#requirement-no-shipped-artifact-names-tolaria-as-a-dependency) | vault-agnostic/1, /3, /4 |
| R3 | [Delete policy stands on its own rationale](product.md#requirement-delete-policy-stands-on-its-own-rationale) | vault-agnostic/6, /7 |
| R4 | [Coexistence is stated generically](product.md#requirement-coexistence-is-stated-generically) | vault-agnostic/2, /5, /6, /7 |
| R5 | [Obsidian is an example, never a dependency](product.md#requirement-obsidian-is-an-example-never-a-dependency) | vault-agnostic/1, /6 |
| R6 | [Root specs corrected, history preserved](product.md#requirement-root-specs-corrected-history-preserved) | vault-agnostic/7 |

---

## Key Technical Decisions

1. **Canonical `vault_path`, two legacy aliases.** `path` and `tolaria_path` stay accepted on input
   forever. The alias chain *is* the migration — no migration tool, no deprecation warning, no
   config edit required of any existing user. Strict superset of today's behaviour.
2. **The rename is one commit.** Field, alias chain, `init` writer, `init --json` key, error text,
   `--path` help, `config.example.toml`, all 24 read sites, and all 15 test fixtures move together.
   Splitting it produces a red suite at every intermediate commit for no reviewer benefit.
3. **`init --json` renames too**, though no test pins it, so the payload is not the one surface
   where the old name survives.
4. **No Obsidian-specific code.** Vault walks are already scoped to `notes/` and `tasks/`, so
   `.obsidian/` and a root `.trash/` are out of traversal by construction. A hidden-directory
   filter would be dead code.
5. **History is superseded, not rewritten.** Dated lessons and the completed `shards-rebrand` spec
   keep their original text; the corrected statement lives in the current layer.

---

## Unit IDs

Units are `vault-agnostic/n`, assigned once and never renumbered. Cite the ID in commit bodies
(`feat(config): vault-agnostic/1 …`).

---

### vault-agnostic/1 — Rename the vault key behind a widened alias chain

**Goal:** `[core].vault_path` is canonical; `[core].path` and `[core].tolaria_path` still load; every machine-visible surface carrying the key name moves with it.

**Requirements:** R1, R2, R5

**Dependencies:** —

**Files:**

```
src/shards/schemas/config.py                  # field, __post_init__, alias chain, docstrings, error text
src/shards/cli/admin.py                       # render_config_toml kwarg, emitted key, --path help, init JSON key
config.example.toml                           # canonical key + Obsidian-as-example comment
src/shards/core/{notes,tasks,lenses}.py       # read sites
src/shards/index/{watcher,tagpull,warm,reconcile,indexed_client}.py   # read sites
src/shards/daemon/server.py                   # read site
src/shards/mcp/instructions.py                # read site (prose handled in /3)
src/shards/storage/files.py                   # note_folder/task_folder call sites
tests/conftest.py                             # shared fixture TOML key
tests/notes/test_schema.py                    # config tests + new alias regressions
tests/cli/test_init.py                        # literal-key assertion, round-trips, example-config load
tests/{memory,mcp,cli,index,search,tasks,notes,daemon}/**   # remaining 15 TOML-literal + attribute fixtures
```

- [ ] **Step 1: Write the failing legacy-alias regression tests**

Add to `tests/notes/test_schema.py`, beside the existing `[core].path` alias test:

```python
def test_legacy_tolaria_path_key_still_loads(tmp_path: Path) -> None:
    """The pre-rename spelling must keep working — no config edit is required."""
    cfg_file = tmp_path / "config.toml"
    cfg_file.write_text(f'[core]\ntolaria_path = "{tmp_path / "vault"}"\n', encoding="utf-8")

    cfg = load_config(cfg_file)

    assert cfg.core.vault_path == tmp_path / "vault"


def test_canonical_vault_path_wins_over_legacy_aliases(tmp_path: Path) -> None:
    """An explicit canonical key beats both aliases; two spellings is not an error."""
    cfg_file = tmp_path / "config.toml"
    cfg_file.write_text(
        f'[core]\n'
        f'vault_path = "{tmp_path / "canonical"}"\n'
        f'tolaria_path = "{tmp_path / "legacy"}"\n'
        f'path = "{tmp_path / "alias"}"\n',
        encoding="utf-8",
    )

    cfg = load_config(cfg_file)

    assert cfg.core.vault_path == tmp_path / "canonical"


def test_legacy_alias_expands_tilde(tmp_path: Path) -> None:
    """`~` expansion is a property of the field, not of the spelling used to reach it."""
    cfg_file = tmp_path / "config.toml"
    cfg_file.write_text('[core]\ntolaria_path = "~/vault"\n', encoding="utf-8")

    cfg = load_config(cfg_file)

    assert cfg.core.vault_path == Path.home() / "vault"
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/notes/test_schema.py -k "legacy or canonical_vault_path" -v`
Expected: FAIL — `AttributeError: 'CoreConfig' object has no attribute 'vault_path'`

- [ ] **Step 3: Rename the field and widen the alias chain**

In `src/shards/schemas/config.py`:

```python
class CoreConfig(msgspec.Struct, kw_only=True):
    """``[core]`` — vault location and default agent identity."""

    vault_path: Path
    agent: str | None = None

    def __post_init__(self) -> None:
        """Expand a leading ``~`` — ``realpath`` does not, so ``~/vault`` would
        otherwise become a literal ``./~/vault`` under the process CWD."""
        self.vault_path = self.vault_path.expanduser()


#: Legacy ``[core]`` spellings of the vault key, accepted on input forever.
#: ``path`` was the root-spec spelling; ``tolaria_path`` predates the rename to
#: a tool-neutral name. Order is precedence: the first present wins, and an
#: explicit ``vault_path`` beats both.
_VAULT_KEY_ALIASES: Final = ("path", "tolaria_path")
```

Replace the two-spelling alias block in `load_config` (currently `if "path" in core and
"tolaria_path" not in core: …`) with:

```python
    core = data.get("core")
    if isinstance(core, dict):
        core = dict(core)
        # Accept every legacy spelling of the vault key; canonical always wins.
        for alias in _VAULT_KEY_ALIASES:
            if alias in core and "vault_path" not in core:
                core["vault_path"] = core.pop(alias)
        core.pop("path", None)
        core.pop("tolaria_path", None)
        agent_override = os.environ.get(_ENV_AGENT)
        if agent_override:
            core["agent"] = agent_override
        data = {**data, "core": core}
```

The two `pop` calls discard a now-redundant alias so msgspec's strict decode does not reject the
unknown key when a config sets both spellings.

Update `load_config`'s docstring — the sentence "The root tech contract spells the vault key
`[core].path`; the field name is `tolaria_path`, so both are accepted on input." becomes: "The
canonical spelling is `[core].vault_path`; `[core].path` and `[core].tolaria_path` are accepted as
legacy aliases."

Update `_missing_config_message` (same file):

```python
        "required: [core].vault_path (path to your Markdown vault folder); "
        "[core].agent, [search], and [tasks] are optional and default."
```

- [ ] **Step 4: Run the alias tests to verify they pass**

Run: `uv run pytest tests/notes/test_schema.py -k "legacy or canonical_vault_path" -v`
Expected: PASS

- [ ] **Step 5: Move every read site**

Run `uv run ruff check .` and follow the failures, or mechanically:

Run: `grep -rln 'tolaria_path' src/ | xargs sed -i 's/tolaria_path/vault_path/g'`
Then re-read each changed file — `src/shards/storage/files.py` also uses `tolaria_path` as a
**parameter name** on `note_folder` / `task_folder`, which this rename correctly fixes, and
`src/shards/storage/sandbox.py`'s docstring reference resolves too.

Run: `grep -rn 'tolaria' src/` — expected: only the prose in `src/shards/__init__.py`,
`src/shards/mcp/instructions.py`, and the foreign-file docstrings, all handled by /2 and /3.

- [ ] **Step 6: Move the `init` writer, its JSON key, and its help text**

In `src/shards/cli/admin.py`: rename the `render_config_toml` keyword to `vault_path`, emit
`f"vault_path = {_toml_string(...)}"`, change the `--path` help to
`"Vault folder ([core].vault_path). Defaults to ~/.shards/vault."`, and change the `init --json`
payload key from `"tolaria_path"` to `"vault_path"`.

- [ ] **Step 7: Move `config.example.toml`**

```toml
# Path to your Markdown vault folder — any directory shards can write
# `notes/` and `tasks/` into. An Obsidian vault root works as-is; shards only
# ever touches its own two subfolders. `~` is expanded at load.
# `path` and `tolaria_path` are still accepted as legacy spellings.
vault_path = "~/shards-vault"
```

- [ ] **Step 8: Move the test fixtures**

Run: `grep -rln 'tolaria_path' tests/ | xargs sed -i 's/tolaria_path/vault_path/g'`
Then fix `tests/cli/test_init.py:236` by hand — the literal-key assertion becomes
`assert "vault_path" in result.output`.

- [ ] **Step 9: Run the full suite and the gates**

Run: `uv run pytest -q && uv run ruff check . && uv run ruff format --check . && uv run ty check src/ tests/`
Expected: 1329 passed (1326 + 3 new), ruff clean, ty clean.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "refactor(config): rename tolaria_path to vault_path

vault-agnostic/1. [core].vault_path is canonical; [core].path and
[core].tolaria_path stay accepted as legacy aliases, so no existing
config needs editing. Moves the init writer, its --json key, the
missing-config error, --path help, and config.example.toml with it."
```

---

### vault-agnostic/2 — Generalise foreign-file docstrings

**Goal:** Docstrings describe foreign-file tolerance and unknown-key round-tripping as applying to any tool sharing the folder, not to Tolaria.

**Requirements:** R4

**Dependencies:** vault-agnostic/1

**Files:**

```
src/shards/schemas/note.py        # unknown-key round-trip docstring
src/shards/core/notes.py          # _resolve_path id gate, list docstring
src/shards/core/tasks.py          # select_tasks skip docstring
src/shards/core/wikilinks.py      # _title_index / scan docstrings
src/shards/core/context.py        # no-id row skip docstring
src/shards/index/warm.py          # foreign-file `id: None` docstring
src/shards/storage/sandbox.py     # sandbox-root docstring
```

- [ ] **Step 1: Rewrite each docstring reference**

Replace every naming of Tolaria as *the* other writer with a generic one. `src/shards/schemas/note.py`'s
"Tolaria's, a user's, another tool's" becomes "a user's, another tool's". Elsewhere,
"Tolaria/foreign file" becomes "foreign file (any writer sharing the folder)".

- [ ] **Step 2: Verify no Tolaria reference remains in these files**

Run: `grep -rin 'tolaria' src/shards/schemas/note.py src/shards/core/ src/shards/index/warm.py src/shards/storage/sandbox.py`
Expected: no output.

- [ ] **Step 3: Run the gates**

Run: `uv run pytest -q && uv run ruff check . && uv run ty check src/ tests/`
Expected: 1329 passed, clean.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs(core): describe foreign files without naming tolaria

vault-agnostic/2. Foreign-file tolerance and unknown-key round-tripping
already applied to any writer sharing the folder; only the docstrings
singled one out."
```

---

### vault-agnostic/3 — Clear Tolaria from the agent-facing runtime surface

**Goal:** The MCP instructions block an agent reads at connect time, the package docstring, and the distribution description describe a Markdown vault with no notes application named.

**Requirements:** R2

**Dependencies:** vault-agnostic/1

**Files:**

```
src/shards/mcp/instructions.py    # _WHAT_SHARDS_IS
src/shards/__init__.py            # package docstring
pyproject.toml                    # project description
tests/memory/test_instructions.py # helper kwarg (already moved by /1); add a negative assertion
```

- [ ] **Step 1: Write the failing negative assertion**

Add to `tests/memory/test_instructions.py`:

```python
def test_instructions_name_no_notes_application() -> None:
    """The block an agent reads must not imply a particular notes app is required."""
    block = build_instructions(_config(vault_path="/home/agent/vault"))

    assert "tolaria" not in block.lower()
    assert "obsidian" not in block.lower()
```

- [ ] **Step 2: Run it to verify it fails**

Run: `uv run pytest tests/memory/test_instructions.py -k name_no_notes_application -v`
Expected: FAIL — the block still contains "Tolaria".

- [ ] **Step 3: Rewrite the three strings**

`src/shards/mcp/instructions.py` — `_WHAT_SHARDS_IS` "Three verbs over one shared Markdown vault
(Tolaria)" becomes "Three verbs over one shared Markdown vault".

`src/shards/__init__.py` — "a mesh for multi-agent collaboration over one Tolaria Markdown folder."
becomes "a mesh for multi-agent collaboration over one Markdown folder."

`pyproject.toml` — `description = "A mesh for multi-agent collaboration over one Markdown folder — note, task, search."`

- [ ] **Step 4: Run the test to verify it passes**

Run: `uv run pytest tests/memory/test_instructions.py -v`
Expected: PASS

- [ ] **Step 5: Run the gates**

Run: `uv run pytest -q && uv run ruff check . && uv run ty check src/ tests/`
Expected: 1330 passed, clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "docs(mcp): drop tolaria from the agent-facing surface

vault-agnostic/3. The MCP instructions block, package docstring, and
distribution description named a notes application shards has never
depended on. Pinned by a negative assertion on the block."
```

---

### vault-agnostic/4 — Clear Tolaria from the shipped plugin bundle

**Goal:** Nothing a user reads while installing the plugin states a notes application as a prerequisite.

**Requirements:** R2

**Dependencies:** —

**Files:**

```
.claude-plugin/marketplace.json                # 2 descriptions
plugins/shards/.claude-plugin/plugin.json      # description
plugins/shards/skills/shards/SKILL.md          # frontmatter compatibility + body prose
tests/test_plugin_bundle.py                    # add a negative assertion over the bundle
```

- [ ] **Step 1: Write the failing negative assertion**

Add to `tests/test_plugin_bundle.py`:

```python
def test_bundle_states_no_notes_application_prerequisite() -> None:
    """Install-time metadata must not read as 'you need a particular notes app'."""
    bundle = [
        Path(".claude-plugin/marketplace.json"),
        Path("plugins/shards/.claude-plugin/plugin.json"),
        Path("plugins/shards/skills/shards/SKILL.md"),
    ]

    for path in bundle:
        assert "tolaria" not in path.read_text(encoding="utf-8").lower(), path
```

- [ ] **Step 2: Run it to verify it fails**

Run: `uv run pytest tests/test_plugin_bundle.py -k no_notes_application -v`
Expected: FAIL on `.claude-plugin/marketplace.json`

- [ ] **Step 3: Rewrite the metadata**

Both `marketplace.json` descriptions and `plugin.json`'s description: replace "one Tolaria Markdown
vault" with "one shared Markdown vault".

`plugins/shards/skills/shards/SKILL.md` frontmatter `compatibility:` — "Requires the shards MCP
server (shards-mcp) or the shards CLI, connected to a configured Tolaria vault" becomes "…connected
to a configured Markdown vault folder". Body line 34: "one shared Tolaria Markdown vault" becomes
"one shared Markdown vault".

- [ ] **Step 4: Run the test to verify it passes**

Run: `uv run pytest tests/test_plugin_bundle.py -v`
Expected: PASS — including the existing JSON-parse and tool-introspection checks.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs(plugin): drop the tolaria prerequisite from the bundle

vault-agnostic/4. The skill's compatibility line read as an install
requirement for a notes app shards does not depend on."
```

---

### vault-agnostic/5 — Neutralise example identities and foreign-frontmatter fixtures

**Goal:** Test and scenario stand-ins for "another agent" and "another tool's frontmatter" no longer carry the name of the tool being dropped.

**Requirements:** R4

**Dependencies:** vault-agnostic/1

**Files:**

```
tests/tasks/{test_finish,test_append,test_list_cancel}.py   # tolaria-agent attribution fixtures
tests/notes/test_append_update.py                            # tolaria-agent + foreign keys
tests/schemas/test_msgspec_roundtrip.py                      # tolaria_pinned / tolaria_meta keys
tests/{notes,tasks}/test_schema.py                           # foreign keys
tests/{notes/test_new,tasks/test_new_update,tasks/test_project,tasks/test_append}.py   # foreign keys
tests/{daemon,search,index,memory,notes}/**                  # foreign filenames, docstring prose
.spec/features/agent-usability/{product,plan}.md             # roster + scenario identities
.spec/features/team-awareness/{product,plan}.md              # scenario identities and sample output
```

- [ ] **Step 1: Rename the agent identity**

Run: `grep -rl 'tolaria-agent' tests/ .spec/features/agent-usability/ .spec/features/team-awareness/ | xargs sed -i 's/tolaria-agent/notes-agent/g'`

- [ ] **Step 2: Rename the foreign-frontmatter keys**

Run: `grep -rl 'tolaria_' tests/ | xargs sed -i 's/tolaria_/othertool_/g'`
These keys exist to prove unknown keys round-trip untouched; the prefix only needs to be one shards
does not own.

- [ ] **Step 3: Rename foreign filenames and fixture prose by hand**

Foreign `.md` stems (`tolaria-note.md` and kin) and the docstrings calling them "a Tolaria file"
become `othertool-note.md` / "a file written by another tool". Also fix the sample graph row in
`.spec/features/team-awareness/product.md:248` (`n-FEWP  note  Tolaria sync notes`) to a neutral
title.

- [ ] **Step 4: Verify the suite still proves the same things**

Run: `uv run pytest -q`
Expected: 1331 passed — the count must not change; these are renames, not deletions.

- [ ] **Step 5: Verify no fixture reference survives**

Run: `grep -rin 'tolaria' tests/ .spec/features/agent-usability/ .spec/features/team-awareness/`
Expected: only the ownership claims in `agent-usability/product.md` and `team-awareness/product.md` that /7 rewrites.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "test: neutralise example agent and foreign-key fixtures

vault-agnostic/5. Stand-ins for 'another agent' and 'another tool's
frontmatter' were named after the tool being dropped. Renames only —
test count unchanged."
```

---

### vault-agnostic/6 — Restate the contract in README and AGENTS.md

**Goal:** The two documents a contributor reads first state a tool-neutral contract and a delete rationale that stands without a git-backed vault.

**Requirements:** R3, R4, R5

**Dependencies:** vault-agnostic/1, /3, /4

**Files:**

```
README.md      # positioning, coexistence claim, config table (3 key references)
AGENTS.md      # header, goal, Connections table, §6 delete rationale, §6 sandbox line, example agents
```

`CLAUDE.md` is a symlink to `AGENTS.md` — do not edit it separately.

- [ ] **Step 1: Rewrite the AGENTS.md Connections row**

The `Tolaria vault + MCP` row is replaced by:

```markdown
| **The vault folder** | The one Markdown folder (`notes/`, `tasks/`) — source of truth | Shards **owns writes** and cheap direct reads inside `notes/` and `tasks/`; it **coexists** with any other tool on the same folder (Obsidian and its plugins, another MCP server, git). Versioning, sync and backup are the vault owner's job, never shards's |
```

- [ ] **Step 2: Rewrite the §6 delete rationale**

The bullet justifying hard delete by "Tolaria's git-backed vault is the recovery path" becomes:

```markdown
- **Delete is a hard `unlink`, by design.** No soft-delete/trash: the vault belongs to the
  operator, and versioning/backup is the vault owner's concern (git, Obsidian Sync, Time Machine,
  or nothing). Shards is not a backup tool and promises no recovery. A `.trash/` would add a second
  delete lifecycle for shards to keep in sync with whatever history mechanism the operator already
  runs — evaluated and deferred, not built. The trade-off is explicit: an unversioned vault has no
  recovery path for a deleted note, and that is the operator's call.
```

- [ ] **Step 3: Fix the remaining AGENTS.md references**

Header and §2 goal: "a single Tolaria Markdown folder" becomes "a single Markdown folder". §6 path
sandboxing: "inside `tolaria_path`" becomes "inside `vault_path`". §2 consumer roster:
`tolaria-agent` becomes `notes-agent`, matching /5.

- [ ] **Step 4: Fix README.md**

Line 3 positioning drops "Tolaria". Line 9's "shards coexists with the Tolaria vault MCP on one
folder — no database to run." becomes "shards coexists with whatever else writes to the folder —
Obsidian, git, another MCP server — and needs no database to run." The config table's three key
references move to `vault_path`, and the alias note becomes: "`path` and `tolaria_path` are also
accepted as legacy spellings of `vault_path`."

- [ ] **Step 5: Verify**

Run: `grep -rin 'tolaria' README.md AGENTS.md`
Expected: no output.
Run: `uv run pytest -q`
Expected: 1331 passed.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "docs: restate the vault contract without tolaria

vault-agnostic/6. Replaces the Connections row and the hard-delete
rationale, whose 'git-backed vault is the recovery path' premise does
not hold for an arbitrary Markdown folder."
```

---

### vault-agnostic/7 — Correct the root specs and record the supersession

**Goal:** No root spec statement is false without Tolaria; the two live feature specs carrying ownership claims are corrected; dated history is untouched.

**Requirements:** R3, R4, R6

**Dependencies:** vault-agnostic/1 … /6

**Files:**

```
.spec/product.md                              # requirement 2, idea line, feature table, non-goals, Resolved
.spec/tech.md                                 # Stack "Vault" row, Sandbox contract row
.spec/plan.md                                 # add vault-agnostic to the Feature Sequence
.spec/features/agent-usability/product.md     # "Does not own … Tolaria's vault/Git"
.spec/features/team-awareness/product.md      # foreign-file scenario, updated_by rationale
```

Never touched: `.spec/lessons.md` (dated entry citing `[core].tolaria_path`) and
`.spec/features/shards-rebrand/product.md:58` (records that the rebrand kept the coupling — true
when written).

- [ ] **Step 1: Correct root `product.md`**

Requirement 2 becomes: "**Markdown is truth.** Schema-valid frontmatter; shards owns the interface,
the operator owns the vault." The idea line's `tolaria-agent` becomes `notes-agent`. The feature
table's "coexists with Tolaria" becomes "coexists with any other writer on the folder". Non-Goals
drops "or replacing Tolaria" and keeps "git sync" with its own reason: "no git sync (versioning is
the vault owner's job)".

- [ ] **Step 2: Add a Resolved entry recording the supersession**

```markdown
- **Vault coupling (2026-08):** the Tolaria naming was dropped — it was never a code dependency (no
  package, import, subprocess or MCP call), only a name and a set of spec claims. `[core].vault_path`
  is canonical with `path`/`tolaria_path` as permanent aliases; versioning and backup are the vault
  owner's job, which is now what justifies hard delete. Supersedes `features/shards-rebrand/`'s
  "keep the Tolaria coupling unchanged". → [features/vault-agnostic/](features/vault-agnostic/product.md)
```

- [ ] **Step 3: Correct root `tech.md`**

Stack table `| Vault | Tolaria folder + MCP (inherited) |` becomes
`| Vault | Any Markdown folder (operator-owned; Obsidian vault works as-is) |`. The Sandbox
contract row's `realpath must stay in tolaria_path` becomes `realpath must stay in vault_path`.

- [ ] **Step 4: Correct the two live feature specs**

`agent-usability/product.md`'s "**Tolaria's vault/Git.**" becomes "**The vault itself — its
versioning, sync and backup.**". `team-awareness/product.md`'s foreign-file scenario becomes "a
file written by another tool, with no shards id", and the `updated_by` rejection rationale swaps
"the git-backed vault is the audit trail" for "attribution stamps live in the body where a human
reads them; audit history is the vault owner's mechanism".

- [ ] **Step 5: Add the feature to the root plan Feature Sequence**

One row for `vault-agnostic`, linked to `features/vault-agnostic/product.md`.

- [ ] **Step 6: Bump `updated:` on every edited spec and validate**

Run: `bash .agents/skills/spec/scripts/validate.sh`
Expected: pass — frontmatter, links, and feature-folder consistency clean.

- [ ] **Step 7: Final whole-tree check**

Run: `grep -rin 'tolaria' . --exclude-dir=.git --exclude-dir=.venv`
Expected: exactly two files — `.spec/lessons.md` and `.spec/features/shards-rebrand/product.md`, both intentionally frozen.

Run: `uv run pytest -q --cov=src && uv run ruff check . && uv run ruff format --check . && uv run ty check src/ tests/`
Expected: 1331 passed, coverage ≥ 97, ruff clean, ty clean.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "docs(spec): drop the tolaria coupling from root specs

vault-agnostic/7. Corrects the claims that were false without Tolaria —
vault/Git ownership, the inherited-MCP stack row, and the coexistence
wording — and records the supersession of shards-rebrand's
'keep the coupling unchanged'. Dated lessons left byte-intact."
```

---

## Dependencies

| Unit | Blocks | Blocked by |
|---|---|---|
| vault-agnostic/1 | /2, /3, /5, /6, /7 | — |
| vault-agnostic/2 | /7 | /1 |
| vault-agnostic/3 | /6, /7 | /1 |
| vault-agnostic/4 | /6, /7 | — |
| vault-agnostic/5 | /7 | /1 |
| vault-agnostic/6 | /7 | /1, /3, /4 |
| vault-agnostic/7 | — | /1 … /6 |

`vault-agnostic/4` is independent of the rename and may run in parallel with /1.

---

## Progress

| Unit | Status |
|---|---|
| vault-agnostic/1 | NOT STARTED |
| vault-agnostic/2 | NOT STARTED |
| vault-agnostic/3 | NOT STARTED |
| vault-agnostic/4 | NOT STARTED |
| vault-agnostic/5 | NOT STARTED |
| vault-agnostic/6 | NOT STARTED |
| vault-agnostic/7 | NOT STARTED |
