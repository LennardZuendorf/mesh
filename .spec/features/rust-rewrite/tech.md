---
type: feature-tech
feature: rust-rewrite
sibling: product.md
parent: ../../tech.md
updated: 2026-09-05
---

# Feature: Rust Rewrite — Architecture

One Cargo package, two binaries, no async runtime, no daemon. `mesh` parses, dispatches into a
library of pure domain functions that read and write Markdown directly, maps one error enum onto
a fixed exit code, and exits. Frontmatter is an ordered map mutated key by key; typed structs are
derived views used for validation and machine output, never the serialisation source on an amend
path. Readiness, relations and every listing are computed from a single vault walk at read time.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)
**Plan:** [plan.md](plan.md)

---

## Files

Single package `mesh` (version 0.2.0, edition 2021, toolchain 1.94) at the repo root. Crate name
in code is `mesh`; `src/lib.rs` carries `#![forbid(unsafe_code)]` plus denies on `unwrap_used`,
`expect_used`, `panic`, `todo` and `string_slice` (`#[cfg(test)]` modules may allow them);
`indexing_slicing` is deliberately not denied.

```
Cargo.toml, Cargo.lock             pinned deps; CI builds --locked
rust-toolchain.toml                channel 1.94.0
rustfmt.toml, clippy.toml          max_width 100; disallowed-method: sort_unstable* in select/search
deny.toml                          advisories, bans, licenses

src/lib.rs                         pub mod surface + crate lints                       [frozen]
src/main.rs                        parse -> dispatch -> map error -> ExitCode, catch_unwind [frozen]
src/bin/mesh-mcp.rs                shim -> mesh::mcp::serve_stdio()                    [frozen]

src/error.rs                       MeshError, code(), Display == the legacy message strings
src/config.rs                      config structs, load, legacy aliases, env overlay, explicit-key set
src/spaces.rs                      Space enum, resolution, sandbox root set, notes exclusions
src/ids.rs                         Crockford base32 over SHA-256, min 4 chars, collision extension
src/timefmt.rs                     iso_z, iso_seconds_z, lenient ISO parse, --since/--expires grammar
src/text.rs                        slugify, duration, wikilink scan, id-form, heading match, preview
src/render.rs                      JSON builders: entry(), hit(), row(); FieldOrder application

src/fm/mod.rs                      Doc, Meta, Row re-exports
src/fm/value.rs                    Value enum incl. Value::Ts (raw text preserved)
src/fm/load.rs                     frontmatter split + yaml-rust2 event loader
src/fm/emit.rs                     the canonical hand-rolled emitter (declaration order, ~150 LOC)
src/fm/doc.rs                      read_doc / read_meta_only / read_body / dump_doc / write_doc

src/storage/mod.rs                 re-exports
src/storage/atomic.rs              sibling temp -> mode match -> fsync -> rename -> parent fsync
src/storage/lock.rs                O_EXCL acquire/hold, TTL staleness, flock + inode CAS, pid liveness
src/storage/sandbox.rs             safe_resolve over the multi-root sandbox
src/storage/walk.rs                THE walk: iter_md(root, recursive) + the skip set

src/model/mod.rs                   re-exports
src/model/common.rs                FieldOrder, Row trait, unknown-key stash rules, tag/owner helpers
src/model/note.rs                  Note view, NOTE_FIELDS, type -> folder routing
src/model/task.rs                  Task view, TASK_FIELDS, status/priority tables
src/model/memory.rs                Memory view, kinds, scopes
src/model/scratch.rs               Scratch view (name-addressed, no id)
src/model/asset.rs                 AssetSidecar view, blob naming, 30-entry media-type table

src/domain/mod.rs                  re-exports
src/domain/select.rs               THE filter/sort/limit engine, generic over a view
src/domain/tags.rs                 apply_tag_spec + TAG_SPEC_SEMANTICS
src/domain/owner.rs                validate_owner against [tasks].collections — every space
src/domain/wikilinks.rs            link scan, title index, resolve, find_dangling
src/domain/notes.rs                note verbs
src/domain/tasks.rs                task lifecycle verbs
src/domain/deps.rs                 edges, readiness, cycles, cascade report, next-selection
src/domain/memories.rs             memory verbs, recall composition, expiry, decay
src/domain/scratch.rs              scratch verbs
src/domain/assets.rs               ingest, sidecar, attach/detach, gc
src/domain/activity.rs             scan_recent, filters, the 7-key row
src/domain/context.rs              BFS, inbound index, GraphResult, tree lines
src/domain/lenses.rs               project_view, session_mentions, session_start, status_report

src/search/mod.rs                  Engine route() — the single entry point
src/search/corpus.rs               space-aware corpus walk, CorpusRow, filters, to_result
src/search/tokenize.rs             the one tokenizer (documents and queries)
src/search/builtin.rs              BM25-lite + the four pinned tiers as floors + snippet windowing
src/search/tagpull.rs              TagPullFilter, select_tagpull
src/search/indexed.rs              subprocess, NDJSON decode, 30 s wall clock, epsilon comparator
src/search/health.rs               the --health payload

src/ctx.rs                         Ctx { config, globals, tty, stdout, stderr }        [frozen]
src/cli/mod.rs                     the COMPLETE clap derive tree, one arm per verb file [frozen]
src/cli/globals.rs                 GlobalOpts, coalesce(), owner resolution             [frozen]
src/cli/out.rs                     emit_mutation/rows/object, notices, delete guard, error envelope [frozen]
src/cli/{note,memory,task,task_dep,scratch,asset,search,lens,admin,watch}.rs   one verb family each

src/mcp/mod.rs                     serve_stdio(): stdio loop, lifecycle
src/mcp/proto.rs                   JSON-RPC 2.0 line framing
src/mcp/schema.rs                  hand-written JSON Schemas + explicit annotations per tool
src/mcp/tools.rs                   the 37-tool table + dispatch (thin adapters over the domain)
src/mcp/errors.rs                  structured error payload (identical object to the CLI envelope)
src/mcp/instructions.rs            the instructions block (2048-byte budget)

tests/common/mod.rs                VaultFixture — drives the real binary through --config
tests/fixtures/python-vault/**     byte-frozen vault written by the Python binary before deletion
tests/fixtures/fake-indexed/       stub engine that echoes fixture NDJSON and records its argv
tests/compat_corpus.rs             semantic-fidelity gate over the frozen vault
tests/{note,memory,task,task_dep,scratch,asset,search,lens,mcp,admin}_cli.rs
tests/race.rs                      N-process claim/append/lock races
tests/bundle.rs                    plugin-bundle invariants against the live clap tree + tool table
scripts/smoke.sh                   temp vault, ~30 verbs, assert exit 0
```

