# mesh

A mesh for multi-agent collaboration over a single Markdown folder. One Rust binary divides the
folder into five **spaces** — `notes`, `tasks`, `memories`, `scratch`, `assets` — and gives each
one a verb family, plus `search`, read-only session lenses and an MCP server. No database, no
daemon, no background process: every command reads the folder directly and exits.

- **Notes + memories + search = recall.** Notes are durable knowledge; memories are an agent's
  beliefs about the operator and the fleet, ranked by match, importance and recency.
- **Tasks = coordination + handoff**, with a live dependency graph: `claim` / `release` /
  `finish` / `cancel`, `block` / `unblock`, derived readiness and `task next`.
- **Scratch and assets are the working surface** — per-agent session state, and content-addressed
  files beside the vault.
- **Markdown is the source of truth**; mesh owns the interface (and the writes), not the data.

Ranked search delegates to the first-party [`indexed`](https://github.com/LennardZuendorf/indexed)
engine when one is configured, and falls back to a built-in BM25-lite engine that needs nothing
installed. mesh coexists with whatever else writes the folder — a Markdown editor, git, another
MCP server — and needs no database to run.

The spec is the source of truth: see [`.spec/`](.spec/). Working in here? Read
[`AGENTS.md`](AGENTS.md) first. For architecture Q&A — spaces, memories vs. notes vs. scratch,
assets, derived readiness, atomicity, vault/config resolution — see
[`docs/concepts.md`](docs/concepts.md).

---

## Install

Requires **Rust 1.94+** (the toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml)).

```bash
cargo install --path .        # installs `mesh` and `mesh-mcp` into ~/.cargo/bin
# or, without installing:
cargo build --release         # ./target/release/mesh and ./target/release/mesh-mcp
```

`scripts/install.sh` is a one-line wrapper around the first form.

Two binaries are produced from one crate: **`mesh`** (everything, including `mesh mcp`) and
**`mesh-mcp`** (a shim that runs the same stdio MCP server, so the bundled `.mcp.json` needs no
edit). Shell completions come from `mesh completions {bash,zsh,fish,powershell,elvish}`.

## First run

```bash
mesh init
```

`init` writes `~/.mesh/config.toml` (or `$MESH_CONFIG_PATH` when set), creates the vault
directory — and nothing else; the space folders are created lazily on first write, so pointing
mesh at an existing folder scatters no empty directories. Re-running is always safe: with an
existing config and no `--force`, it refuses (exit 2) and never opens the file for writing.

```
mesh init [--path PATH] [--agent ID] [--collections CSV] [--search-collection NAME]
          [--hybrid | --no-hybrid] [--threshold FLOAT] [--engine ENGINE]
          [--spaces | --no-spaces] [--obsidian] [--force]
```

The rendered config:

```toml
[core]
vault_path = "/home/you/.mesh/vault"
agent = "my-agent"

[spaces]
notes = "notes"
tasks = "tasks"
memories = "memories"
scratch = "scratch"
assets = "assets"

[search]
hybrid = true

[tasks]
collections = []
```

`[search].threshold`, `collection` and `engine` are written only when you ask for them
(`--threshold`, `--search-collection`, `--engine` other than `auto`). `--no-spaces` leaves the
`[spaces]` block out entirely — the defaults are identical either way. `--obsidian` is shorthand
for `notes = "."`: the notes space *becomes* the vault root, so an existing Markdown vault is
exposed as-is and the other four spaces sit inside it as strict subfolders (the notes walk skips
them, so `mesh note list` never lists your tasks).

Then:

```bash
mesh note new "hello" --body "first note" --type note
mesh task new "do something" --body "details" --priority high
mesh memory new "operator prefers terse output" --kind preference --importance 4
mesh task list
mesh search "something"
```

---

## Config

Config lives at `~/.mesh/config.toml`. A missing config exits 2 with a three-line message naming
the resolved path and the required key. A committed reference copy — every key `src/config.rs`
reads, documented — is [`config.example.toml`](config.example.toml); `config.toml` itself is
gitignored, since it names a real vault path and identity.

