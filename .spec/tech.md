---
type: entrypoint
scope: technical
children:
  - plan.md
updated: 2026-09-26
---

# Mesh — Technical Architecture

One Rust binary over one Markdown folder. Parse, dispatch into pure domain functions that read
and write the folder directly, map one error enum onto a fixed exit code, exit. No daemon, no
database, no async runtime; ranking is delegated to `indexed` when it is configured and present.
CLI and MCP are two thin renderers over the same domain.

---

## Stack

| Layer | Choice |
|---|---|
| Runtime | Rust 1.94, edition 2021, single binary, no async runtime |
| CLI | `clap` 4 (derive) + `clap_complete` |
| Data | Markdown + `yaml-rust2` (read) + a hand-rolled canonical emitter (write); `serde_json` with `preserve_order`; `indexmap` |
| Config | `toml` (read) + `toml_edit` (format-preserving edits) |
| Time / hashing / syscalls | `chrono`, `sha2`, `rustix` (O_EXCL, flock, fstat, kill, umask) |
| Walking / watching | `walkdir`, `notify` + `notify-debouncer-full` |
| Terminal UI | `ratatui` + `crossterm` — the dashboard only, feature-trimmed to the crossterm backend, imported only by `src/cli/dashboard.rs` |
| Agents | Hand-rolled JSON-RPC 2.0 over stdio (no MCP SDK) |
| Search engine | `indexed` (first-party hybrid; mesh wraps its CLI) |
| Dev | `assert_cmd`, `predicates`, `tempfile`, `serial_test`; `cargo llvm-cov`, `cargo deny` |

Rejected on purpose: `regex` (five hand-written scanners, one needing a negative lookahead),
`anyhow` (exit codes are a typed contract), any MCP SDK (it would pull an async runtime and cost
byte control over the tool schemas), `mime_guess`, `insta`, `proptest`. Mesh code stays small —
wrapper, locks, walk, wikilinks.

**Vault requirement.** Mesh needs only a directory it can write into — no notes application need
be installed, running, or detected; `mesh init` creates the folder when none exists. Obsidian is
the maintainer's reference pairing, not a dependency: the notes space can *be* an Obsidian vault,
and nothing checks for one.