**Dependencies.** clap 4.5 (derive, env, wrap_help), clap_complete 4.5, serde 1, serde_json 1
(`preserve_order`), indexmap 2, yaml-rust2 0.10, toml 0.9, toml_edit 0.23, chrono 0.4
(`default-features = false`, std + clock), sha2 0.10, tempfile 3, rustix 1 (fs, process),
walkdir 2, notify 8, notify-debouncer-full 0.6, dirs 6, thiserror 2, which 7. Dev: assert_cmd 2,
predicates 3, tempfile 3, serial_test 3. Rejected: `regex` (five hand-written scanners, one of
which needs a negative lookahead), `anyhow` (exit codes are a contract), any MCP SDK (pulls an
async runtime; we must byte-control the schemas), `mime_guess`, `insta`, `proptest`.

---

## Contract / API

### Config

```toml
[core]
vault_path = "~/mesh-vault"     # required; ~-expanded THEN non-strict canonicalised
agent      = "my-agent"         # optional; $MESH_AGENT wins when set and non-empty

[spaces]                        # NEW; the whole table and every key is optional
notes    = "notes"              # relative path | absolute path | "." (vault root) | false
tasks    = "tasks"
memories = "memories"
scratch  = "scratch"
assets   = "assets"

[search]
collection = "my-vault"         # unset => `indexed` is never invoked
hybrid     = true
# threshold = 0.65              # deliberately omitted by `mesh init`
engine     = "auto"             # NEW: auto | indexed | builtin | substring
spaces     = ["notes", "tasks", "memories", "assets"]   # NEW: default search corpus

[tasks]
collections = ["my-agent", "peer"]   # roster; [] = open roster
strict      = false                  # NEW: default for `task claim --strict`
```

Legacy aliases `[core].path` and `[core].tolaria_path` fold into `vault_path`
(`vault_path` > `path` > `tolaria_path`) and are stripped before decoding. `[search].threshold`
explicitness is recorded from the raw table before decoding — the body-recall tier depends on it.
Unknown tables and keys are ignored; a present `[daemon]` table is ignored with one advisory from
`status`. Precedence: `--config` > `$MESH_CONFIG_PATH` > `~/.mesh/config.toml`; `--vault` >
`$MESH_VAULT` > the file; `$MESH_AGENT` > the file.

**Space resolution**, per space, in order: `false` → disabled (every verb for it exits 2, no
sandbox root, no corpus); absolute → expanded and canonicalised verbatim; `"."`/`""` → the vault
root; relative → joined onto the vault root; absent → the built-in default. Three validations run
at load, all before any I/O, all exit 2: duplicate enabled roots; containment (if notes is the
vault root, every other enabled root must be a strict descendant and is recorded as a notes-walk
exclusion); escape (a *relative* root may not resolve outside the vault; an absolute one may and
is added to the sandbox set explicitly). Type/status routing is always relative to the **space
root**, never the vault root. Directories are created lazily on first write; `mesh init` creates
only the vault root.

