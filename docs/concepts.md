# Mesh Concepts

Answers to recurring architecture questions about Mesh — a CLI + MCP server for
multi-agent collaboration over a single shared Markdown vault. Each section below was
researched against the actual source and is kept as a standalone reference; if the
implementation changes, re-verify against the cited files rather than trusting this
doc blindly.

## Where do agent memories and tasks live vs. notes?

There is no separate "agent memory" store or hidden area — notes *are* the memory.
Mesh deliberately has no separate memory primitive; recall is just search over notes.

Routing is folder-based, defined by `_NOTE_SUBDIRS` / `_TASK_SUBDIRS` in
`src/mesh/storage/files.py`:

```
notes/            type: note
notes/logs/       type: log        (closest thing to "agent scratch")
notes/decisions/  type: decision
notes/references/ type: reference
notes/projects/   type: project
tasks/open/       status: open | claimed
tasks/done/       status: done | cancelled
```

All of it sits in one flat Markdown tree, fully visible in Obsidian or a plain text
editor — deliberate, since the product's whole thesis is one shared substrate for
humans and agents, not two separate audiences.

`tasks/` vs `notes/` is a real structural split: different folders, and a different
schema branch (a `Task` is a `Note` with `type: task` plus lifecycle fields). "Agent
memory" vs "human notes" is *not* a real split — that distinction doesn't exist in the
code. If someone wants it, they'd build it as a convention on top of what's there (for
example, reserve `type: log` plus a tag for agent scratch, and exclude that tag from
their own Obsidian view). Mesh gives no primitive for enforcing that separation itself.

## How do you inject different schemas into Mesh's notes tools?

Two tiers, by design:

**1. Closed, code-level schema.** `Note` and `Task` are `msgspec.Struct`s with a fixed
field set; `type` is a closed `Literal["note", "log", "decision", "reference",
"project"]`. The CLI (`mesh note new` / `update` / `append`) has no generic `--meta
key=value` escape hatch — only title, type, tags, owner, body, and file are settable.
Adding a genuinely new typed schema means editing `mesh.schemas.note` /
`mesh.schemas.task` in the Mesh codebase itself. That's deliberate — it's what keeps
the "three verbs" surface from sprawling.

**2. Open extension via the `extra` stash — the actual injection point.**
`_Frontmatter.model_validate` in `mesh.schemas.note` splits incoming frontmatter into
the known field set (validated against the `Struct`) and everything else, which is
stashed in `.extra` and merged back unchanged on write. Any extra frontmatter key —
hand-written, or written by another tool — round-trips byte-for-byte through every
Mesh read and write. Mesh won't validate it, index it, or expose it through CLI flags,
but it won't drop or corrupt it either.

That's how you layer a custom schema onto Mesh in practice: on top, via direct file
edits or another tool writing extra frontmatter keys — not through the CLI.

## How atomic are edits, how good are the reads — Mesh vs. e.g. Obsidian's CLI?

Checked directly against `obsidian.md/cli`, not assumed: Obsidian's official CLI
requires the Obsidian app to already be running — it's a remote-control surface into a
live GUI process (e.g. `obsidian daily:append content="..."`, vault targeted by name),
and its docs state no atomicity or concurrency guarantee at all. It solves "script the
app I already have open," not multi-process write-safety.

Mesh is a standalone filesystem tool — no running app required:

- **Writes.** `atomic_write` in `mesh.storage.files`: temp file in the same directory
  → `fsync(file)` → `os.replace` (atomic rename) → best-effort `fsync(dir)` for rename
  durability. A crash mid-write leaves the destination untouched, never half-written.
- **Concurrency.** A per-entity `O_EXCL` lockfile holding the claiming PID
  (`mesh.storage.locks`). A lock is stale only if its PID is dead or the lock is older
  than 300 s, and reclaiming a stale lock is a real compare-and-swap (open fd + `flock`
  + inode re-check before unlink) — not a blind unlink. That closes a real race where
  two agents could otherwise both judge a lock stale and the second delete the first's
  fresh one.
- **Reads.** Warm from the daemon's in-RAM frontmatter index when it's up (kept fresh
  by a filesystem watcher, backstopped by a writer's own post-write notification so a
  create-then-list sequence doesn't race the watcher), or a direct disk re-parse
  through one shared safe reader (`read_post`) when the daemon is down. That reader
  swallows both `OSError` and YAML parse errors, so one malformed file degrades to
  "skipped" rather than crashing anyone else's read.
- **Byte-level hygiene.** A YAML dumper that never emits anchors or aliases (identical
  `created`/`updated` datetime objects would otherwise emit `&id001`/`*id001` — valid
  YAML, but unreadable to a restricted parser), and writes preserve the destination
  file's existing mode rather than silently narrowing it to Mesh's own umask.

Net: Mesh gives real filesystem-level atomicity and lock-based concurrency,
independent of any running app. Obsidian's CLI gives scripting of an app you already
have open, with no stated consistency model. Different problems, not a strict
better/worse.

## How is the vault location determined? Is a central config file required?

Yes — a real central config, resolved per machine (or per env-var override). There is
no git-style walk-up-the-tree discovery.

- Default path is `~/.mesh/config.toml`, or `$MESH_CONFIG_PATH` if set.
- `[core].vault_path` is the one required key. `path` and `tolaria_path` remain
  permanent legacy aliases for that same key — unrelated to the shards→mesh rename,
  kept as-is for backward compatibility.
- The path is canonicalized once at load: `expanduser()` then `resolve()` (symlinks
  followed), so the sandbox, the watcher, and every walker agree on one path space.
- No config file → exit 2, naming the exact resolved path and the missing key.
  `mesh init` creates both the config file and the vault directory.
- Running multiple vaults on one machine: point `$MESH_CONFIG_PATH` at a different
  config file per vault. This is safe because the daemon socket is named from a digest
  of the *resolved vault path* (`mesh-<digest>.sock`), not the user, so two vaults'
  daemons never collide or cross-answer each other.

---

For the broader architecture picture, see [`.spec/tech.md`](../.spec/tech.md) and
[`README.md`](../README.md).