```toml
[core]
vault_path = "~/mesh-vault"   # required; ~-expanded then canonicalised at load
agent      = "my-agent"       # optional; the default owner/claimer identity

[spaces]                      # entirely optional; every key optional
notes    = "notes"            # relative path | absolute path | "." | false
tasks    = "tasks"
memories = "memories"
scratch  = "scratch"
assets   = "assets"

[search]
collection = "my-vault"       # optional; unset ⇒ `indexed` is never invoked
hybrid     = true             # default true
# threshold = 0.65            # deliberately unset; applies only when written
engine     = "auto"           # auto | indexed | builtin | substring
spaces     = ["notes", "tasks", "memories", "assets"]   # the default search corpus

[tasks]
collections = ["my-agent", "peer"]   # roster of valid --owner values; [] = open roster
strict      = false                  # default for `task claim --strict`
```

**`[spaces]` semantics**, per key, in order:

| Value | Meaning |
|---|---|
| absent | the built-in default (`notes`, `tasks`, `memories`, `scratch`, `assets` under the vault) |
| `"subdir"` | a path relative to `vault_path` |
| `"/abs/path"` | an absolute folder, `~`-expanded; added to the sandbox explicitly and listed by `mesh status` |
| `"."` or `""` | the vault root itself |
| `false` | the space is **disabled** — every verb for it exits 2 with `space 'scratch' is disabled in [spaces]`, and it contributes no sandbox root and no search corpus |

Three checks run at load, all exit 2 before any I/O: two enabled spaces may not resolve to the
same directory (except that the notes space may be the vault root while the others are strict
subfolders of it); when notes *is* the vault root, every other enabled root must be a strict
descendant and is excluded from the notes walk; and a relative root may not resolve outside
`vault_path`. Type and status routing is always relative to the **space root**, never the vault
root — with `notes = "."`, `note new --type log` lands in `<vault>/logs/`.

An omitted `[spaces]` table, or any omitted key, resolves to the layout the Python mesh used —
**a pre-rewrite config behaves identically with zero edits.**

**Environment and global flags**

| Var / flag | Effect | Precedence |
|---|---|---|
| `--config PATH` | config file location | above everything |
| `MESH_CONFIG_PATH` | config file location, `~`-expanded, only when non-empty | above `~/.mesh/config.toml` |
| `--vault PATH` | `[core].vault_path` | above `MESH_VAULT` |
| `MESH_VAULT` | `[core].vault_path` | above the file |
| `MESH_AGENT` | `[core].agent`, only when non-empty | above the file |
| `MESH_INDEXED_BIN` | the `indexed` binary to invoke | above the `PATH` lookup |
| `EDITOR` / `VISUAL` | interactive body for `note new` / `memory new` / `scratch set` | `VISUAL`, then `EDITOR` |
| `XDG_RUNTIME_DIR` | `mesh watch` lock directory, else `~/.mesh/run/` | — |

`[core].path` and `[core].tolaria_path` are permanent input aliases for `vault_path`
(precedence `vault_path` > `path` > `tolaria_path`, no warning). Unknown tables and keys are
ignored, never rejected, so a fleet mid-upgrade keeps working; a leftover `[daemon]` table is
inert. A `vault_path` that exists and is not a directory is a config error (exit 2).

`mesh config show` prints the *effective* config after the environment overlay, plus every
resolved space path and the sandbox roots — the fastest way to answer "where will my write
land". `mesh config set core.agent value` edits the file in place, preserving comments and key
order.

---

## CLI surface

`mesh --help` is always the source of truth for the live command list; this is the shape.

```
mesh [--version] [--json] [--quiet] [--owner ID] [--mine] [--config PATH] [--vault PATH] <command>
```

`--json`, `--quiet`, `--owner` and `--mine` are accepted **on either side of the command name**
on every non-admin command, with identical effect. Admin commands (`init`, `status`, `reindex`,
`watch`, `config`, `completions`, `mcp`) take them on the global side only.

**Output classes.** Every command declares one, so precedence is never a guess:

| Class | Commands | Rule |
|---|---|---|
| **M** — mutation | create, append, update, claim, release, finish, cancel, delete, block, unblock, `scratch set`, `asset add`/`attach`/`detach`, `config set` | `--quiet` beats `--json`. Quiet prints the id (or name) alone; JSON is `{"id", …fields, "updated"}` in that order |
| **L** — listing / lens / object / admin | `get`, `list`, the five lenses, `task next`, `asset path`, `asset gc`, `init`, `status`, `reindex`, `watch`, `config` | `--json` beats `--quiet` |
| **S** — search | `search`, `memory recall` | output is *always* one JSON array line; `--json` is accepted and inert; `--quiet` suppresses only the stderr degradation notice |

Timestamps render identically everywhere: `YYYY-MM-DDTHH:MM:SS[.ffffff]Z`, microseconds only when
non-zero. Human previews are a plain 200-code-point slice of the body.

### `mesh note`

`new` · `append` · `update` · `get` · `list` · `delete`

```
note new    TITLE  [--type note|log|decision|reference|project] [--tags CSV] [--owner ID]
                   [--body TEXT] [--file PATH]
note append TARGET TEXT [--section S] [--timestamp]
note update TARGET [--tags SPEC] [--type T] [--title TEXT]
note get    TARGET [--full] [--meta-only] [--related] [--foreign]
note list   [--tags CSV] [--any-tag] [--owner ID] [--type T] [--since DUR|ISO]
            [--sort updated|created|title] [--limit 20] [--foreign]
note delete TARGET [--force]
```

`TARGET` is an `n-` id or a title slug. Types route into folders under the notes root: `note` →
`<notes>/`, and `log`/`decision`/`reference`/`project` → `logs/`, `decisions/`, `references/`,
`projects/`. Rows are `{id}··{type}··{title}` (two spaces).

`--tags` on `update` is a grammar, not a list: bare `x,y` **merges** (additive, idempotent),
`=x,y` replaces the whole list, `+x,-y` is a per-token delta. A mixed spec is exit 2 rather than
a guess.

`--foreign` surfaces non-mesh Markdown living in the notes space (`id: null`, title from the
first `# H1`, else the filename stem). Foreign files stay invisible to every mutating verb.
`note update --title` prints one stderr advisory when the rename dangles `[[Old Title]]`
backlinks — an advisory, never a refusal: mesh does not rewrite bodies it did not write.

### `mesh task`

`new` · `update` · `append` · `claim` · `release` · `finish` · `cancel` · `get` · `list` ·
`delete` · `block` · `unblock` · `next`

```
task new     TITLE [--priority high|normal|low] [--tags CSV] [--owner ID] [--body TEXT]
                   [--project ID] [--blocks CSV] [--blocked-by CSV]
task update  TASK_ID [--priority P] [--tags SPEC] [--title T] [--project ID] [--owner ID]
                     [--blocks CSV] [--blocked-by CSV]
task append  TASK_ID TEXT [--section S] [--timestamp]
task claim   TASK_ID [--strict | --no-strict]
task release TASK_ID [--force] [--note TEXT]
task finish  TASK_ID [--outcome TEXT]
task cancel  TASK_ID [--reason TEXT]
task get     TASK_ID [--full] [--meta-only]
task list    [--status CSV] [--owner ID] [--mine] [--tags CSV] [--any-tag] [--project ID]
             [--since DUR|ISO] [--stale DUR|ISO] [--available] [--ready] [--blocked]
             [--sort updated|created|title|priority] [--limit 20]
task delete  TASK_ID [--force]
task block   TASK_ID --on CSV
task unblock TASK_ID [--on CSV] [--all]
task next    [--claim] [--strict] [--mine] [--project ID] [--tags CSV]
```

`TASK_ID` is id-only — a title slug never resolves a task. Status routes the file:
`open`/`claimed` live in `<tasks>/open/`, `done`/`cancelled` in `<tasks>/done/`, each walked
non-recursively. Rows are `{id}\t{status}\t{claimed_by|-}\t{title}`.

`claim` is an atomic test-and-set under an `O_EXCL` lock: reclaiming your own task is a no-op
that does not rewrite the file, and another agent's claim is exit 4. `release`, `finish` and
`cancel` are idempotent the same way — a second `finish` adds no second `## Outcome` section and
does not bump `updated`. `--since` is a recency floor, `--stale` its exact inverse.

### The task dependency graph