**Sandbox.** `safe_resolve(spaces, candidate)` canonicalises both sides with realpath semantics,
resolving a non-existent tail against its deepest existing ancestor, and accepts the path iff it
equals or is beneath **any** enabled root; otherwise `Validation("path escapes sandbox {root}: {resolved}")`.

### The walk and its skip set

`storage::walk::iter_md(root, recursive)` is the only walk in the tree. It skips: any path
component beginning with `.`; anything under a notes-exclusion root; files larger than 4 MiB;
files that are not valid UTF-8. A malformed or unreadable file yields `None`, never an error.
`read_meta_only` stops at the closing `---` and serves every scan that does not need a body.

### On-disk models

All five spaces share the base frontmatter block in this declaration order:
`id, type, title, tags, owner, created, updated, related`, then per-space fields, then unknown
keys in their original order. **Frontmatter key order on disk == machine-JSON key order ==
declaration order**, driven by one `FieldOrder` per model. Filenames are `<id>.md` in every
id-bearing space; scratch is the sole exception and is name-addressed.

| Space | Prefix | Layout | Extra fields (declaration order) |
|---|---|---|---|
| notes | `n-` | `<notes>/`, `logs/`, `decisions/`, `references/`, `projects/` by type; recursive walk | — |
| tasks | `t-` | `<tasks>/open/` (open, claimed), `<tasks>/done/` (done, cancelled); **non-recursive** | `status`, `priority`, `claimed_by`, `project`, `blocks`, `blocked_by` |
| memories | `m-` | flat `<memories>/m-XXXX.md`; reads recurse; **no verb ever moves a memory file** | `kind`, `scope`, `importance`, `source`, `expires`, `superseded_by` |
| scratch | — | `<scratch>/<agent-slug>/<name-slug>.md` | see below |
| assets | `a-` | `<assets>/a-XXXX.<ext>` (blob) + `<assets>/a-XXXX.md` (sidecar) | `filename`, `media_type`, `bytes`, `sha256`, `blob` |

**Memory fields.** `kind` default `fact`, closed on write to `fact|preference|procedure|insight|episode`
and free-form on read (the `priority` precedent); `scope` default `shared`, `shared|private`,
where `private` is hidden from listings whose effective owner differs — a courtesy filter, never
authorisation; `importance` 1..5 default 3, a sort and ranking key, never a threshold; `source`
free text, never interpreted; `expires` a soft TTL that deletes nothing; `superseded_by` an `m-`
id, excluded from recall and default listings, kept for audit. There is **no** use-count or
last-used field and **no** touch-on-read: a read verb that writes breaks idempotence, contends
locks, makes the read-only MCP hint a lie and dirties the vault.

**Scratch frontmatter**, in this order: `type: scratch`, `name`, `agent`, `tags`, `created`,
`updated`. No id. A name that slugifies to empty is exit 2.

**Asset sidecar.** `media_type` from a 30-entry static extension table, else
`application/octet-stream`; `sha256` the full lowercase hex digest; `blob` the blob filename
relative to the assets root. The blob keeps the source extension lowercased only when it matches
`[a-z0-9]{1,12}`. Write order is **blob, then sidecar** — a crash after the blob leaves an orphan
`asset gc` finds; the reverse would leave a sidecar every read verb surfaces as an asset pointing
at nothing. A failed sidecar write unlinks the blob best-effort.

### Frontmatter read and emit

**Read** accepts everything the Python era wrote: alphabetically sorted keys, space-separated
timestamps with microseconds and an offset, `null`, `[]`, block lists at column 0, quoted
strings, bare dates, naive datetimes, and anchors/aliases (resolved by the parser; never
re-emitted). Timestamps arrive as strings and are parsed leniently — `T` or space separator,
optional fractional seconds, `Z` / `+HH:MM` / none. YAML-1.2 scalar resolution is sufficient; no
YAML-1.1 `yes/no` handling is required. Naive datetimes are reinterpreted as UTC, never shifted;
a bare date becomes midnight UTC.

**Emit** is hand-rolled and deterministic:

| Rule | Value |
|---|---|
| key order | model declaration order, then unknown keys in original order (**not** alphabetical) |
| timestamps we set | `YYYY-MM-DDTHH:MM:SSZ`, `.ffffff` only when non-zero |
| a `Value::Ts` read from disk and not modified | re-emitted from its preserved raw text, verbatim |
| null / empty list | `null` / `[]` |
| non-empty list | block style, two-space indent under its key |
| nested map | two-space indent |
| strings | plain unless empty, whitespace-edged, starting with an indicator char, containing `: ` or ` #` or a newline, or plain-resolvable to null/bool/int/float/timestamp — then double-quoted with JSON-style escapes |
| line folding | none (no width folding, no anchors) |
| document | `---\n<yaml>---\n\n<body>`, body trailing-trimmed, **one trailing newline** |