---

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
├── cli/                         # one file per verb family + globals, out, admin, watch, dashboard
└── mcp/                         # stdio JSON-RPC server, schemas, 40-tool table, instructions
tests/                           # one per verb family + compat corpus, race, bundle, review regressions
```

---

## Invariants

1. **No daemon.** Every command reads disk directly and behaves identically whether or not any
   watcher runs. `mesh watch` is an optional foreground accelerator for search freshness and
   folder reconciliation only.
2. **Spaces.** The vault is five configurable spaces — notes, tasks, memories, scratch, assets —
   each a folder relative to the vault root, an absolute folder, the vault root itself, or
   disabled. The sandbox is the union of the enabled roots; type/status routing is relative to a
   space root, never the vault root; folders are created lazily on first write. An omitted
   `[spaces]` table reproduces the pre-rewrite layout exactly.
3. **Derived state is never stored.** Task readiness is computed at read time from the union of
   both edge directions. No verb writes another entity's file as part of its own transaction: the
   unblock cascade is a report, and `blocks` mirrors are single-lock, one at a time, best-effort.
4. **Canonical frontmatter.** Mesh reads everything the Python era wrote and writes its own
   canonical form: model-declaration key order, RFC 3339 `T…Z` timestamps for values it sets,
   unmodified scalars re-emitted from preserved raw text, no anchors, no line folding, one
   trailing newline. Unknown keys round-trip in place. Compatibility is semantic, not byte-level;
   machine JSON pins key *order*, not whitespace.
5. **One walk, one skip set.** Every scan goes through a single walk that skips dot-prefixed path
   components, nested space roots, files over 4 MiB and non-UTF-8 files, and through a single safe
   reader that yields nothing rather than failing. The **writer is bounded by the same constant**:
   a document that would exceed it is refused (exit 2) rather than written into permanent
   unaddressability.
6. **Writes are atomic and single-entity.** Temp file plus rename, preserving an existing file's
   mode; every mutation holds that entity's lock and re-resolves its target inside it; lock
   removal is always a compare-and-swap on `(dev, ino)`, never a blind unlink. Whatever the write
   commits — an edge list, a deletion predicate, a reference guard — is **recomputed inside the
   lock**: the edit is replayed against the value the locked read returns, never a result derived
   from an earlier unlocked scan. A scan may still feed a decision that needs the whole graph
   (the cycle check), never the list that gets written.
7. **Identity is validated across every space** at one core write boundary — notes, tasks,
   memories, assets and the scratch namespace. A spelling check, never authorisation; every
   identity that becomes part of a path is normalised first **and rejected when the normal form
   is empty** (exit 2), so no identity can collapse the layout it is a component of. An empty
   `--owner` reads as absent everywhere rather than landing on disk as an identity no roster,
   filter or `--mine` can match.
8. **No panics on user input.** `unwrap`/`expect`/`panic` are lint-denied in the library; `main`
   catches anything that escapes and prints one line instead of a trace.
9. **Agent content is inert data**, never instructions or shell input.
10. **Mesh owns the interface, not the vault** — versioning, sync and backup are the vault
    owner's job. That is the basis for hard delete (no trash, no promised recovery) and for
    skipping and round-tripping any file or key mesh did not write.
11. **A shared space is not mesh's to delete.** A space may resolve to a folder the operator also
    writes (`assets = "."` puts one at the vault root), so "nothing else names this file" is not
    an ownership test. A file is mesh's exactly when its **name is one mesh itself would have
    minted**, and that test is derived as the inverse of the namer so the two cannot drift; it
    must match *this* entity's id before any unlink. Frontmatter an agent or an editor can write
    (an asset's `blob`) names a file only through that test — passing the sandbox check answers
    "inside the vault", never "mine".

---

## Performance

**Goal:** instant CLI. **Target:** cold start under 10 ms for a read command on a warm
filesystem, asserted by a wall-clock test that also proves the MCP tool table is never
constructed off the MCP path. The pin asserts the **minimum of ten** warm end-to-end runs of
the suite's own binary stays under 50 ms — min-of-N is monotone against scheduling noise, so it
is stable under CI load while still lifting on a real regression — with ~6× headroom over the
measured ~7.5 ms idle floor. Heavy work does not exist: a full-vault scan of thousands of files
in Rust is milliseconds, which is what let the warm daemon be deleted rather than ported. The
dashboard's TUI crates are linked into the binary but imported only by
`src/cli/dashboard.rs`, and the same pin proves they tax no other verb's startup.

**The Rust rewrite decision was reversed (2026-09).** It was shelved when the trade was a ~2–10 ms
Rust floor against a ~150–180 ms Python floor for a three-verb CLI a human invoked occasionally.
What changed is the product shape: a granular multi-space surface (five verb families plus
lenses, search and MCP) called by agents in hot loops pays that floor on every call, and the
daemon that used to hide it became the thing most in the way. → § Stack, § Invariants

---

## Shared primitives (DRY)

Every space is note-shaped, so each cross-cutting mechanic has **one** implementation the verb
families share — never per-verb copies:

- **Frontmatter** — one ordered-map loader, one canonical emitter, one document reader/writer.
- **Safe read** — one reader that yields nothing on an I/O error, malformed YAML or non-UTF-8;
  every scanner routes through it.
- **Vault walk** — one iterator with one skip set; all scanners consume it.
- **Select** — one filter/sort/limit engine generic over a typed view; every list verb and the
  tag pull use it, so listings cannot drift.
- **Locks and atomic writes** — one lock module, one atomic-write function.
- **CLI output** — one output surface (mutation/rows/object emitters, notices, the delete guard,
  the error envelope); every verb renders through it.
- **Errors** — one enum whose `code()` is the exit status, mapped once in `main`.

---

## Contracts

| Contract | Rule |
|---|---|
| Config | `~/.mesh/config.toml`. `[core]` `vault_path` + `agent`; `[spaces]` notes/tasks/memories/scratch/assets (relative path, absolute path, `"."`, or `false`; every key optional); `[search]` collection, hybrid, threshold, engine, spaces; `[tasks]` collections, strict. `vault_path` is expanded then canonicalised at the parse boundary; `path` and `tolaria_path` are permanent input aliases. Unknown tables and keys are ignored. Precedence: `--config` > `$MESH_CONFIG_PATH` > default; `--vault` > `$MESH_VAULT` > file; `$MESH_AGENT` > file. Missing config → exit 2. `[search].threshold` applies **only when explicitly set**, on the `indexed` branch as well as the built-in one; every `--threshold` takes a finite number (`nan`/`inf` are exit 2, on `search`, `memory recall` and `init` alike). `config set` writes only keys a reader consumes and parses each as **that reader's own type** — string, boolean, finite float, string list (CSV or a TOML array), or a space path / `false`; an unknown key or a wrong type is exit 2 rather than a silent no-op. |
| IDs | `n-` / `t-` / `m-` / `a-` + Crockford base32 over `SHA-256(created_iso \0 title)`, 4+ chars, extended on collision. Asset ids digest the content instead, making the id the content address. Never sequential; existing ids are never recomputed. |
| Folders | Routing is relative to the **space root**. Notes: `note→<notes>/`, `log/decision/reference/project→<notes>/{logs,decisions,references,projects}/`, recursive. Tasks: `open\|claimed→<tasks>/open/`, `done\|cancelled→<tasks>/done/`, non-recursive. Memories: flat, never moved. Scratch: `<scratch>/<agent>/<name>.md`, both components rejected when they normalise to empty. Assets: blob plus sidecar sharing one stem; a file in the assets root is one of mesh's blobs **only when its name is one mesh would mint for that id** (§ Invariants 11), which is what `asset gc`, `asset remove` and `asset path` decide by — never the sidecar's `blob` key read as a bare path. |
| Atomic write | Temp file plus rename, mode-preserving, `fsync`ed; the destination is untouched on any failure before the rename. |
| Locks | `O_EXCL` per entity under the space's `.locks/`; stale when the PID is dead or older than 300 s; both reclaim and release are `(dev, ino)` compare-and-swaps under an exclusive `flock`. `mesh status` reports stale locks. |
| Sandbox | Every resolved path must equal or sit beneath one enabled space root. |
| Exit codes | 0 ok · 1 io/infrastructure or declined confirmation · 2 validation · 3 not found (incl. corrupt frontmatter on read/amend) · 4 claim conflict or contended lock · 5 blocked. Codes live on the error enum; `main` maps them once. |
| Error envelope | Under `--json`, one JSON object on stderr: `kind`, `message`, `next_action`, the structured fields, plus `candidates` on not-found and `retry_after_ms` on a lock conflict. MCP renders the identical object. A verb that spans several transactions names **what already committed** and the exact command that repairs it; that wrapper changes the message only — `kind`, `code` and `candidates` read through to the wrapped error. |

### Note fields

`id`, `type` (any string on read; `note|log|decision|reference|project` on mesh writes), `title`,
`tags`, `owner`, `claimed_by` (absent when never claimed, `null` after release), `created`,
`updated`, `related` — the shared base block for every space, in declaration order. Mesh-native
is the frontmatter id, never the stem: an adopted file keeps its foreign filename.

### Per-space additions

| Space | Adds |
|---|---|
| tasks | `status` (open|claimed|done|cancelled), `priority`, `claimed_by`, `project`, `blocks`, `blocked_by` — readiness derived from both directions |
| memories | `kind`, `scope`, `project` (optional raw string mirroring the task field — workstream-envelope membership; the shared/private `scope` axis is untouched), `importance`, `source`, `expires`, `superseded_by` |
| scratch | `type`, `name`, `agent`, `tags`, `created`, `updated` — name-addressed, no id |
| assets | `filename`, `media_type`, `bytes`, `sha256`, `blob` on the sidecar; the blob is written first |

**Appends:** finish → `## Outcome` + timestamp; cancel → `## Cancelled` + timestamp.