`blocked_by` is authoritative; `blocks` is a best-effort mirror maintained one file at a time,
under that file's own lock. **Readiness is derived, never stored** — it is computed at read time
from the union of both edge directions, so no verb ever writes another entity's file as part of
its own transaction and there is no stored flag to go stale.

- `task block T --on B1,B2` adds edges (additive, order-preserving); `task unblock T --on B1` or
  `--all` removes them. Both are idempotent, and a self-edge or a cycle is exit 2
  (`dependency cycle: t-a -> t-b -> t-a`).
- `task list --ready` = `--available` **and** unblocked; `task list --blocked` = open or claimed
  with at least one unsatisfied blocker. `--available` stays deliberately dependency-blind.
- `task get` prints a `ready: true|false` line and appends `"ready"` last in `--json`.
- `task claim --strict` refuses a blocked task with **exit 5**, writing nothing. Without
  `--strict` the claim succeeds and prints `task t-x is blocked by t-a, t-b` on stderr. The
  default comes from `[tasks].strict`.
- `task finish` / `task cancel` report the tasks their completion unblocked as
  `"unblocked": [...]` — a report, not a cascade of writes.
- `task next` is the "give me work" primitive: it selects the highest-priority ready, unclaimed
  task and, with `--claim`, takes it in the same invocation, retrying across candidates when it
  loses a race. Nothing ready is exit 3 (`no ready task`).

### `mesh memory`

`new` · `append` · `update` · `get` · `list` · `recall` · `forget`

```
memory new    TITLE [--kind fact|preference|procedure|insight|episode] [--scope shared|private]
                    [--importance 1..5] [--source TEXT] [--expires DUR|ISO] [--supersedes m-ID]
                    [--tags CSV] [--owner ID] [--body TEXT] [--file PATH]
memory append TARGET TEXT [--section S] [--timestamp]
memory update TARGET [--tags SPEC] [--title T] [--kind K] [--scope S] [--importance N]
                     [--source TEXT] [--expires DUR|ISO|none] [--owner ID]
memory get    TARGET [--full] [--meta-only] [--related]
memory list   [--kind K] [--scope S] [--tags CSV] [--any-tag] [--owner ID] [--mine]
              [--min-importance N] [--since DUR|ISO] [--include-expired] [--include-superseded]
              [--sort updated|created|title|importance] [--limit 20]
memory recall QUERY [--kind K] [--tags CSV] [--owner ID] [--mine] [--min-importance N]
                    [--limit 10] [--threshold F] [--no-decay] [--include-expired]
                    [--meta-only] [--full]
memory forget TARGET [--force] [--expired]
```

A memory is a note-shaped Markdown file with the id prefix `m-`, laid out **flat** in the
memories space — no identity-derived folders, so a typo'd `$MESH_AGENT` cannot fork a namespace,
and no memory verb ever moves a file. Reads are recursive, so filing memories into subfolders by
hand keeps working. `TARGET` is an `m-` id or a title slug.

| field | default | meaning |
|---|---|---|
| `kind` | `fact` | `fact` \| `preference` \| `procedure` \| `insight` \| `episode`; closed on write, free-form on read |
| `scope` | `shared` | `private` is hidden from listings whose effective owner differs — a courtesy filter, never a security boundary |
| `importance` | `3` | 1..5; a sort and ranking key, never a cutoff |
| `source` | `null` | free text: an id, a URL, a session label. Never interpreted |
| `expires` | `null` | soft TTL; expired memories drop out of `list` and `recall`, and **nothing is ever deleted automatically** |
| `superseded_by` | `null` | set by `memory new --supersedes m-OLD`; excluded from recall and default listings, kept for the audit trail |

There is no `use_count`, no `last_used` and no touch-on-read: a read verb that writes would break
idempotence, contend locks and dirty git on every hit. `recall` ranks

```
importance_weight = 0.6 + 0.1 * importance          # 0.7 .. 1.1
recency           = 0.5 ^ (age_days / 90)           # age from `updated`, not `created`
final             = match_score * importance_weight * (0.35 + 0.65 * recency)
```