Unknown keys survive because they are never removed from the ordered map; the literal key `extra`
is an ordinary unknown key, never bound to a field. Absent optionals stay absent because only
keys the command was asked to set are inserted.

**Machine JSON** is `serde_json::to_string` (compact, `preserve_order`), one line, `\n`-terminated.
Key *order* is a contract; whitespace is not.

### IDs

`SHA-256(created_iso + "\0" + title)`, the digest read as a big-endian unsigned integer and
rendered MSB-first in Crockford base-32 (`0123456789ABCDEFGHJKMNPQRSTVWXYZ`) without zero
padding, truncated to 4 characters and extended one character at a time while a file with that
stem exists. `created_iso` is the same `YYYY-MM-DDTHH:MM:SS[.ffffff]Z` string the emitter writes.
Asset ids substitute `sha256(bytes)` for the title digest, which makes the id the content
address and preserves the universal `<id>.md` filename invariant for the sidecar. Python-era ids
are never recomputed. The id-form pattern is `^[nmta]-[0-9A-Za-z]+$`; title-form wikilinks resolve
against the **notes** title index only.

### Locks and atomic writes

`atomic_write`: sibling `mkstemp` → write → match the destination's mode when it exists (else
`0o666 & ~umask`) → `fsync` → `rename` → best-effort parent `fsync`. Any failure before the
rename unlinks the temp and leaves the destination untouched.

Locks live at `<space-root>/.locks/<id>.lock`, per space at `<space-root>/.locks/_create.lock`
for creates, and at `<scratch-root>/.locks/<agent-slug>/<name-slug>.lock` for scratch (a
directory per agent, so `agent a-b`/`name c` cannot collide with `agent a`/`name b-c`). TTL 300 s,
3 attempts, 15 s bounded wait, 10 ms poll. Both removals — reclaiming a peer's stale lock and
releasing your own — are compare-and-swaps holding an open descriptor and an exclusive `flock`,
unlinking only when the file at the path is still that same `(dev, ino)`. A `PermissionError`
from the liveness probe means alive. **Every mutating verb re-resolves its target inside the
lock.** The watcher's reconcile move takes the same lock, non-blocking.

### Task readiness

```
effective_blockers(T) = T.blocked_by  ∪  { S : T ∈ S.blocks }
satisfied(B)          ⇔ B.status ∈ {done, cancelled}  ∨  B does not exist
ready(T)              ⇔ T.status == "open" ∧ T.claimed_by is null
                        ∧ every B in effective_blockers(T) is satisfied
```

`blocked_by` is authoritative; `blocks` is a best-effort mirror for readability and forward
traversal. Readiness is computed from the same `Vec<Row>` the list path already built — one scan
of both task folders, no second walk. `ready()` never recurses, so a hand-made cycle can never
hang a read. Edge mutation validates ids, rejects self-edges, runs an iterative DFS cycle check
over the *proposed* graph before any write, writes the authoritative side under its own lock,
then mirrors onto each blocker **under its own lock, one at a time, in ascending id order,
released before the next** — never two locks at once, so concurrent edge writes cannot deadlock;
a failed mirror is a warning, not an error. The unblock cascade is a **report**:
`dependents(X) = X.blocks ∪ { T : X ∈ T.blocked_by }`, filtered by `ready`, emitted on stderr and
appended to the machine payload. `task next` selects among ready tasks by owner-eligibility, then
priority rank, then FIFO by `created`, then path, and with `--claim` retries across up to 3
candidates on a claim conflict.

### Recall ranking

```
importance_weight = 0.6 + 0.1 * importance          # 0.7 .. 1.1 (default importance 3)
recency           = 0.5 ^ (age_days / 90)           # age from `updated`, not `created`
final             = match_score * importance_weight * (0.35 + 0.65 * recency)
```

`--no-decay` drops the recency term. Decay is ranking, never deletion.

### Search routing and scoring

```
engine == "substring"                                              -> Builtin(substring)
engine == "builtin"                                                -> Builtin(bm25)
hybrid ∧ collection.is_some() ∧ indexed reachable on PATH           -> Indexed
otherwise                                                          -> Builtin(bm25)
```

The `--health` gate ladder reports the **first closed gate**, in order: hybrid disabled, no
collection configured, indexed binary not found. The reported `mode` is the branch actually
taken, never predicted from the gates. `daemon_up` keeps its key position and now reports watcher
liveness. `indexed_available()` is a pure PATH (or `$MESH_INDEXED_BIN`) lookup that never
executes the binary.

Built-in scoring, one tokenizer for documents and queries (lowercase, split on non-alphanumeric,
no stemming, no stop-words):

