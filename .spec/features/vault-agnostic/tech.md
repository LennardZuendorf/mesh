---
type: feature-tech
feature: vault-agnostic
sibling: product.md
parent: ../../tech.md
updated: 2026-08-18
---

# Feature: Vault-Agnostic — Architecture

A rename and a re-statement, not a decoupling: tracing the tree found **zero** program-level
coupling to Tolaria to remove. There is no tolaria package, import, module, subprocess, or MCP
client call anywhere; `uv.lock` has no occurrence of the string; `CoreConfig.tolaria_path` is an
ordinary `pathlib.Path` that nothing validates as "a Tolaria vault"; and `shards init` creates the
folder itself (`cli/admin.py`, default `~/.shards/vault`), so the standalone happy path never
involved Tolaria at all. The work is therefore one field rename behind a widened alias chain, plus
edits to the strings and spec statements that assert a dependency the code never had.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

```
src/shards/schemas/config.py        # CoreConfig.tolaria_path → vault_path; widen the alias chain   ~6 LOC
src/shards/cli/admin.py             # render_config_toml kwarg + emitted key + --path help + init JSON key
src/shards/storage/files.py         # note_folder/task_folder parameter names; sandbox docstring
src/shards/mcp/instructions.py      # _WHAT_SHARDS_IS prose; vault line label
src/shards/__init__.py              # package docstring
src/shards/core/{notes,tasks,wikilinks,context,lenses}.py   # 24 read sites + foreign-file docstrings
src/shards/index/{watcher,warm,tagpull,reconcile,indexed_client}.py   # read sites
src/shards/storage/sandbox.py       # docstring naming the sandbox root
src/shards/schemas/note.py          # unknown-key round-trip docstring
pyproject.toml                      # distribution description
config.example.toml                 # canonical key + Obsidian-as-example comment
.claude-plugin/marketplace.json     # 2 descriptions
plugins/shards/.claude-plugin/plugin.json   # description
plugins/shards/skills/shards/SKILL.md       # compatibility line + body prose
README.md                           # positioning, coexistence claim, config table
AGENTS.md                           # Connections table, delete rationale, sandbox line  (CLAUDE.md is a symlink)
.spec/product.md                    # requirement 2, feature table, non-goals
.spec/tech.md                       # stack row, sandbox contract row
tests/**                            # 15 TOML fixtures, attribute sites, 1 literal-key assertion
```

---

## Contract / API

The vault key resolves through a three-spelling alias chain in `load_config`, canonical last:

```python
# src/shards/schemas/config.py — CoreConfig
class CoreConfig(msgspec.Struct, kw_only=True):
    vault_path: Path            # was: tolaria_path
    agent: str | None = None

# src/shards/schemas/config.py — load_config, replacing the current two-spelling alias
_VAULT_ALIASES = ("path", "tolaria_path")   # accepted on input, oldest-to-newest precedence
for alias in _VAULT_ALIASES:
    if alias in core and "vault_path" not in core:
        core["vault_path"] = core.pop(alias)
```

Precedence rule: an explicit `vault_path` always wins; otherwise the first alias present is used.
A config setting two spellings is not an error — the canonical one, then `path`, then
`tolaria_path`, in that order. This is a strict superset of today's behaviour, so every config
that loads before this change still loads after it.

**Machine-visible surfaces that carry the key name** — all move to `vault_path` together, since a
partial rename is what makes a config key ambiguous:

| Surface | Location | Today | After |
|---|---|---|---|
| msgspec field | `schemas/config.py:69` | `tolaria_path` | `vault_path` |
| TOML input | `load_config` | `tolaria_path`, `path` | `vault_path` + both aliases |
| Generated config | `cli/admin.py:222` | `tolaria_path = …` | `vault_path = …` |
| `init --json` | `cli/admin.py:312` | `"tolaria_path"` | `"vault_path"` |
| Missing-config error | `schemas/config.py:137` | `[core].tolaria_path … your Tolaria vault folder` | `[core].vault_path … your Markdown vault folder` |
| `--path` help | `cli/admin.py:241` | `([core].tolaria_path)` | `([core].vault_path)` |