---

## Build order

`foundation → note → task+graph → memory → scratch → asset → search → lenses → mcp → admin/watch
→ verify` — the order the rewrite was built in, and the order to re-derive the tree in if it ever
has to be rebuilt. Phases 1–3 are all delivered; the live sequence is in [plan.md](plan.md).

---

## Implemented surfaces

Contracts compounded from the (now-deleted) feature specs. Full detail lives in the code plus the
tests cited.

- **Adoption & note claims** — `note adopt` mints a mesh id into an existing foreign file in
  place: insert-only-absent keys (`id`, `title`, `created`, `updated`; `owner` additionally only
  when the flag is given), foreign keys and the filename untouched, byte-identical re-run. A
  batch is a sequence of single-entity transactions: the first failure stops the run naming what
  committed; the idempotent re-run heals. `note claim`/`release` are the task machinery minus the
  lifecycle: test-and-set on `claimed_by` only, conflict = the shared claim-conflict envelope
  (same shape, message names its own entity), `--force` on release breaks a foreign holder.
  `--mine` on note list is owner-or-claimed_by. The status census carries per-agent
  `notes_owned`/`notes_claimed`, JSON keys appended last. Pinned by `tests/note_cli.rs`,
  `tests/race.rs` (8-way real-process claim race), `tests/review_regressions.rs`, and the MCP
  parity tests (`src/mcp/`, `tests/mcp_cli.rs`, `tests/bundle.rs`).

