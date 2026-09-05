# Mesh Concepts

Answers to recurring architecture questions about mesh — a Rust CLI and MCP server for
multi-agent collaboration over one shared Markdown vault. Each section is researched against the
actual source and cites it; if the implementation changes, re-verify against the cited files
rather than trusting this doc blindly.

The one-paragraph version: `src/main.rs` parses, dispatches into pure domain functions that read
and write the folder directly, maps one error enum onto a fixed exit code, and exits. There is no
daemon, no database and no async runtime. `src/cli/` and `src/mcp/` are two thin renderers over
the same `src/domain/` functions.

---

## What is a "space", and why five of them?

A space is a named folder with its own verb family. `src/spaces.rs` defines exactly five —
`Notes`, `Tasks`, `Memories`, `Scratch`, `Assets` — and resolves each one from the optional
`[spaces]` table in the config (`src/config.rs::from_table`):

| Value | Resolution |
|---|---|
| key absent | the built-in default folder name under `vault_path` |
| `"subdir"` | relative to `vault_path` |
| `"/abs/path"` | used verbatim, `~`-expanded, added to the sandbox explicitly |
| `"."` or `""` | the vault root itself |
| `false` | disabled — every verb for it exits 2, and it contributes no sandbox root and no corpus |

Spaces are **configuration, not layout**. That is what lets the notes space *be* an existing
Markdown vault (`mesh init --obsidian` writes `notes = "."`) while tasks, memories, scratch and
assets sit inside it as strict subfolders. `Spaces::resolve` rejects two enabled spaces landing
on the same directory, requires every other root to be a strict descendant when notes is the
vault root (recording those roots as exclusions so the notes walk skips them), and refuses a
relative root that escapes `vault_path` — all three at load, all exit 2, all before any I/O.

The sandbox (`src/storage/sandbox.rs`) is the **union of the enabled roots**, not `vault_path`:
`vault_path` itself is only in the set when some space resolves to it. Space directories are
created lazily on first write, never at load, so pointing mesh at an existing folder scatters no
empty directories.

Five is a closed set. A sixth space needs a spec change — that is the "keep the surface honest"
thesis, restated for a granular surface.

## Where do memories live, versus notes, versus scratch?

All three are note-shaped Markdown files with the same frontmatter base block, the same lock and
atomic-write mechanics, and the same wikilink-derived `related`. What separates them is
*audience*, and mesh makes that a folder rather than a convention:

```
notes/            type: note        durable knowledge; the operator reads it
notes/logs/       type: log
notes/decisions/  type: decision
notes/references/ type: reference
notes/projects/   type: project
tasks/open/       status: open | claimed
tasks/done/       status: done | cancelled
memories/         type: memory      an agent's belief about the operator or the fleet
scratch/<agent>/  type: scratch     this session's working state
assets/           type: asset       a blob plus its sidecar
```

Routing is defined by `src/domain/notes.rs::note_folder` (notes) and the `open`/`done` split in
`src/domain/tasks.rs`; both are relative to the **space root**, never `vault_path`. Notes and
memories are walked recursively; tasks are `open/` then `done/`, each non-recursively, so a file
at `tasks/open/sub/t-x.md` is not a task.

Three deliberate choices are worth knowing:

- **Memories are flat.** `src/domain/memories.rs` never derives a folder from an identity, so a
  typo'd `$MESH_AGENT` cannot silently fork a namespace, and no memory verb ever moves a file.
  `scope` is a frontmatter filter, not a folder. Reads are recursive, so a human who files
  memories into subfolders keeps working.
- **Memories are not a memory subsystem.** There is no `use_count`, no `last_used` and no
  touch-on-read. A read verb that writes would break idempotence, contend locks, make
  `readOnlyHint` a lie on the MCP surface and dirty git on every hit. Recall ranks on
  `importance` and recency from `updated`, both of which the write path already maintains
  (`memories::recall_score`).
- **Scratch is excluded from every lens.** It carries frontmatter (six keys: `type`, `name`,
  `agent`, `tags`, `created`, `updated`) so one reader and one filter path serve the whole
  system, but it is out of the default search corpus, `recent-activity`, `graph`,
  `build-context` and `session-start`. It is a workbench; `search --space scratch` opts in.

"Agent memory versus human notes" is now a real split, where it used to be a convention. What is
*still* not checked anywhere is which space a given piece of writing belongs in — that guidance
lives in `.spec/design.md` § Which space wins, in the MCP instructions block and in the bundled
skill, and mesh decides nothing for you.