`init --json`'s key is renamed even though no test pins it (`tests/cli/test_init.py:87-89` asserts
only `payload["path"]` and `payload["agent"]`) — leaving it as `tolaria_path` would make the JSON
payload the one place the old name survives.

---

## Implementation Detail

### Why no ignore rules are needed

Every vault walk is already scoped to the two shards-owned subtrees, so an Obsidian vault's
`.obsidian/` and root `.trash/` are outside the traversal without any filter:

| Call site | Root walked |
|---|---|
| `index/warm.py:71` | `vault/notes`, `vault/tasks` |
| `core/wikilinks.py:59,64` | `_notes_root`, `_tasks_root` |
| `core/notes.py:114` | `_notes_root` |
| `core/tasks.py:163` | `tasks/{open,done}` non-recursive |

`index/watcher.py:154-157` schedules observers on the same two folders. Pointing `vault_path` at an
existing Obsidian vault root is therefore safe: shards creates and watches `notes/` and `tasks/`
and never reads Obsidian's own directories. Adding a hidden-directory filter would be dead code.

### Foreign-file tolerance is already generic

The only Tolaria-aware behaviour in the tree is negative and already tool-neutral: files lacking an
`n-`/`t-` id are skipped (`core/notes.py:135`, `core/wikilinks.py:75`, `core/tasks.py:927`,
`core/context.py:119`) and unknown frontmatter keys round-trip via the `_Frontmatter` stash
(`schemas/note.py`). Both rules apply to any writer sharing the folder. Only the docstrings that
name Tolaria as *the* other writer change.

### Test-fixture blast radius

`tests/conftest.py:63` writes the TOML key for nearly all of the 1326 tests, so the fixture and the
field rename land in one commit. `tests/cli/test_init.py:236` (`assert "tolaria_path" in
result.output`) is the single literal-key assertion and flips to `vault_path`.
`tests/cli/test_init.py:243-256` loads the repo-root `config.example.toml`, so that file must move
in the same step or the suite reddens. Two legacy-alias regression tests are added alongside
`tests/notes/test_schema.py:58-62` (which already covers the `path` alias) to pin that
`tolaria_path` and `path` both still decode.

Fixture identities that merely *mention* Tolaria — the `tolaria-agent` owner used in attribution
tests and the `tolaria_pinned` / `tolaria_meta` unknown-frontmatter keys — are renamed to neutral
equivalents. They are foreign-writer stand-ins, and a foreign writer named after the tool we just
stopped depending on reads as a leftover.

<!-- merge -->
### Vault ownership and the delete rationale

Shards owns the *interface* to a Markdown folder, not the folder. The vault belongs to the
operator, and versioning, sync, and backup are the vault owner's concern — git, Obsidian Sync,
Time Machine, or nothing at all. Shards is not a backup tool and does not promise recovery.

This is what justifies **hard `unlink` with no trash**: shards does not run a delete lifecycle it
would then have to keep in sync with whatever the operator's own history mechanism is, and adding
`.trash/` would create exactly that second lifecycle. The trade-off is explicit — an operator with
no versioning on their vault has no recovery path for a deleted note, and that is their call to
make, not shards's to paper over. The same reasoning is why no `updated_by` audit key is written:
attribution stamps live in the body where a human reads them, and audit history is the vault
owner's mechanism, not a frontmatter field.

Shards's obligation is narrower and unchanged: never corrupt what it did not write. Files with no
shards id are skipped by every id-scoped read, and unrecognised frontmatter keys round-trip
byte-for-byte — for **any** tool sharing the folder, named or not.
<!-- /merge -->

<!-- merge -->
### No notes application is a dependency

Shards requires a directory it can write `notes/` and `tasks/` into. Nothing more: no notes
application need be installed, running, or aware of shards, and no vault marker, version probe, or
layout sniff is performed. `shards init` creates the folder when none is given.

Obsidian is the reference pairing — it is what the maintainer runs, and an Obsidian vault root is a
supported value for `[core].vault_path` — but it is an example, never a requirement, and no code
path knows it exists.
<!-- /merge -->

---

## Open Questions

None.