- **Project envelope** — one membership rule per space, all read-time: tasks by `project`
  equality, notes by `related` containment (the stored, wikilink-backfilled list), memories by
  `project` equality over the optional field. The `project` lens appends the `notes` then
  `memories` sections last (append contract, pinned key order); `search --project` and
  `memory recall --project` scope to the members — recall as an eligibility filter before the
  unchanged ranking rules, search as an active filter on both engine branches (built-in: after
  scoring, before `--limit`; `indexed`: unbounded fetch, post-filter, display cap, the tag pull
  included) — and both compose with every other filter as a plain conjunction; a zero-row
  conjunction is an empty result, never an error. Membership reads the envelope spaces the vault
  enables, so `--space` narrows the corpus, never the envelope. MCP carries the `project` param
  on `mesh_search`, `mesh_memory_recall` and the memory write tools with CLI-identical
  semantics; the tool count stays 40. The seed gate is unchanged and shared by lens, search and
  recall: an unknown id → exit 3 with candidates; a foreign seed → `seed is not mesh-native`.
  Unknown and dangling `project` values stay tolerated (zero-member envelopes) — a read-time
  join, never a validated reference. Pinned by `tests/{lens,search,memory,mcp}_cli.rs`.

- **Wikilinks** — `[[Title]]` → id by title match against the notes index; `[[n-id]]`/`[[t-id]]`/
  `[[m-id]]`/`[[a-id]]` pass through; alias and anchor forms (`|`, `#`, `^`) strip at the lookup
  boundary; `related` is deduped; unresolvable links are dangling and counted by `mesh status`.
