---
type: entrypoint
scope: technical
children:
  - plan.md
  - features/rust-rewrite/tech.md
updated: 2026-09-05
---

# Mesh — Technical Architecture

One Rust binary over one Markdown folder. Parse, dispatch into pure domain functions that read
and write the folder directly, map one error enum onto a fixed exit code, exit. No daemon, no
database, no async runtime; ranking is delegated to `indexed` when it is configured and present.
CLI and MCP are two thin renderers over the same domain.

Feature detail for the in-flight rewrite: [features/rust-rewrite/tech.md](features/rust-rewrite/tech.md).

---

## Stack

| Layer | Choice |
|---|---|
| Runtime | Rust 1.94, edition 2021, single binary, no async runtime |
| CLI | `clap` 4 (derive) + `clap_complete` |
| Data | Markdown + `yaml-rust2` (read) + a hand-rolled canonical emitter (write); `serde_json` with `preserve_order` |
| Config | `toml` (read) + `toml_edit` (format-preserving edits) |
| Time / hashing / syscalls | `chrono`, `sha2`, `rustix` (O_EXCL, flock, fstat, kill, umask) |
| Walking / watching | `walkdir`, `notify` |
| Agents | Hand-rolled JSON-RPC 2.0 over stdio (no MCP SDK) |
| Search engine | `indexed` (first-party hybrid; mesh wraps its CLI) |
| Dev | `assert_cmd`, `predicates`, `tempfile`, `serial_test` |

Rejected on purpose: `regex`, `anyhow`, any MCP SDK, `mime_guess`. Mesh code stays small —
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
├── model/                       # per-space typed views + field order (note, task, memory, scratch, asset)
├── domain/                      # verbs + select/tags/owner/wikilinks/deps/activity/context/lenses
├── search/                      # route, corpus, tokenize, builtin, tagpull, indexed, health
├── cli/                         # one file per verb family + globals, out, admin, watch
└── mcp/                         # stdio JSON-RPC server, schemas, 37-tool table, instructions
tests/                           # one integration file per verb family + compat corpus, race, bundle
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
   reader that yields nothing rather than failing.
6. **Writes are atomic and single-entity.** Temp file plus rename, preserving an existing file's
   mode; every mutation holds that entity's lock and re-resolves its target inside it; lock
   removal is always a compare-and-swap on `(dev, ino)`, never a blind unlink.
7. **Identity is validated across every space** at one core write boundary — notes, tasks,
   memories, assets and the scratch namespace. A spelling check, never authorisation; every
   identity that becomes part of a path is normalised first.
8. **No panics on user input.** `unwrap`/`expect`/`panic` are lint-denied in the library; `main`
   catches anything that escapes and prints one line instead of a trace.
9. **Agent content is inert data**, never instructions or shell input.
10. **Mesh owns the interface, not the vault** — versioning, sync and backup are the vault
    owner's job. That is the basis for hard delete (no trash, no promised recovery) and for
    skipping and round-tripping any file or key mesh did not write.

---

## Performance

**Goal:** instant CLI. **Target:** cold start under 10 ms for a read command on a warm
filesystem, asserted by a wall-clock test that also proves the MCP tool table is never
constructed off the MCP path. Heavy work does not exist: a full-vault scan of thousands of files
in Rust is milliseconds, which is what let the warm daemon be deleted rather than ported.

**The Rust rewrite decision was reversed (2026-09).** It was shelved when the trade was a ~2–10 ms
Rust floor against a ~150–180 ms Python floor for a three-verb CLI a human invoked occasionally.
What changed is the product shape: a granular multi-space surface (five verb families plus
lenses, search and MCP) called by agents in hot loops pays that floor on every call, and the
daemon that used to hide it became the thing most in the way. → [features/rust-rewrite/tech.md](features/rust-rewrite/tech.md)

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
| Config | `~/.mesh/config.toml`. `[core]` `vault_path` + `agent`; `[spaces]` notes/tasks/memories/scratch/assets (relative path, absolute path, `"."`, or `false`; every key optional); `[search]` collection, hybrid, threshold, engine, spaces; `[tasks]` collections, strict. `vault_path` is expanded then canonicalised at the parse boundary; `path` and `tolaria_path` are permanent input aliases. Unknown tables and keys are ignored. Precedence: `--config` > `$MESH_CONFIG_PATH` > default; `--vault` > `$MESH_VAULT` > file; `$MESH_AGENT` > file. Missing config → exit 2. `[search].threshold` applies **only when explicitly set**. |
| IDs | `n-` / `t-` / `m-` / `a-` + Crockford base32 over `SHA-256(created_iso \0 title)`, 4+ chars, extended on collision. Asset ids digest the content instead, making the id the content address. Never sequential; existing ids are never recomputed. |
| Folders | Routing is relative to the **space root**. Notes: `note→<notes>/`, `log/decision/reference/project→<notes>/{logs,decisions,references,projects}/`, recursive. Tasks: `open\|claimed→<tasks>/open/`, `done\|cancelled→<tasks>/done/`, non-recursive. Memories: flat, never moved. Scratch: `<scratch>/<agent>/<name>.md`. Assets: blob plus sidecar sharing one stem. |
| Atomic write | Temp file plus rename, mode-preserving, `fsync`ed; the destination is untouched on any failure before the rename. |
| Locks | `O_EXCL` per entity under the space's `.locks/`; stale when the PID is dead or older than 300 s; both reclaim and release are `(dev, ino)` compare-and-swaps under an exclusive `flock`. `mesh status` reports stale locks. |
| Sandbox | Every resolved path must equal or sit beneath one enabled space root. |
| Exit codes | 0 ok · 1 io/infrastructure or declined confirmation · 2 validation · 3 not found (incl. corrupt frontmatter on read/amend) · 4 claim conflict or contended lock · 5 blocked. Codes live on the error enum; `main` maps them once. |
| Error envelope | Under `--json`, one JSON object on stderr: `kind`, `message`, `next_action`, the structured fields, plus `candidates` on not-found and `retry_after_ms` on a lock conflict. MCP renders the identical object. |