so a re-confirmed memory stays hot; `--no-decay` drops the recency term for audits. Decay is
ranking, never deletion — expiry, supersession and `memory forget` are the only ways a memory
leaves recall. `memory forget --expired` is the bulk sweep, under the same delete guard.

### `mesh scratch`

`set` · `append` · `get` · `list` · `clear`

```
scratch set    NAME [--body TEXT] [--file PATH] [-] [--agent ID]
scratch append NAME TEXT [--section S] [--timestamp] [--agent ID]
scratch get    NAME [--agent ID]
scratch list   [--agent ID] [--all-agents] [--since DUR|ISO]
scratch clear  NAME [--force] [--agent ID]
```

Scratch is name-addressed, not id-addressed: the file lives at
`<scratch>/<slugify(agent)>/<slugify(name)>.md` and carries a six-key frontmatter block
(`type`, `name`, `agent`, `tags`, `created`, `updated`) so one reader, one row parser and one
filter path serve the whole system. A name that slugifies to empty is exit 2.

`set` is a whole-body overwrite and is idempotent — an identical body leaves the file's bytes
untouched. `-` reads the body from stdin (the one place mesh reads stdin outside `mesh mcp`).
`get` prints the body **verbatim**, with no preview truncation. `--agent ID` addresses a peer's
namespace for both reads and writes; identity here is a convention, not a boundary.

Scratch is excluded from the default search corpus, from `recent-activity`, `graph`,
`build-context` and `session-start`. It is a workbench, not memory — `search --space scratch`
opts in explicitly.

### `mesh asset`

`add` · `get` · `path` · `list` · `attach` · `detach` · `remove` · `gc`

```
asset add    PATH [--title T] [--tags CSV] [--owner ID] [--caption TEXT] [--attach ID]
asset get    ASSET_ID [--meta-only] [--full]
asset path   ASSET_ID
asset list   [--tags CSV] [--any-tag] [--owner ID] [--mine] [--media-type MT]
             [--since DUR|ISO] [--sort updated|created|title|bytes] [--limit 20]
asset attach ASSET_ID TARGET [--section S]
asset detach ASSET_ID TARGET
asset remove ASSET_ID [--force]
asset gc     [--apply]
```

An asset is a blob plus a sidecar sharing one stem:

```
assets/a-7Q3KDX9M.png     the bytes, verbatim
assets/a-7Q3KDX9M.md      an ordinary mesh entity: filename, media_type, bytes, sha256, blob
```

**The id is the content address** — `a-` plus Crockford base32 of `sha256(bytes)` — so
`asset add` is idempotent by content: identical bytes return the same id, write nothing, do not
bump `updated`, exit 0, and say so on stderr. Ingest is **copy only**; `move` and `link` were
rejected as a data-loss footgun. The blob is written **before** the sidecar, and a failed sidecar
write unlinks the blob: a crash can leave an orphan blob that `asset gc` finds, never a sidecar
pointing at nothing. The source extension is kept only when it is lowercase `[a-z0-9]{1,12}`;
the original filename is preserved in frontmatter as data, never as a path component, so a
hostile filename can never traverse.

`asset attach a-X TARGET` appends `![[a-7Q3KDX9M.png]]` to the target's body through the ordinary
append path and links both `related` lists, so the pair shows up in `graph`/`build-context` for
free. `asset path` prints the absolute blob path and nothing else — that is what gets piped into
an image tool. `asset remove` on a still-referenced asset is exit 2 unless you pass `--force`.

### `mesh search`

```
mesh search [QUERY] [--type T] [--tags T]... [--owner O] [--status S] [--kind K]
            [--space CSV] [--engine auto|indexed|builtin|substring] [--limit 10]
            [--threshold F] [--meta-only] [--full] [--health]
```

Output is **always** one JSON array line on stdout. `--tags` is repeatable and ANDed; with no
query it becomes an exact tag pull (`score = 1.0`, metadata only). Hit keys, in order: `id`,
`type`, `title`, `score`, `path` always; then `tags`, `owner`, `updated`, `snippet` and `space`
when they apply. There is no `body` key — `--full` overloads `snippet`.

Engines:

| `--engine` | Behaviour |
|---|---|
| `auto` (default) | `indexed` when `[search].hybrid` is on, a collection is configured and the binary is on `PATH`; otherwise the built-in engine |
| `indexed` | the `indexed` branch even with `hybrid = false`; degrades with the standard notice when unavailable |
| `builtin` | BM25-lite over the corpus, with the four legacy tiers (title-exact 1.0, title-substring 0.8, tag 0.6, body 0.4) kept as score floors |
| `substring` | the legacy scoring and head-of-body snippets, byte-for-byte |

`indexed` invocations get a 30-second wall clock; a timeout, a missing binary or a non-zero exit
degrades to the built-in engine with one stderr notice and is never an error. `--space` defaults
to `[search].spaces` (notes, tasks, memories, assets — scratch is opt-in). `--health`
short-circuits before everything else, never shells `indexed`, and reports which branch would run
and why:

```json
{"mode":"fallback","hybrid_configured":true,"collection":null,
 "daemon_up":false,"indexed_binary_available":false,
 "reason":"no collection configured ([search].collection unset)"}
```

`--threshold` resolves as the flag, then `[search].threshold` **only when it is physically
present in the TOML**, then the engine's own `0.4` floor. That is why `mesh init` omits the key:
writing the nominal `0.65` makes the tag and body tiers unreachable.

Two commands keep an external index fresh, and both are optional:

```bash
mesh reindex               # `indexed index create <root> --collection C`; always exit 0
mesh watch                 # foreground watcher: reconcile misfiled files + incremental index
mesh watch --once          # one sweep and exit
```

`mesh watch` is an accelerator, never a gatekeeper: every command behaves identically whether or
not a watcher is running. A second watcher on the same vault exits 4.

### Lenses and admin

Read-only lenses, all accepting `--space CSV`: `recent-activity` (`--since`, `--owner`, `--mine`,
`--limit`), `build-context SEED_ID` (`--depth`), `graph SEED_ID` (`--depth`,
`--direction out|in|both`), `project PROJECT_ID`, and `session-start` (`--owner`, `--team`,
`--meta-only`, `--no-memories`, `--budget`). `session-start` composes tasks → mentions →
memories → recent activity, deduped by id with the earlier section winning and a `reason` on
every entry; `--budget N` trims bodies first, then whole entries, and records the drop as one
final `{"reason": "truncated", "dropped": N}` entry.

Admin: `mesh init`, `mesh status`, `mesh reindex`, `mesh watch`,
`mesh config {path,show,get,set}`, `mesh completions SHELL`, `mesh mcp`. `mesh status` is
strictly read-only and reports counts per space, freshness, dangling links (capped at 50, with
the real total alongside), stale locks, the per-agent claim breakdown, the dependency summary
(`blocked`, `ready`, `cycles`, `dangling_blockers`), every resolved space path and watcher
liveness. There is no `mesh doctor`: `status` plus `config show` cover it.

### Exit codes and errors

| code | meaning |
|---|---|
| 0 | ok |
| 1 | io / infrastructure (`io error: {e}`); a declined delete prompt; an internal error |
| 2 | validation: a bad enum value, sort, tag spec or direction; an ambiguous slug; an unknown owner; a sandbox escape; a disabled space; a missing config; no body on a headless path; the delete guard; a referenced `asset remove`; a self-edge or a cycle; a parse failure; the no-args help case |
| 3 | not found — including any corrupt-frontmatter entity on a read or amend verb (it stays deletable, which is the repair path), and `task next` with nothing ready |
| 4 | claim conflict, contended lock, a second `mesh watch` |
| 5 | blocked: `task claim --strict` / `task next --strict --claim` on a task with an unsatisfied blocker |

There is never a backtrace: a panic that escapes prints `internal error: {msg}` and exits 1.

Human mode writes one plain-text line to stderr. Under `--json`, a failure writes **one JSON
object** to stderr instead, with identical exit codes:

```json
{"kind":"not_found","message":"note not found: nope",
 "next_action":"check the id and retry, or list to find the right one",
 "id_or_slug":"nope","candidates":["n-1WVR"]}
```

`candidates` appears on a not-found (near misses by edit distance), `retry_after_ms` on a lock
conflict. Degradation notices — the substring fallback, a blocked claim, a duplicate title, a
dangling rename — are one stderr line, suppressed by `--quiet`, and **never** enter a payload.