- **Search** — hit shape `{id,type,title,score,path}` plus conditional `tags`/`owner`/`updated`/
  `snippet`/`space`; foreign files surface with `id: null`. Routing: `indexed` when hybrid is on,
  a collection is configured and the binary is on PATH; otherwise a built-in BM25-lite engine
  whose four legacy tiers (title-exact 1.0, title-substring 0.8, tag 0.6, body 0.4) remain
  reachable as floors; `--engine substring` restores the legacy scoring exactly. Ordering is
  score desc, updated desc, path asc, with the ±0.02 recency-tiebreak band kept only on the
  `indexed` path, and the path arm is what breaks a score-and-`updated` tie on both engines.
  `--tags` and `--status` speak the CSV mesh itself writes — `--tags` is repeatable *and*
  comma-split and ANDed, `--status` is a membership union whose unknown value is exit 2, the same
  rule `task list` obeys. Under an active filter the `indexed` fetch is unbounded and `--limit` is
  a display cap applied after filtering, so a filtered page is never short of rows that were
  simply never fetched. `--project` scopes the corpus to one project's envelope members
  (→ Project envelope): the seed resolves through the shared gate before any engine I/O, and the
  member filter composes with every other filter as a plain conjunction.
- **Tasks** — atomic `O_EXCL` claim; idempotent release/finish/cancel that never rewrite a
  no-op and that report the status they *found*, not the one asked for; `--available` unchanged
  and dependency-blind; `--ready`/`--blocked` are the dependency-aware filters; a strict claim on
  a blocked task exits 5; `task next` selects and optionally claims in one invocation, retrying
  across candidates on a race. `block`/`unblock` carry the add and remove sets into the task's
  own lock and replay them against the locked read (§ Invariants 6), so concurrent edits compose;
  the cycle check keeps using the whole-graph scan, and a retraction onto a task that does not
  exist has nothing to retract and warns about nothing.
- **Assets** — ingest is copy-only and idempotent by content, and `--attach` resolves its target
  **before** any I/O, so a refused target writes nothing. `attach`/`detach` span three and two
  transactions; both are idempotent, so re-running one heals every partial state, and a failure
  names which halves committed. `remove`'s reference guard runs inside the asset's lock.
- **Scratch and memories** — the scratch agent is validated exactly like the name beside it, so
  no identity can collapse `<scratch>/<agent>/<name>.md` to a shared file and lock; the agent
  census counts only a real `<agent>/<name>.md` pair. `memory forget --expired` re-reads each
  memory under its own lock and keeps whatever is no longer expired, so a renewal that reported
  success is never contradicted; one unreadable memory is skipped, not fatal.
- **Admin and watch** — `mesh reindex` and `mesh watch` report the rebuild that actually ran
  rather than a constant, so a degradation reaches the operator and `watch --json` never claims
  an index update it did not make. Reconciliation *moves* a file and never destroys one: an
  occupied destination leaves the source in place. `status.deps.cycles` reports **strongly
  connected components**, not one entry per DFS back edge — while a component is listed the
  graph is still cyclic, and it disappears only when it is really gone. `status` names the
  notes-corpus split explicitly: the human block labels the count `notes: N (mesh-native)` and
  adds a `foreign markdown` line when foreign files exist, and the payload **appends** a
  `notes_foreign` count — never mid-payload, per the append contract — so `notes: 0` beside a
  vault full of adopted files cannot read as blindness.

- **Dashboard** — `mesh dashboard [--interval]` (default 2 s): a foreground, read-only, human-only
  terminal view of the vault — one screen, four fixed panes (agents census incl. note
  ownership/claims, tasks by ready/blocked/claimed, recent activity, vault health). Every number
  comes from the same domain reads the CLI uses (`status_report`, `tasks::list`, the
  recent-activity lens, the search-health line) — the dashboard adds no second source of truth.
  Every refresh is a direct read (identical with no watcher); a failed refresh keeps the last
  good frame with a dim status line, never a crash. Terminal restoration is a Drop guard, so
  quit, error and the panic-catch all restore cooked mode, cursor and colors; no tty → exit 2
  `dashboard needs a terminal`. Keys are minimal and read-only: `q`/Ctrl-C quit, `r` refresh, `m`
  mine-only (seeds from the global `--mine`), `Tab` pane focus, arrows scroll the focused pane.
  Not exposed over MCP (asserted). No lock, no signal handler: two dashboards are two harmless
  readers. Pinned by `src/cli/dashboard.rs` unit tests (headless snapshot composition), the
  `Session`/`FrameSource` seams, `tests/dashboard_cli.rs`, and the cold-start pin in
  `tests/foundation_cli.rs` → § Performance.
