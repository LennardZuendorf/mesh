---
type: feature-plan
feature: rust-rewrite
sibling: tech.md
parent: ../../plan.md
updated: 2026-09-05
---

# Feature: Rust Rewrite — Implementation Plan

Eleven units. One blocking foundation lands the frozen seam — dependencies, the complete clap
tree, the frontmatter contract, storage, config, spaces, select, errors and the compat corpus —
and gates on that corpus round-tripping. Eight verb-family units then land in parallel against
that seam, each owning a disjoint file set. Unit ten deletes the Python implementation and lands
docs, packaging and CI. Unit eleven verifies the whole surface and compounds the cross-cutting
decisions into the root layer.

**Parent:** [../../plan.md](../../plan.md)
**Requirements:** [product.md](product.md)
**Architecture:** [tech.md](tech.md)

**Feature gate:** no upstream feature. Every earlier arc in the root Feature Sequence is `DONE`;
`tasks-graph`, deferred since Phase 1, is delivered inside this feature rather than as a
separate one.

---

## Problem Frame

A rewrite is one atomic promise — the binary either serves the whole surface or the vault has no
tool. The sequencing problem is therefore not "what order do features ship in" but "what must be
frozen before anything can be built in parallel". Everything that must be byte-consistent across
every verb (frontmatter round-trip, atomic write, locks, the walk, filter/sort/limit, JSON
rendering, error-to-exit mapping) is one file with one owner, landed first and frozen. Everything
after it is a verb family whose diff is confined to its own files. The compat corpus is the gate
between the two: until a Python-written vault reads, dumps and re-reads with full semantic
fidelity, no verb work starts.

---

## Requirements Trace