## How are assets stored, and why a sidecar?

`src/domain/assets.rs` writes two files sharing one stem:

```
assets/a-7Q3KDX9M.png     the blob, bytes verbatim
assets/a-7Q3KDX9M.md      the sidecar — an ordinary mesh entity
```

The id **is** the content address: `a-` plus Crockford base32 of `sha256(bytes)` (`src/ids.rs`),
extended one character at a time on collision against existing sidecar stems. Because blob and
sidecar share the stem, the universal "a mesh file is named `<id>.md`" invariant survives and
every stem-based resolver works unchanged — no bespoke stem→id map, no full sidecar scan per
`asset get`.

Consequences that follow from content addressing rather than from a policy:

- `asset add` is idempotent by content. Identical bytes return the stored id, write nothing, do
  not bump `updated`, and exit 0 with one stderr line.
- The **blob is written before the sidecar**, always. A crash after the blob leaves an orphan
  blob that `asset gc` finds; a sidecar-first write would leave a valid-looking asset pointing at
  nothing, which every read verb, search hit and graph node would surface. A failed sidecar write
  unlinks the blob on the way out.
- Ingest is **copy only**. `move` (which unlinks the operator's source) and `link` were evaluated
  and rejected as a data-loss footgun for a single saved copy.
- The source extension is kept only when it lowercases to `[a-z0-9]{1,12}`
  (`model::asset::blob_extension`); `media_type` comes from a static extension table in
  `src/model/asset.rs`, never a crate. The original filename lives in frontmatter as data, never
  as a path component, so a hostile filename cannot traverse.

`asset attach` appends `![[<blob>]]` to the target's body through the ordinary append path and
adds each id to the other's `related`, so the pair is visible to `graph --direction in` and
`build-context` with no bespoke edge type.

## Why is task readiness computed instead of stored?

Because storing it would mean one verb writing another entity's file. `src/domain/deps.rs` treats
`blocked_by` as authoritative and `blocks` as a best-effort mirror, and computes readiness at
read time from the **union of both edge directions** over the current `tasks/` scan.

That single decision buys:

- **No stale flag.** There is no `ready:` key on disk to disagree with the graph. `task get`
  computes it per call; `task list --ready` computes it once per listing.
- **No multi-file transaction.** `task finish` does not rewrite the tasks it unblocked; it
  *reports* them as `"unblocked": [...]`. Mirrors are applied one task at a time, each under its
  own lock, never held simultaneously, and a failure is a stderr warning rather than a rollback.
- **Hand edits work.** A one-sided edge someone typed into a Markdown file still contributes,
  because the union is what is read.

Cycles are detected on the post-edit graph and only when they run through an edge being added
(`deps::check_acyclic`), so a removal is never refused and a pre-existing hand-made cycle
elsewhere never blocks an unrelated write. A cycle or a self-edge is exit 2; a strict claim on a
blocked task is exit 5 and writes nothing.

## How do you inject a different schema?

Two tiers, by design — unchanged in substance from the Python era, with new mechanism names:

**1. Closed, code-level schema.** Each space has a typed *view* in `src/model/` (`Note`, `Task`,
`Memory`, `Scratch`, `AssetSidecar`) with a fixed field set and a `FieldOrder` that drives both
the on-disk key order and the `--json` key order. `type` is a closed set on write and free-form
on read. There is no generic `--meta key=value` escape hatch on any verb. Adding a genuinely new
typed field means editing the model.

**2. Open extension by construction.** The frontmatter is not deserialised into a struct and
re-serialised. `src/fm/load.rs` parses it into an ordered map (`Meta`), verbs mutate that map key
by key, and `src/fm/emit.rs` writes it back — so unknown keys survive because **nothing ever
removes them**. The typed view is a *derived* read of that map, used for validation and for
`--json`, never as the serialisation source on an amend path. A key literally named `extra` is an
ordinary unknown key and is treated like any other.

That is how a custom schema layers onto mesh in practice: direct file edits, or another tool
writing extra frontmatter keys. mesh will not validate them, index them or expose them through
CLI flags — but it will not drop or corrupt them either.

## How atomic are edits? What happens with two agents at once?

mesh is a standalone filesystem tool; nothing needs to be running.

- **Writes.** `src/storage/atomic.rs::atomic_write` — sibling temp file in the same directory →
  write → match the destination's mode (or `0o666 & ~umask` for a fresh file) → `fsync` →
  `rename` → best-effort parent `fsync`. Any failure before the rename unlinks the temp and
  leaves the destination untouched, never half-written.
- **Concurrency.** A per-entity `O_EXCL` lockfile under the space's `.locks/`, holding the
  claiming PID (`src/storage/lock.rs`). A lock is stale only when its PID is dead **or** it is
  older than 300 s; reclaiming a stale lock is a real compare-and-swap — open, `flock`, re-check
  `(dev, ino)` before unlink — and so is releasing one, so a lock that landed at the same path in
  between is never deleted by the wrong process. A permission error from `kill(pid, 0)` means
  *alive*, not gone. Every mutating verb **re-resolves its target inside the lock**, which is the
  TOCTOU rule the whole write layer is built on.
- **Idempotence.** A no-op never rewrites the file: reclaiming your own task, releasing an
  unclaimed one, a second `finish`, adding an existing edge, `scratch set` with an identical
  body, `asset add` of identical bytes. The tests assert this byte-for-byte, because "no write"
  is what keeps git clean and a watcher quiet.
- **Reads.** One safe reader (`src/fm/doc.rs`) that yields nothing on an I/O error, malformed
  YAML or non-UTF-8, and one walk (`src/storage/walk.rs::iter_md`) with one skip set:
  dot-prefixed path components (`.obsidian/`, `.git/`, `.locks/`, `.trash/`), nested space roots,
  files over 4 MiB, and anything that is not valid UTF-8. So one malformed or hostile file
  degrades to "skipped" rather than crashing anyone else's read.
- **Byte-level hygiene.** `src/fm/emit.rs` is hand-written and deterministic: model-declaration
  key order then unknown keys in their original order, RFC 3339 `T…Z` timestamps for values mesh
  sets, unmodified scalars re-emitted verbatim from their preserved raw text, `null` for none,
  `[]` for an empty list, no anchors or aliases ever, no line folding, one trailing newline.

Net: real filesystem-level atomicity and lock-based concurrency, independent of any running app
— and, since the daemon was deleted, independent of any mesh process too. A write is durable on
disk before anything else is told about it, so a create followed immediately by a list always
sees the new entity.

## How is the vault located? Is a central config required?

Yes — a real central config, resolved per machine (or per environment override). There is no
git-style walk-up-the-tree discovery.

- Resolution order for the file (`src/config.rs::resolve_config_path`): `--config PATH`, then
  `$MESH_CONFIG_PATH` when non-empty, then `~/.mesh/config.toml`.
- `[core].vault_path` is the one required key. `path` and `tolaria_path` remain permanent input
  aliases for it (precedence `vault_path` > `path` > `tolaria_path`, no warning); `--vault` and
  `$MESH_VAULT` override the file, in that order.
- The path is expanded then non-strictly canonicalised once at load (`storage::realpath`), so the
  sandbox, the watcher and every walker agree on one path space even when the tail does not exist
  yet.
- No config file → **exit 2**, with a three-line message naming the resolved path, `mesh init`,
  and the required key. `mesh init` creates the config file and the vault directory.
- Unknown tables and keys are **ignored, not rejected**, so a fleet mid-upgrade keeps working. A
  leftover `[daemon]` table is inert.
- Multiple vaults on one machine: point `--config`/`$MESH_CONFIG_PATH` at a different file per
  vault. Nothing is shared between them — there is no socket and no process to collide. The one
  per-vault singleton left is the `mesh watch` lock, named from a digest of the resolved vault
  path.

`mesh config show` prints the effective config after the environment overlay, every resolved
space path, and the sandbox roots — the fastest way to answer "where will this write actually
land".

## Why was the daemon removed?

The Python daemon existed to hide a ~150–180 ms interpreter floor behind a warm socket and an
in-RAM frontmatter index. In Rust a full-vault scan of thousands of files is milliseconds, so the
accelerator stopped paying for itself — and it cost a read-your-writes race (a create followed by
a list could miss the agent's own new entity, until the writer started notifying the daemon), a
second code path for every read, and a whole class of "is it up?" degradation notices.

What the daemon genuinely did beyond caching — keeping the external `indexed` index fresh, and
moving a file that ended up in the wrong status folder — is now `mesh watch`
(`src/cli/watch.rs`), an explicit foreground process. It is an accelerator, never a gatekeeper:
every command behaves identically whether or not it runs. `mesh daemon start|stop|status` remains
as a hidden non-spawning shim that keeps every key, string and exit code so existing scripts do
not crash.

---

For the broader architecture picture, see [`.spec/tech.md`](../.spec/tech.md) and
[`README.md`](../README.md).