- **MCP** — stdio JSON-RPC, 40 `mesh_*` tools mirroring the safe verbs plus the read-only
  lenses, each carrying explicit read-only/idempotent/destructive hints with exactly one
  destructive tool (`mesh_task_cancel`). Withheld: every removal verb, asset ingest and gc, and
  all admin. Every parameter carries a description; enums render domain literals; a config-derived
  instructions block is sent on connect and degrades to naming `mesh init`. Failures cross as
  the structured error envelope, never a trace.
- **Session lenses** — `recent-activity`, `build-context`, `graph` (`--direction in|out|both`,
  inbound index inverted at read time), `project` (the workstream envelope — the `type: project`
  note plus the notes, tasks and memories that belong to it; → Project envelope), and `session-start` (tasks → mentions → memories → activity, deduped by id,
  a `reason` on every entry, `--team` widening only the activity half, `--budget` trimming bodies
  before entries and recording the drop). All are read-only and accept a space filter. `--mine`
  resolves against the **acting** identity — `--owner` when given, else `[core].agent` — on every
  lens and list verb, the same identity a claim would be written as. A lens narrows its corpus
  when a space is disabled; any other listing failure is still an error, never an empty result.
  A seed that names a foreign file — one `search` can see but no lens can address — fails as
  `seed is not mesh-native (no mesh id)` (exit 3, envelope `not_found`), never confused with
  `seed not found`.

---

## Risks

| Risk | Mitigation |
|---|---|
| YAML compatibility with Python-era vaults | The read side accepts every form PyYAML wrote (sorted keys, space-separated timestamps, bare dates, naive and offset datetimes, quoted scalars, anchors, unknown keys); a byte-frozen Python-written corpus gates the foundation unit on semantic round-trip before any verb work starts → `tests/compat_corpus.rs` |
| Lock-semantics parity in a new language | The staleness table, the `PermissionError`-means-alive rule and both compare-and-swaps are ported rule-for-rule and pinned by real multi-process race tests (8-way claim, concurrent appends, stale reclaim, release CAS) → `tests/race.rs` |
| `indexed` contract drift | One wrapper, byte-identical argv (search/update/create against the shipped `indexed` CLI; machine output is requested through the child's `INDEXED_SIMPLE_OUTPUT` env, never an argv flag), a tolerant decode of its single `--simple-output` envelope (v2 `relevance` used as-is, v1 squared-L2 normalised via `1/(1+s)`, chunks deduped to their document's best), and a stub engine fixture that pins argv and every decode-tolerance rule. `mesh reindex` routes **update-when-exists** — `index update <collection>` when the collection exists, `index create files --path <root> --collection <C>` only when it does not, because create prompts to overwrite an existing collection and mesh never answers a prompt. The ingest path (create/update) runs on a **600 s** wall clock while search keeps its 30 s; `MESH_INDEXED_TIMEOUT_MS` overrides both, and every clock degrades instead of failing. `search --health` reports the branch actually taken |
| A hostile or huge file in a vault-root notes space | One walk, one skip set: dot components, nested space roots, files over 4 MiB, non-UTF-8; the safe reader yields nothing rather than failing |
| Windows | Locks and the watcher are POSIX-shaped; POSIX-first, Windows best-effort |
| Untrusted content | Multi-root sandbox on every resolved path; agent content is never shell input |
| Unbounded memory growth | Nothing prunes memories by design; expiry and supersession control visibility, and a retention lens can be added later without taking a deletion policy back |