| ID | Requirement | Units |
|---|---|---|
| R1 | [One Rust binary, instant start](product.md#requirement-r1--one-rust-binary-instant-start) | rust-rewrite/1, rust-rewrite/10, rust-rewrite/11 |
| R2 | [Configurable spaces](product.md#requirement-r2--configurable-spaces) | rust-rewrite/1 |
| R3 | [The notes space may be the whole vault](product.md#requirement-r3--the-notes-space-may-be-the-whole-vault) | rust-rewrite/1, rust-rewrite/2 |
| R4 | [Note verbs](product.md#requirement-r4--note-verbs) | rust-rewrite/2 |
| R5 | [Task lifecycle verbs](product.md#requirement-r5--task-lifecycle-verbs) | rust-rewrite/3 |
| R6 | [Live task dependency graph](product.md#requirement-r6--live-task-dependency-graph) | rust-rewrite/3 |
| R7 | [A single "give me work" primitive](product.md#requirement-r7--a-single-give-me-work-primitive) | rust-rewrite/3 |
| R8 | [Memory verbs](product.md#requirement-r8--memory-verbs) | rust-rewrite/4 |
| R9 | [Recall ranking](product.md#requirement-r9--recall-ranking) | rust-rewrite/4, rust-rewrite/7 |
| R10 | [Supersession and expiry](product.md#requirement-r10--supersession-and-expiry-never-auto-deletion) | rust-rewrite/4 |
| R11 | [Scratch](product.md#requirement-r11--scratch) | rust-rewrite/5 |
| R12 | [Content-addressed assets](product.md#requirement-r12--content-addressed-assets) | rust-rewrite/6 |
| R13 | [Attaching assets to entities](product.md#requirement-r13--attaching-assets-to-entities) | rust-rewrite/6 |
| R14 | [Search across spaces and engines](product.md#requirement-r14--search-across-spaces-and-engines) | rust-rewrite/7 |
| R15 | [The indexed wrapper](product.md#requirement-r15--the-indexed-wrapper) | rust-rewrite/7 |
| R16 | [Read-only lenses](product.md#requirement-r16--read-only-lenses) | rust-rewrite/8 |
| R17 | [Session start with memories and a budget](product.md#requirement-r17--session-start-with-memories-and-a-budget) | rust-rewrite/8 |
| R18 | [Admin surface](product.md#requirement-r18--admin-surface) | rust-rewrite/10 |
| R19 | [The daemon is removed; the watcher is optional](product.md#requirement-r19--the-daemon-is-removed-the-watcher-is-optional) | rust-rewrite/10 |
| R20 | [MCP server](product.md#requirement-r20--mcp-server) | rust-rewrite/9 |
| R21 | [Python-era vaults keep working](product.md#requirement-r21--python-era-vaults-keep-working) | rust-rewrite/1, rust-rewrite/11 |
| R22 | [Exit codes](product.md#requirement-r22--exit-codes) | rust-rewrite/1, rust-rewrite/11 |
| R23 | [JSON error envelope](product.md#requirement-r23--json-error-envelope) | rust-rewrite/1, rust-rewrite/9 |
| R24 | [Identity and roster across every space](product.md#requirement-r24--identity-and-roster-across-every-space) | rust-rewrite/1 |
| R25 | [Safe writes](product.md#requirement-r25--safe-writes) | rust-rewrite/1, rust-rewrite/3 |

Every unit below cites the R-IDs it satisfies. R-IDs are never renumbered.

---

## Key Technical Decisions

1. **One package, not a workspace.** Fewer manifests to contend over; the lint hardening that
   justified a split lives in `lib.rs` instead. See [tech.md](tech.md) § Files.
2. **Freeze the seam before parallelising.** The complete clap tree and every domain signature
   land in unit one as compiling stubs, so later units only replace bodies of files they own.
3. **Semantic compatibility, not byte compatibility.** Read everything the Python era wrote;
   write a canonical form. This deletes the two hardest sub-projects in the port (a
   PyYAML-identical emitter with 80-column folding, and a Python-identical JSON formatter) and
   costs nothing an operator can observe.
4. **Readiness is derived.** No verb writes another entity's file, so there is no multi-file
   transaction, no lock ordering, no cascade of `updated` bumps and no write amplification
   through the watcher.
5. **The daemon is deleted, not ported.** A Rust full-vault scan is milliseconds; the warm index
   has no job left. The legacy verbs survive as a shim so no script breaks.
6. **Every mutation writes exactly one entity under exactly one lock**, re-resolved inside that
   lock, atomically.

---

## Unit IDs

Units are `rust-rewrite/n` — assigned once, never renumbered on reorder. Cite the ID in commits
and tests (`feat(task): rust-rewrite/3 derive readiness from both edge directions`).

---

### rust-rewrite/1 — Foundation and the frozen seam

**Goal:** the package builds, the whole command tree parses, and a Python-written vault round-trips.

**Requirements:** R1, R2, R3, R21, R22, R23, R24, R25

**Dependencies:** —

**Files:**

```
Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml clippy.toml deny.toml
src/lib.rs src/main.rs src/ctx.rs src/bin/mesh-mcp.rs
src/error.rs src/config.rs src/spaces.rs src/ids.rs src/timefmt.rs src/text.rs src/render.rs
src/fm/*.rs src/storage/*.rs src/model/{mod,common}.rs
src/domain/{mod,select,tags,owner,wikilinks}.rs
src/cli/{mod,globals,out}.rs                  # complete clap tree; every verb file as a stub
tests/common/mod.rs tests/fixtures/python-vault/** tests/compat_corpus.rs
```

**Test scenarios:**

- Every file in the frozen corpus parses; every typed field carries its expected value; unknown
  keys including one named `extra` survive load, dump and reload unchanged.
- A read command over the whole corpus changes no file's bytes or modification time.
- An append changes only the update timestamp, the derived relations and the appended block.
- Space resolution: absent table reproduces the legacy layout; disabled space exits 2; duplicate
  roots exit 2; a relative escape exits 2; an absolute root is accepted and reported.
- Walk skip set: dot-directories, a 5 MiB Markdown file and a non-UTF-8 file are all skipped.
- Lock table: stale reclaim happens exactly once, a live lock exits 4, and the release
  compare-and-swap never unlinks a different file at the same path.
- Error table: every variant's message is byte-exact and maps to its documented exit code; the
  machine-mode envelope carries the documented key order.

**Verification:** `cargo test --locked compat_corpus` plus `cargo clippy --all-targets -- -D warnings`
green, and `cargo run -- --help` listing every subcommand in the fixed order.

---

### rust-rewrite/2 — Note family

**Goal:** the six note verbs, foreign-file reads, and the rename advisory.

**Requirements:** R3, R4

**Dependencies:** rust-rewrite/1

**Files:**

```
src/model/note.rs src/domain/notes.rs src/cli/note.rs
tests/note_cli.rs
```

**Test scenarios:**

- Create, append, update, get, list, delete round-trip with the legacy row formats, key order,
  branch precedence and exit codes.
- The duplicate-title advisory fires on stderr, is suppressed by quiet mode, and never enters a
  payload.
- A rename with title-form backlinks succeeds and emits one advisory naming the count and ids.
- Foreign Markdown is listed and readable behind the foreign flag, and is not found for append,
  update and delete.

**Verification:** `cargo test --locked --test note_cli`.

---

### rust-rewrite/3 — Task family and the dependency graph

**Goal:** the ten lifecycle verbs, live readiness, edge mutation, and the work-selection verb.

**Requirements:** R5, R6, R7, R25

**Dependencies:** rust-rewrite/1

**Files:**

```
src/model/task.rs src/domain/tasks.rs src/domain/deps.rs
src/cli/task.rs src/cli/task_dep.rs
tests/task_cli.rs tests/task_dep_cli.rs tests/race.rs
```

**Test scenarios:**

- Eight processes racing one claim: exactly one exit 0, seven exit 4, one holder on disk.
- Idempotent no-ops (same-agent reclaim, release of an unclaimed task, a second finish or cancel,
  a duplicate edge, an absent edge removal) rewrite no bytes and move no timestamp.
- Readiness from a one-sided hand-written edge; a dangling blocker counts as satisfied.
- A cycle and a self-edge are refused before any write; breaking a cycle is never refused.
- Finishing a blocker reports the newly ready ids and rewrites no dependent file.
- Strict claim on a blocked task exits 5 and writes nothing; non-strict claim succeeds with an
  advisory.
- Two agents invoking the next verb with claim concurrently each obtain a different task; with
  nothing ready the verb exits 3.

**Verification:** `cargo test --locked --test task_cli --test task_dep_cli --test race`.

---

### rust-rewrite/4 — Memory family

**Goal:** the seven memory verbs, recall ranking, supersession and expiry.

**Requirements:** R8, R9, R10

**Dependencies:** rust-rewrite/1

**Files:**

```
src/model/memory.rs src/domain/memories.rs src/cli/memory.rs
tests/memory_cli.rs
```

**Test scenarios:**

- Create, append, update, get, list and forget over ids and title slugs; the fourteen-line meta
  block and the declared key order.
- A private memory is hidden from a listing run as a different identity and visible to its owner.
- Supersession keeps both files, marks the old one, and removes it from recall and default lists.
- An expired memory disappears from recall and default lists, reappears behind the flag, and is
  still on disk.
- Recall ranks a recently updated memory above an equally matching stale one; disabling decay
  removes that effect; recall never writes.
- Flat layout: no memory verb creates an identity-derived directory or moves a file.

**Verification:** `cargo test --locked --test memory_cli`.

---

### rust-rewrite/5 — Scratch family

**Goal:** per-agent, name-addressed working files with five verbs.

**Requirements:** R11

**Dependencies:** rust-rewrite/1

**Files:**

```
src/model/scratch.rs src/domain/scratch.rs src/cli/scratch.rs
tests/scratch_cli.rs
```

**Test scenarios:**

- Set overwrites idempotently; append adds; get returns the body verbatim with no truncation.
- A name that normalises to empty exits 2; a name and an agent that both slugify cannot collide
  across namespaces.
- Reading and writing another agent's namespace works through the explicit identity flag.
- Clear obeys the delete guard on every row of the decision table.
- Scratch files appear in no lens and in no default search.

**Verification:** `cargo test --locked --test scratch_cli`.

---

### rust-rewrite/6 — Asset family

**Goal:** content-addressed ingest, sidecars, attach and detach, referenced-removal refusal, gc.

**Requirements:** R12, R13

**Dependencies:** rust-rewrite/1, rust-rewrite/2, rust-rewrite/3, rust-rewrite/4

**Files:**

```
src/model/asset.rs src/domain/assets.rs src/cli/asset.rs
tests/asset_cli.rs
```

**Test scenarios:**

- Adding identical bytes twice returns one id, writes nothing the second time, exits 0 and emits
  the advisory.
- A hostile source filename never influences the stored path and survives as metadata.
- The blob is written before the sidecar; a forced sidecar failure leaves no sidecar and no blob.
- Attach appends an embed through the ordinary append path, so relations and the graph lens pick
  the pair up in both directions; detach removes the relations and leaves the body text alone.
- Removing a referenced asset without force exits 2 naming the reference count; gc reports
  orphans and changes nothing unless applied.
- The path verb prints the absolute blob path and exits 3 when the blob is missing.

**Verification:** `cargo test --locked --test asset_cli`.

---

### rust-rewrite/7 — Search

**Goal:** the corpus, the ranked built-in engine, tag pull, the engine wrapper and the health payload.

**Requirements:** R9, R14, R15

**Dependencies:** rust-rewrite/1

**Files:**

```
src/search/*.rs src/cli/search.rs
tests/search_cli.rs tests/fixtures/fake-indexed/
```

**Test scenarios:**

- A Python-era vault returns the legacy hit key set and ordering with no new flags.
- Every pinned tier remains exactly reachable as a floor; substring mode reproduces legacy
  scoring and head-of-body snippets.
- A multi-word query with no literal substring match returns ranked results.
- Space restriction returns only that space and names it on each hit; the memory-kind filter
  applies.
- The stub engine pins the argv byte-for-byte; blank, malformed and boolean-score records are
  skipped; an integer score coerces; a hung engine degrades within the wall clock with the
  standard advisory and exit 0; an escaping path is dropped.
- The health payload short-circuits before any work, keeps its five keys in order, names only the
  first closed gate, and reports the branch actually taken.

**Verification:** `cargo test --locked --test search_cli`.

---

### rust-rewrite/8 — Lenses

**Goal:** the five read-only lenses, including memories and the budget in session start.

**Requirements:** R16, R17

**Dependencies:** rust-rewrite/2, rust-rewrite/3, rust-rewrite/4

**Files:**

```
src/domain/activity.rs src/domain/context.rs src/domain/lenses.rs src/cli/lens.rs
tests/lens_cli.rs
```

**Test scenarios:**

- Row formats, ordering, dedup, tree indentation, edge orientation and the absent-value
  convention match the legacy contract for all five lenses.
- Direction is validated before seed resolution; the inbound index is built at most once and
  never at depth zero.
- Session start composes tasks, mentions, memories and activity in that order, dedupes by id with
  the earlier section winning, caps memories at five, and suppresses them behind the flag.
- A budget trims bodies before entries and appends exactly one synthetic truncated entry naming
  the dropped count.
- Every lens leaves the vault byte-identical, and no lens emits an infrastructure notice.

**Verification:** `cargo test --locked --test lens_cli`.

---

### rust-rewrite/9 — MCP server

**Goal:** the stdio JSON-RPC server and the 37-tool table.

**Requirements:** R20, R23

**Dependencies:** rust-rewrite/2, rust-rewrite/3, rust-rewrite/4, rust-rewrite/5, rust-rewrite/6, rust-rewrite/7, rust-rewrite/8

**Files:**

```
src/mcp/*.rs
tests/mcp_cli.rs
```

**Test scenarios:**

- A scripted session drives initialize, initialized, ping, tools list and tools call; an unknown
  method returns the documented code.
- Exactly 37 tools with the expected names; the 21 legacy names, parameters and descriptions are
  unchanged; every tool carries all three annotation hints explicitly and exactly one is
  destructive.
- No registered name contains delete, daemon, reindex or status; every withheld verb is absent.
- List-returning tools wrap their array; errors return the identical envelope object the CLI
  emits, one per error kind.
- With no config anywhere, every tool returns the config-missing payload and the server keeps
  serving.
- The tag-spec text is byte-identical across its four sites; the serialised tool list is under
  32 KB and the instructions block under 2048 bytes.

**Verification:** `cargo test --locked --test mcp_cli`.

---

### rust-rewrite/10 — Admin, watch, packaging, docs, and Python removal

**Goal:** the admin surface, the watcher, the bundle and CI, and the deletion of the Python tree.

**Requirements:** R1, R18, R19

**Dependencies:** rust-rewrite/1, rust-rewrite/7, rust-rewrite/8, rust-rewrite/9

**Files:**

```
src/cli/admin.rs src/cli/watch.rs
tests/admin_cli.rs tests/bundle.rs scripts/smoke.sh
.github/workflows/ci.yml .pre-commit-config.yaml .gitignore config.example.toml
README.md docs/concepts.md AGENTS.md plugins/** hooks/** .claude-plugin/**
DELETED: src/mesh/**, the Python tests/**, pyproject.toml, uv.lock
```

**Test scenarios:**

- Init renders the config, creates only the vault root, and refuses an existing file without
  force without opening it for writing.
- Status is read-only, keeps every legacy key in position, and appends the new groups; dangling
  links are capped with a total alongside.
- Config show reports the effective configuration, every resolved space path and every sandbox
  root; config set preserves comments and ordering.
- Reindex always exits 0, emits one advisory on a missing engine, and is a silent no-op with no
  collection configured.
- A second watcher exits 4; the watcher reconciles a misfiled entity without moving its update
  timestamp; a malformed file does not stop the loop.
- The daemon shim never spawns, always exits 0, and preserves every key and string.
- The bundle test pins both binary names, the server entry name, the byte-identical hook pair,
  the playbook's allowed-tools set and its rule headings against the live command tree.
- No Python file, manifest or lockfile remains in the tree.

**Verification:** `cargo test --locked --test admin_cli --test bundle` and `bash scripts/smoke.sh`.

---

### rust-rewrite/11 — Verification and compound

**Goal:** prove the whole surface against the requirement set, then promote the cross-cutting
decisions into the root layer.

**Requirements:** R1, R21, R22

**Dependencies:** rust-rewrite/2, rust-rewrite/3, rust-rewrite/4, rust-rewrite/5, rust-rewrite/6, rust-rewrite/7, rust-rewrite/8, rust-rewrite/9, rust-rewrite/10

**Files:**

```
tests/compat_corpus.rs tests/*_cli.rs scripts/smoke.sh
.spec/product.md .spec/tech.md .spec/design.md .spec/plan.md
```

**Test scenarios:**

- The exit-code matrix is exercised once per documented failure and no output contains a panic.
- The flag-placement matrix holds byte-identically before and after the command name for every
  non-admin command, including the idempotent no-op branches.
- A cold read command on a warm filesystem starts in under 10 ms, and the MCP tool table is never
  constructed off the MCP path.
- The frozen corpus still round-trips after every verb family has landed.

**Verification:** `cargo test --workspace --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
`cargo llvm-cov --locked --fail-under-lines 80`, `bash scripts/smoke.sh`, then
`bash .agents/skills/spec/scripts/validate.sh` after the merge blocks in
[tech.md](tech.md) are promoted into the root layer.

---

## Dependencies

| Unit | Blocks | Blocked by |
|---|---|---|
| rust-rewrite/1 | every other unit | — |
| rust-rewrite/2 | rust-rewrite/6, rust-rewrite/8, rust-rewrite/9 | rust-rewrite/1 |
| rust-rewrite/3 | rust-rewrite/6, rust-rewrite/8, rust-rewrite/9 | rust-rewrite/1 |
| rust-rewrite/4 | rust-rewrite/6, rust-rewrite/8, rust-rewrite/9 | rust-rewrite/1 |
| rust-rewrite/5 | rust-rewrite/9 | rust-rewrite/1 |
| rust-rewrite/6 | rust-rewrite/9 | rust-rewrite/2, rust-rewrite/3, rust-rewrite/4 |
| rust-rewrite/7 | rust-rewrite/9, rust-rewrite/10 | rust-rewrite/1 |
| rust-rewrite/8 | rust-rewrite/9, rust-rewrite/10 | rust-rewrite/2, rust-rewrite/3, rust-rewrite/4 |
| rust-rewrite/9 | rust-rewrite/10 | rust-rewrite/2 … rust-rewrite/8 |
| rust-rewrite/10 | rust-rewrite/11 | rust-rewrite/1, rust-rewrite/7, rust-rewrite/8, rust-rewrite/9 |
| rust-rewrite/11 | — | rust-rewrite/2 … rust-rewrite/10 |

Same-feature dependencies only. Units 2, 3, 4, 5 and 7 are mutually independent and run in
parallel on disjoint files.

---

## Progress

| Unit | Status |
|---|---|
| rust-rewrite/1 | NOT STARTED |
| rust-rewrite/2 | NOT STARTED |
| rust-rewrite/3 | NOT STARTED |
| rust-rewrite/4 | NOT STARTED |
| rust-rewrite/5 | NOT STARTED |
| rust-rewrite/6 | NOT STARTED |
| rust-rewrite/7 | NOT STARTED |
| rust-rewrite/8 | NOT STARTED |
| rust-rewrite/9 | NOT STARTED |
| rust-rewrite/10 | NOT STARTED |
| rust-rewrite/11 | NOT STARTED |