**Delete guard**, on `note delete`, `task delete`, `memory forget`, `scratch clear` and
`asset remove`: `--force` deletes immediately; without it, a machine path (`--json`/`--quiet`) or
a non-tty is exit 2 with nothing removed, and a tty prompts `Delete {id}? [y/N]: ` (anything but
`y` is exit 1). Delete is a hard `unlink` — there is no trash and no promised recovery, by
design; versioning is the vault owner's job.

---

## Compatibility with a Python-era vault

A vault written by mesh 0.1.0 is read as-is. The reader accepts everything PyYAML wrote —
alphabetically sorted keys, space-separated timestamps (`2026-09-05 07:27:02.307028+00:00`),
bare dates, naive datetimes, non-UTC offsets, quoted scalars, anchors and aliases, `null`, `[]`,
and any unknown key including one literally named `extra`. Unknown keys round-trip in place and
are never dropped; a file mesh only reads is never rewritten.

On the **first write** to a file, mesh re-emits it in its own canonical form. Three things
change, and nothing else does:

1. **Key order** is the model's declaration order (`id, type, title, tags, owner, created,
   updated, related, …space-specific…`) followed by unknown keys in their original order —
   readable for a human and for a Markdown editor, rather than alphabetical.
2. **Timestamps mesh sets** are RFC 3339 UTC `YYYY-MM-DDTHH:MM:SSZ` (with `.ffffff` only when
   non-zero) instead of PyYAML's space-separated `+00:00` form. Scalars mesh did not modify are
   re-emitted verbatim from their preserved raw text.
3. **One trailing newline** at end of file, where the Python era stripped it.

Compatibility is *semantic*, not byte-level: a checked-in Python-written vault
(`tests/fixtures/python-vault/`) gates the test suite on every typed field, every unknown key and
every listing order surviving a load → dump → load round trip. Machine JSON pins key **order**,
not whitespace. Existing ids are never recomputed, so no id ever changes.

---

## MCP

```bash
mesh mcp        # the stdio MCP server
mesh-mcp        # the same server, as its own binary (what the plugin bundle wires up)
```

JSON-RPC 2.0 over stdio, newline-delimited, protocol `2025-06-18`. On connect the server sends an
`instructions` block built from your live config — your resolved identity, the valid-owner
roster, the vault path, the current search mode and which-space-wins guidance — so a client is
oriented before making any tool call, with no separate skill required. A config that fails to
load is never fatal: tools then fail per call with a structured `config_missing` error.

**37 tools**, each carrying explicit `readOnlyHint` / `idempotentHint` / `destructiveHint`
annotations (RO = read-only, IDEM = idempotent):

| Family | Tools |
|---|---|
| notes | `mesh_note_get` (RO), `mesh_note_list` (RO), `mesh_note_new`, `mesh_note_append`, `mesh_note_update` (IDEM) |
| tasks | `mesh_task_get` (RO), `mesh_task_list` (RO), `mesh_task_new`, `mesh_task_append`, `mesh_task_claim` (IDEM), `mesh_task_release` (IDEM), `mesh_task_finish` (IDEM), `mesh_task_update` (IDEM), **`mesh_task_cancel` (DESTRUCTIVE)**, `mesh_task_block` (IDEM), `mesh_task_unblock` (IDEM), `mesh_task_next` |
| memories | `mesh_memory_new`, `mesh_memory_append`, `mesh_memory_update` (IDEM), `mesh_memory_get` (RO), `mesh_memory_list` (RO), `mesh_memory_recall` (RO) |
| scratch | `mesh_scratch_set` (IDEM), `mesh_scratch_append`, `mesh_scratch_get` (RO), `mesh_scratch_list` (RO) |
| assets | `mesh_asset_get` (RO), `mesh_asset_list` (RO), `mesh_asset_attach` (IDEM) |
| search + lenses | `mesh_search` (RO), `mesh_health` (RO), `mesh_recent_activity` (RO), `mesh_build_context` (RO), `mesh_graph` (RO), `mesh_project` (RO), `mesh_session_start` (RO) |

`mesh_task_cancel` is the **only** destructive tool, which is exactly why every removal verb is
withheld. **Not exposed over MCP:** `note delete`, `task delete`, `memory forget`,
`scratch clear`, `asset remove`, `asset add` (it reads an arbitrary filesystem path — a human
act), `asset gc`, and all admin (`init`, `status`, `reindex`, `watch`, `config`, `completions`,
`daemon`). No registered tool name contains `delete`, `daemon`, `reindex` or `status`. Failures
cross as the same structured envelope the CLI emits under `--json`, never a stack trace.

Register the server by hand with any MCP-capable client:

```json
{
  "mcpServers": {
    "mesh": {
      "command": "mesh-mcp"
    }
  }
}
```

### Install the plugin

This repo doubles as its own Claude Code plugin marketplace
(`.claude-plugin/marketplace.json` at the repo root), so the MCP server and the `mesh` skill
install together in one step — from Claude Code:

```
/plugin marketplace add <path-or-url-to-this-repo>
/plugin install mesh@mesh
```

That installs `plugins/mesh/`: the bundled `.mcp.json` (wiring `mesh-mcp` in, so the skill can
never be installed without the tools it describes), the `mesh` skill
(`skills/mesh/SKILL.md` — the vault-coherence and coordination playbook, one skill, not split by
verb), and an optional `SessionStart` hook that runs `mesh session-start --meta-only --json` to
warm-start a fresh session's queue, mentions and memories. The binaries still have to be
installed and `mesh init` run once per machine — the plugin ships the wiring and the playbook,
not a config.

`SKILL.md`'s frontmatter stays inside the six-field spec Claude accepts for a claude.ai skill
upload (`name`, `description`, `license`, `compatibility`, `metadata`, `allowed-tools`), so the
same file works as this plugin's local skill and as an account-enabled skill for Cowork sessions,
which never read a local `.claude/skills/` directory — those sessions get their orientation from
the MCP `instructions` block instead. `allowed-tools` lists the 36 non-destructive tools;
`mesh_task_cancel` is deliberately left out so it always asks first.

---

## Migrating from the Python mesh

The vault needs no migration (see
[Compatibility with a Python-era vault](#compatibility-with-a-python-era-vault)), and neither
does the config. Five things changed on the surface:

- **The daemon is gone.** `mesh daemon start|stop|status` still parses — it is a hidden,
  non-spawning shim that keeps every key, string and exit code so no existing script crashes —
  but nothing runs in the background any more. A full-vault scan in Rust is milliseconds, so
  every command reads disk directly and a write is visible to the very next read by
  construction. What the daemon did for search freshness and folder reconciliation is now
  **`mesh watch`**, an explicit foreground process you start when you want it.
  `recent-activity` no longer emits `daemon down, scanning the folder directly`, because the
  direct scan is the only path; `search --health` keeps its `daemon_up` key at the same position
  and now reports watcher liveness.
- **Errors under `--json` are a JSON object on stderr**, not plain text. Human mode, stdout and
  every exit code are unchanged.
- **`--blocks` / `--blocked-by` are live**, not inert: they are cycle-checked and mirrored, and
  **exit code 5** is now reachable from `task claim --strict`. `--limit -1` keeps its meaning
  (negative is unbounded, `0` yields an empty list) and now parses in both the
  space-separated and the `--limit=-1` form.
- **`--help` is clap's rendering**, not rich's — no box drawing, and a bracketed string prints
  literally (`Owner identity (must be in [tasks].collections).`). Every usage line, ordering,
  help string, default and required marker is preserved; only the frame changed.
- **New keys are appended, never moved.** `mesh status` keeps every key at its old position and
  adds the per-space, dependency, spaces and watcher groups after `agents` (with
  `dangling_links` now capped at 50 and `dangling_links_total` carrying the real count);
  `task get` appends `"ready"` last; `session-start` gains `reason: "memory"` entries,
  `--no-memories` and `--budget`. Unknown config tables and keys are now ignored rather than
  rejected, so a fleet mid-upgrade keeps working.

Everything else — every flag, default, output line, key order, stderr notice and exit code — is
preserved. `mesh --version` now prints `0.2.0`.