```
idf(t)      = ln(1 + (N - df(t) + 0.5) / (df(t) + 0.5))
tf_sat(f,t) = tf(f,t) / (tf(f,t) + 1.2)
raw         = Σ_t idf(t) · (3.0·tf_sat(title,t) + 2.0·tf_sat(tags,t)
                          + 1.5·tf_sat(headings,t) + 1.0·tf_sat(body,t))
bm25        = raw / (Σ_t idf(t) · 3.0)
score       = max(bm25, tier)
tier        = 1.0 title == query | 0.8 query ⊂ title | 0.6 query ⊂ any tag | 0.4 query ⊂ body | 0.0
```

The four pinned tiers are floors, so the ranker provably cannot lose a hit the legacy substring
scan would have returned. `--engine substring` disables the BM25 term entirely and restores
head-of-body snippets. Threshold resolution: `--threshold`, else `[search].threshold` only when
physically present in the TOML, else the engine floor `0.4`; comparison is strict `<`. Ordering
is `score desc`, `updated desc` (undated last), `path asc`, stable. The ±0.02 epsilon comparator
is kept **only** on the `indexed` path, reproduced as the same pairwise comparison (it is not a
total order and is not "fixed" into a derived `Ord`).

`indexed` subprocess forms, argv byte-identical, no shell:

```
indexed index search <query> --collection <C> --json --limit <N>
indexed index update <path>  --collection <C>
indexed index create <root>  --collection <C>
```

Bounded by a 30 s wall clock; a timeout degrades to the built-in engine with the standard notice
and is never an error. NDJSON decoding skips blank and malformed lines, rejects a boolean
`score`, coerces an integer one, ignores unknown keys, and treats an absent snippet as none. Each
surviving hit is threshold-filtered, `safe_resolve`d (dropped on escape), re-read (dropped when
unreadable), re-filtered, and emitted with the sandbox-resolved realpath.

Corpus = every `*.md` under the resolved roots of `[search].spaces`, recursive, in space order,
through the one walk and the one safe reader. Foreign files carry `id: null`; assets contribute
sidecars only. For a Python-era vault the default corpus is byte-identical to the old notes +
tasks walk.

### Errors and exit codes

```rust
pub enum MeshError {
  Validation(String), NoteNotFound(String), TaskNotFound(String), MemoryNotFound(String),
  AssetNotFound(String), ScratchNotFound(String), SeedNotFound(String), ProjectNotFound(String),
  AmbiguousSlug { slug, ids, detail }, ClaimConflict { task_id, existing_owner }, Lock(String),
  Blocked { task_id, blockers }, ConfigMissing(String), Io(#[from] std::io::Error),
}
```

| code | variants |
|---|---|
| 1 | `Io`; a declined delete prompt; a caught panic (`internal error: {msg}`) |
| 2 | `Validation`, `AmbiguousSlug`, `ConfigMissing`, clap's own parse failures, the no-args help case |
| 3 | every `*NotFound`; any corrupt-frontmatter entity on a read or amend verb; `task next` with nothing ready |
| 4 | `ClaimConflict`, `Lock`, a second `mesh watch` |
| 5 | `Blocked` |

`Display` reproduces the legacy message strings byte-for-byte, including the sorted-id ambiguous
slug form and the three-line missing-config text. `main` is the only place that touches process
exit and wraps dispatch in `catch_unwind`, so no Rust panic message ever reaches a user. The
corrupt-entity rule is implemented once, in `validated_view()`.

**Error envelope** (machine mode only, one JSON object on stderr, same exit code):

```
{"kind","message","next_action", <structured fields in a fixed order>, ["candidates"], ["retry_after_ms"]}
```

`kind` ∈ the nine legacy values plus `blocked`. Structured field order:
`task_id, existing_owner, id_or_slug, slug, ids, seed_id, project_id, cfg_path`. `candidates` is
up to 5 nearest ids by slug edit distance within the relevant space; `retry_after_ms` comes from
the lock protocol's own poll interval. No `next_action` may read as an authorisation decision —
`not authorized`, `denied`, `permission`, `forbidden` are banned by test. One `MeshError`, two
renderers: MCP encodes the identical object.

### MCP protocol

JSON-RPC 2.0 over stdio, newline-delimited. Methods: `initialize` (protocolVersion
`2025-06-18`, `capabilities.tools`, `serverInfo`, `instructions`), `notifications/initialized`,
`ping`, `tools/list`, `tools/call`. An unknown method returns `-32601`. Server name `mesh`;
instructions are built once at start-up from an optional config — a load failure is never fatal
and surfaces per call as `kind: config_missing`.

Tool results are `content: [{type: "text", text: <JSON>}]` plus `structuredContent` = the object,
or `{"result": [...]}` for list-returning tools. Failures are `isError: true` with the JSON error
envelope as the text. **Every** tool carries an `annotations` object with `readOnlyHint`,
`idempotentHint` and `destructiveHint` set explicitly; `destructiveHint: true` is
`mesh_task_cancel` alone.