### Note fields

`id`, `type` (note|log|decision|reference|project), `title`, `tags`, `owner`, `created`,
`updated`, `related` — the shared base block for every space, in declaration order.

### Per-space additions

| Space | Adds |
|---|---|
| tasks | `status` (open|claimed|done|cancelled), `priority`, `claimed_by`, `project`, `blocks`, `blocked_by` — readiness derived from both directions |
| memories | `kind`, `scope`, `importance`, `source`, `expires`, `superseded_by` |
| scratch | `type`, `name`, `agent`, `tags`, `created`, `updated` — name-addressed, no id |
| assets | `filename`, `media_type`, `bytes`, `sha256`, `blob` on the sidecar; the blob is written first |

**Appends:** finish → `## Outcome` + timestamp; cancel → `## Cancelled` + timestamp.

---

## Build order

`foundation → note → task+graph → memory → scratch → asset → search → lenses → mcp → admin/watch
→ verify` — the unit sequence in [features/rust-rewrite/plan.md](features/rust-rewrite/plan.md).
Phases 1–2 shipped in Python and are being re-delivered by the rewrite; Phase 3 (the dependency
graph) lands with it.

---

## Implemented surfaces

Contracts compounded from the (now-deleted) feature specs. Full detail lives in the code plus the
tests cited; the in-flight rewrite's own contracts are in
[features/rust-rewrite/tech.md](features/rust-rewrite/tech.md).

- **Wikilinks** — `[[Title]]` → id by title match against the notes index; `[[n-id]]`/`[[t-id]]`/
  `[[m-id]]`/`[[a-id]]` pass through; alias and anchor forms (`|`, `#`, `^`) strip at the lookup
  boundary; `related` is deduped; unresolvable links are dangling and counted by `mesh status`.
- **Search** — hit shape `{id,type,title,score,path}` plus conditional `tags`/`owner`/`updated`/
  `snippet`/`space`; foreign files surface with `id: null`. Routing: `indexed` when hybrid is on,
  a collection is configured and the binary is on PATH; otherwise a built-in BM25-lite engine
  whose four legacy tiers (title-exact 1.0, title-substring 0.8, tag 0.6, body 0.4) remain
  reachable as floors; `--engine substring` restores the legacy scoring exactly. Ordering is
  score desc, updated desc, path asc, with the ±0.02 recency-tiebreak band kept only on the
  `indexed` path.
- **Tasks** — atomic `O_EXCL` claim; idempotent release/finish/cancel that never rewrite a
  no-op; `--available` unchanged and dependency-blind; `--ready`/`--blocked` are the
  dependency-aware filters; a strict claim on a blocked task exits 5; `task next` selects and
  optionally claims in one invocation, retrying across candidates on a race.
- **MCP** — stdio JSON-RPC, 37 `mesh_*` tools mirroring the safe verbs plus the read-only
  lenses, each carrying explicit read-only/idempotent/destructive hints with exactly one
  destructive tool (`mesh_task_cancel`). Withheld: every removal verb, asset ingest and gc, and
  all admin. Every parameter carries a description; enums render domain literals; a config-derived
  instructions block is sent on connect and degrades to naming `mesh init`. Failures cross as
  the structured error envelope, never a trace.
- **Session lenses** — `recent-activity`, `build-context`, `graph` (`--direction in|out|both`,
  inbound index inverted at read time), `project` (a `type: project` note plus every task
  pointing at it), and `session-start` (tasks → mentions → memories → activity, deduped by id,
  a `reason` on every entry, `--team` widening only the activity half, `--budget` trimming bodies
  before entries and recording the drop). All are read-only and accept a space filter.

---

## Risks

| Risk | Mitigation |
|---|---|
| YAML compatibility with Python-era vaults | The read side accepts every form PyYAML wrote (sorted keys, space-separated timestamps, bare dates, naive and offset datetimes, quoted scalars, anchors, unknown keys); a byte-frozen Python-written corpus gates the foundation unit on semantic round-trip before any verb work starts → `tests/compat_corpus.rs` |
| Lock-semantics parity in a new language | The staleness table, the `PermissionError`-means-alive rule and both compare-and-swaps are ported rule-for-rule and pinned by real multi-process race tests (8-way claim, concurrent appends, stale reclaim, release CAS) → `tests/race.rs` |
| `indexed` contract drift | One wrapper, byte-identical argv, tolerant NDJSON decode, a 30 s wall clock that degrades instead of failing, and a stub engine fixture that pins argv and every decode-tolerance rule; `search --health` reports the branch actually taken |
| A hostile or huge file in a vault-root notes space | One walk, one skip set: dot components, nested space roots, files over 4 MiB, non-UTF-8; the safe reader yields nothing rather than failing |
| Windows | Locks and the watcher are POSIX-shaped; POSIX-first, Windows best-effort |
| Untrusted content | Multi-root sandbox on every resolved path; agent content is never shell input |
| Unbounded memory growth | Nothing prunes memories by design; expiry and supersession control visibility, and a retention lens can be added later without taking a deletion policy back |