37 tools: the 21 legacy ones verbatim (names, parameters, descriptions, return shapes) plus
memory (7), scratch (4), asset (3) and task-graph (3, incl. `task_next`). Extended optional
params default to null/false so an old client's call is unchanged: `mesh_task_list` gains
`ready`/`blocked`; `mesh_task_claim` gains `strict`; `mesh_search` gains `spaces`, `kind`,
`engine`; `mesh_session_start` gains `budget`, `no_memories`. Withheld: every delete/remove/
clear/forget verb, `asset_add` (it reads an arbitrary filesystem path — a human act), `asset_gc`,
and every admin verb; no registered tool name may contain `delete`, `daemon`, `reindex` or
`status`. New descriptions are capped at 200 characters and the serialised `tools/list` response
is asserted ≤ 32 KB; the instructions block keeps its 2048-byte budget and its authorisation
denylist. `TAG_SPEC_SEMANTICS` stays byte-identical across the two update-tool schema
descriptions, the instructions block and the CLI `--tags` help.

---

## Implementation Detail

**Dispatch.** `main.rs` parses the complete clap tree, builds `Ctx`, calls one verb function, and
maps `MeshError::code()` onto `ExitCode`. Because `cli/mod.rs` declares every subcommand and args
type up front and every verb file ships as a compiling stub, an implementation change is always
"replace the body of one owned file" — which is what makes parallel implementation
conflict-free.

**Flag placement.** `--json`/`--quiet` are declared on the root struct *and* on each non-admin
subcommand, merged by a hand-written `coalesce_flags`, because clap's `global = true` cannot be
redeclared per subcommand. Booleans OR; owner is local-wins-else-global, except `task update` and
`memory update`, whose reassignment `--owner` deliberately does not fold in the global one.

**No-args behaviour.** Each sub-app takes `Option<Sub>`; `None` prints that command's long help
to **stdout** and exits 2, overriding clap's stderr default, so a wrapper that captures stdout to
detect help keeps working.

**Output classes.** M (mutations): `--quiet` beats `--json`; JSON is `{"id", <fields>, "updated"}`.
L (listings, lenses, objects, admin): `--json` beats `--quiet`. S (search and recall): output is
always one JSON array; `--json` is inert; `--quiet` suppresses only the stderr notice. Every
command carries an explicit class.

**Preview and snippet** are a plain 200-**code-point** slice, no ellipsis, no word boundary —
code points, because a byte slice panics on a multibyte boundary.

**Delete guard.** Force deletes. Without force: a machine path or no tty exits 2 removing
nothing; a tty prompts and anything but `y` exits 1.

**Watcher.** Foreground, blocking, single-threaded. Singleton via an `O_EXCL` lock at
`$XDG_RUNTIME_DIR/mesh-<sha256(vault)[..12]>.watch.lock` (else `~/.mesh/run/`) holding the pid,
with the entity-lock staleness and CAS rules; a second watcher exits 4. Debounce is
`notify-debouncer-full` at 250 ms per path, which collapses an editor's write-then-rename and
mesh's own temp-write-then-rename into one `indexed index update`. Reconcile moves a file whose
frontmatter disagrees with its folder using a byte-preserving rename — no reserialisation, so
`updated` does not move and unknown keys round-trip — taking the entity lock non-blocking and
leaving a contended entity for a later event. Every guard is reproduced in order (non-`.md`,
unreadable, not a mesh id, unknown type/status, sandbox escape, `src == dest` returns the
caller's own path and never a realpath, source raced away), and every `io::Error` is swallowed by
design: one escaping error would kill the loop and freeze freshness for its whole lifetime.

**The `daemon` shim** is hidden, ~40 lines, never spawns, always exits 0, preserves every key,
string and idempotence rule, reports the watch lock path as its socket, and prints one advisory.

**Testing.** Unit tests are colocated at the bottom of each module (densest on ids, tags, select,
wikilinks, deps and locks; the two former property suites become plain unit tests — no
`proptest`). Integration tests are one file per verb family, each building its own temp vault
through `VaultFixture` and driving the real binary with `--config`, so no test mutates
process-global environment and the suite runs at default parallelism (`serial_test` is needed
only by the two umask tests). `--help` is asserted with `predicates::str::contains` on the parts
that matter — subcommand order and key flags — not snapshots. Every exit-code row also asserts
the output does **not** contain a panic. `tests/race.rs` runs real processes: 8 racing claims →
exactly one 0 and seven 4; concurrent appends lose nothing; a stale lock is reclaimed exactly
once; the release CAS never deletes an unrelated file at the lock path.

**The compat corpus** (`tests/fixtures/python-vault/`, written by the Python binary before it is
deleted and then byte-frozen) carries all five note types, tasks in every status with populated
edges, unknown keys including one named `extra`, a bare-date `created`, a naive datetime, a
non-UTC offset, unicode titles, a quoted date-like string, a legacy free-form priority, empty
metadata, a corrupt file, a foreign file and a stale lock. `tests/compat_corpus.rs` asserts
**semantic** fidelity, not bytes: every file parses; every typed field has the expected value;
unknown keys survive load → dump → load unchanged; a no-op read command never rewrites a file;
an append changes only `updated`, `related` and the appended block.

**CI.** `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked -D warnings`,
`cargo test --locked` on Linux and macOS; `cargo llvm-cov --fail-under-lines 80` (ratchets up,
never down); a release-build package job that runs `--version`, `--help`, `completions bash`,
asserts the shim binary exists (it blocks on stdin — never `--help` it), drives one
`initialize` frame through `mesh mcp`, and runs `scripts/smoke.sh`; `cargo deny check`.

---

<!-- merge -->
## Stack

| Layer | Choice |
|---|---|
| Runtime | Rust 1.94, edition 2021, single binary, no async runtime |
| CLI | `clap` 4 (derive) + `clap_complete` |
| Data | Markdown + `yaml-rust2` (read) + a hand-rolled canonical emitter (write); `serde`/`serde_json` with `preserve_order`; `indexmap` |
| Config | `toml` (read) + `toml_edit` (format-preserving `config set`) |
| Time / hashing / syscalls | `chrono` (std+clock only), `sha2`, `rustix` (O_EXCL, flock, fstat, kill, umask) |
| Walking / watching | `walkdir`, `notify` + `notify-debouncer-full` |
| Agents | Hand-rolled JSON-RPC 2.0 over stdio (no MCP SDK, no async runtime) |
| Search engine | `indexed` (first-party hybrid; mesh wraps its CLI by subprocess) |
| Dev | `assert_cmd`, `predicates`, `tempfile`, `serial_test`; `cargo llvm-cov`, `cargo deny` |

Rejected on purpose: `regex` (five hand-written scanners, one needing a negative lookahead),
`anyhow` (exit codes are a typed contract), any MCP SDK (async runtime plus loss of byte control
over schemas), `mime_guess`, `insta`, `proptest`.
<!-- /merge -->

<!-- merge -->
## Layout

```
src/
├── main.rs, lib.rs, ctx.rs      # parse -> dispatch -> exit code; module surface; invocation context
├── bin/mesh-mcp.rs              # shim binary for the plugin bundle
├── error.rs config.rs spaces.rs ids.rs timefmt.rs text.rs render.rs
├── fm/                          # frontmatter: value, load, canonical emit, doc
├── storage/                     # atomic write, O_EXCL locks, sandbox, THE walk
├── model/                       # per-space typed views + FieldOrder (note, task, memory, scratch, asset)
├── domain/                      # verbs + select/tags/owner/wikilinks/deps/activity/context/lenses
├── search/                      # route, corpus, tokenize, builtin, tagpull, indexed, health
├── cli/                         # one file per verb family + globals, out, admin, watch
└── mcp/                         # stdio JSON-RPC server, schemas, 37-tool table, instructions
tests/                           # one integration file per verb family + compat corpus, race, bundle
```
<!-- /merge -->

<!-- merge -->
## Invariants that changed with the rewrite

1. **No daemon.** Every command reads disk directly. `mesh watch` is an optional foreground
   accelerator for search freshness and folder reconciliation only; behaviour is identical
   whether or not it runs. The socket, the warm index and the client-fallback matrix are deleted.
2. **Spaces.** The vault is five configurable spaces (notes, tasks, memories, scratch, assets),
   each a folder relative to the vault root, an absolute folder, the vault root itself, or
   disabled. The sandbox is the union of the enabled roots; type/status routing is relative to a
   space root, never the vault root; folders are created lazily on first write. An omitted
   `[spaces]` table reproduces the pre-rewrite layout exactly.
3. **Derived state is never stored.** Task readiness is computed at read time from the union of
   both edge directions. No verb ever writes another entity's file as part of its own
   transaction; the unblock cascade is a report, and mirrors are single-lock, one at a time,
   best-effort.
4. **Canonical frontmatter.** Mesh reads everything the Python era wrote and writes its own
   canonical form: model-declaration key order (not alphabetical), RFC 3339 `T…Z` timestamps for
   values it sets, unmodified scalars re-emitted from preserved raw text, no anchors, no line
   folding, one trailing newline. Unknown keys round-trip in place. Compatibility is semantic,
   not byte-level; machine JSON likewise pins key *order*, not whitespace.
5. **One walk, one skip set.** Every scan in the tree goes through a single walk that skips
   dot-prefixed path components, nested space roots, files over 4 MiB and non-UTF-8 files, and
   through a single safe reader that yields nothing rather than failing.
6. **Identity is validated across every space.** The roster check lives at the one core write
   boundary and applies to notes, tasks, memories, assets and the scratch namespace. It is a
   spelling check, never an authorisation boundary; every identity that becomes part of a path is
   normalised first.
7. **No panics on user input.** `unwrap`/`expect`/`panic` are lint-denied in the library and
   `main` catches any that escape, printing one line instead of a trace.
<!-- /merge -->

---

## Deviations from the Python contract

Twenty-four, and nothing else changes.

1. The `recent-activity: daemon down, scanning the folder directly` notice is **removed** —
   the direct scan is now the only path, so the notice would fire always and say nothing.
2. `search --health`'s gate ladder drops `daemon down`; `daemon_up` keeps its key position and
   reports **watcher** liveness. The three surviving reason strings are unchanged.
3. `mesh daemon start|stop|status` becomes a hidden, non-spawning shim: all keys, strings and
   idempotence rules preserved, `socket` holds the watch lock path, always exit 0.
4. `--help` rendering is clap's, not rich's — no box drawing. Content, ordering, defaults and
   required markers are preserved; bracketed strings now print literally.
5. `--blocks`/`--blocked-by` help drops `(inert v1)`, and exit code 5 becomes reachable.
6. Unknown config tables and keys are ignored rather than rejected.
7. The id-form pattern widens from `^[nt]-…` to `^[nmta]-…`, so a body containing `[[m-x]]` now
   lands in `related`.
8. Under machine output, errors become one JSON object on stderr. Human output, stdout and every
   exit code are unchanged.
9. `task get` gains a human `ready:` line and a `ready` key appended last in JSON.
10. `note update` gains `--title`, paired with the dangling-backlink advisory.
11. The default built-in engine is BM25-lite with the four pinned tiers as **floors**;
    `--engine substring` restores legacy scoring and head-of-body snippets; an explicit
    `[search].threshold` on a non-substring engine emits one advisory.
12. `indexed` invocations gain a 30 s wall clock; a timeout degrades and is never an error.
13. Every walk skips dot-prefixed components, nested space roots, files over 4 MiB and non-UTF-8
    files.
14. `mesh status` caps `dangling_links` at 50, appends `dangling_links_total`, and appends the
    memories, scratch, assets, deps, spaces and watcher groups after `agents`.
15. `session_mentions` tie order is fixed to `(updated desc, id asc)` instead of hash order.
16. `session-start` gains `reason: "memory"` entries (capped at 5), `--no-memories` and
    `--budget`, whose truncation is signalled by one final synthetic `truncated` entry. The
    shipped hook command is unchanged.
17. `mesh --version` prints `0.2.0`.
18. `mesh` and each sub-app with no arguments print help to **stdout** and exit 2, overriding
    clap's stderr default.
19. The MCP surface grows from 21 to 37 tools and the playbook's `allowed-tools` from 20 to 36;
    `destructiveHint` remains exactly `{mesh_task_cancel}`; the bundle and hook JSONs are
    byte-unchanged.
20. Frontmatter keys are written in **model declaration order**, not alphabetically — readable
    for humans and for Obsidian, and identical to the machine-JSON order.
21. Timestamps mesh writes are RFC 3339 `YYYY-MM-DDTHH:MM:SS[.ffffff]Z`, not PyYAML's
    space-separated `+00:00` form, and the same string is the id digest input. There is no
    80-column line folding, and quoting is double-quoted with JSON escapes rather than
    single-quoted. Existing ids are never recomputed, so cross-implementation id reproducibility
    is not a requirement.
22. A written document ends with exactly one trailing newline (the Python era stripped it).
23. Machine JSON uses compact separators rather than Python's `", "` / `": "`. Key order remains
    a contract; whitespace does not.
24. Every MCP tool carries an explicit `annotations` object with all three hints set; the legacy
    "annotations absent entirely" quirk and the `x-fastmcp-wrap-result` schema key are dropped.
    List-returning tools still wrap as `{"result": [...]}`.

---

## Performance Budget

Cold start under 10 ms for a read command on a warm filesystem, asserted by a wall-clock test
that also proves the MCP tool table is never constructed off the MCP path. A full-vault scan of
~10k files is gated by an ignored benchmark, not a CI assertion. `tools/list` serialises to
≤ 32 KB; the MCP instructions block to ≤ 2048 bytes.

---

## Open Questions

1. **Pinned dependency versions.** If a pinned version does not resolve on crates.io at
   implementation time, take the latest compatible one and record the substitution in the
   foundation unit's commit rather than blocking.
2. **Coverage floor.** 80% lines is the initial gate against the Python suite's 97% statements;
   the gate ratchets up as the suite fills in, and never down.
