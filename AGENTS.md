# AGENTS.md — Mesh Engineering Guide

> **`mesh`** — a mesh for multi-agent collaboration over a single Markdown folder.
> One Rust binary, five spaces (`notes`, `tasks`, `memories`, `scratch`, `assets`),
> one folder, all agents. No daemon.

---

## 0. Spec-Driven Workflow (read this first)

**This is a spec-driven repository. The spec is the source of truth — code follows the spec, never the other way around.**

The canonical product & technical contract lives in **[`.spec/`](.spec/)** — start at
[`.spec/product.md`](.spec/product.md), [`.spec/tech.md`](.spec/tech.md), and [`.spec/plan.md`](.spec/plan.md).

Before you touch anything:

1. **Confirm the `spec` skill is installed and use it.** Spec work in this repo *must* go through
   the `spec` skill — do not hand-author or hand-edit the spec.
   - Verify it's available: it should appear in your in-session skill list, or under
     `.claude/skills/spec/` (project) or `~/.claude/skills/spec/` (user). Invoke it with `/spec`.
   - **If the `spec` skill is missing, stop and install it before proceeding.** Tell the user it's
     not installed rather than working around it.
2. **Read the `.spec/` root layer** (`product.md`, `tech.md`, `design.md`, `plan.md`,
   `lessons.md`) before writing code or docs, then the feature layer for whatever arc is in
   flight — today that is [`.spec/features/rust-rewrite/`](.spec/features/rust-rewrite/). Landed
   arcs are compounded into the root layer and deleted, so the root files plus `src/` and
   `tests/` are the whole truth for everything already shipped.
3. **Any change that contradicts or extends the spec updates the spec first** — via the `spec`
   skill, with user confirmation — *then* the implementation follows.
4. Keep the surface honest: the surface is **granular verbs over five spaces**, and that is the
   whole thesis. A new verb, a new flag family or a sixth space needs a spec change and an
   explicit sign-off. Growing the surface without one is the failure mode this project is
   designed against.

---

## 1. Core Operating Principles

Follow the **ASK → PLAN → CONFIRM → EXECUTE** loop:

1. **ASK** — clarify requirements and constraints before assuming.
2. **PLAN** — break the task down and present the approach (and which spec sections it touches).
3. **CONFIRM** — get explicit user approval before implementing.
4. **EXECUTE** — implement incrementally with clear explanations.

Quality over speed. Simplicity (KISS) wins. `clippy -D warnings` clean, `cargo fmt` clean,
meaningful test coverage, and an instant CLI: cold start under 10 ms, because there is no warm
process to hide behind.

---

## 2. Repository Goals & Connections

**Goal.** Give a human operator and a fleet of agents one shared substrate for capturing
knowledge and coordinating work over a single Markdown folder — without a database, a background
process, or an external task tracker.

- **Notes + memories + search = recall.** No separate memory store: memories are note-shaped
  Markdown files in their own space, ranked by match, importance and recency; ranked retrieval
  delegates to `indexed` when it is configured and present.
- **Tasks = coordination = handoff.** `owner` / `claimed_by` / `claim` / `release` / `finish` /
  `cancel` / `list`, plus the dependency graph: `blocks` / `blocked_by` with readiness derived at
  read time, `block` / `unblock`, a strict claim gate (exit 5) and `task next`.
- **Scratch and assets are the working surface.** Per-agent session state, and content-addressed
  files beside the vault.
- **Markdown is the source of truth.** Mesh owns the *interface* (and writes), not the data.

**Connections (what mesh talks to):**

| System | Role | Boundary |
|---|---|---|
| **The vault folder** | One Markdown folder divided into five configurable spaces — source of truth | Mesh **owns writes** and direct reads inside the enabled space roots; it **coexists** with any other tool on the same folder (a Markdown editor and its plugins, another MCP server, git). If `[search].collection` is set, `mesh reindex` hands the whole configured vault root to `indexed`, which ingests everything under it — not just the spaces. Versioning, sync and backup are the vault owner's job, never mesh's |
| **`indexed`** | First-party hybrid-search engine (ingest + embeddings + ranked retrieval, CLI/MCP) | Mesh's `search` is a thin wrapper with a 30 s wall clock; the mesh↔indexed contract is co-designed; a missing, failing or slow binary degrades to the built-in BM25-lite engine, never an error |
| **Cowork agents** | Consumers (flights-agent, notes-agent, …) | Call mesh via CLI (`--json`) and the 37 `mesh_*` MCP tools |
| **`$MESH_AGENT`** | Per-session agent identity | Drives `--owner` defaults and `--mine`; validated against `[tasks].collections` across every space |
| **`mesh watch`** | Optional foreground watcher: folder reconciliation + `indexed index update` | An accelerator, never a dependency — every command behaves identically with no watcher running |

---

## 3. Tech Stack

**Language & runtime:** Rust 1.94 (pinned in `rust-toolchain.toml`), edition 2021, one crate, two
binaries (`mesh`, `mesh-mcp`), `#![forbid(unsafe_code)]`, no async runtime.

**Core crates:** `clap` 4 + `clap_complete` (CLI), `yaml-rust2` (frontmatter *read* only — the
emitter is hand-written), `serde_json` with `preserve_order` + `indexmap` (ordered payloads),
`toml` + `toml_edit` (read / format-preserving edit), `chrono`, `sha2`, `rustix` (`O_EXCL`,
`flock`, `fstat`, `kill`, `umask`), `walkdir`, `notify` + `notify-debouncer-full`, `dirs`,
`thiserror`, `which`.

**Rejected on purpose:** `regex` (five hand-written scanners in `src/text.rs`, one of which needs
a negative lookahead), `anyhow` (exit codes are a contract; every path returns
`Result<T, MeshError>`), any MCP SDK (it would pull `tokio` + `tower` + `schemars`, and the tool
schemas must be byte-controlled anyway), `mime_guess` (a static extension table is smaller and
fully deterministic).

**Dev tools:** `cargo fmt`, `cargo clippy`, `cargo test`, `assert_cmd` + `predicates` (integration
tests drive the real binary), `tempfile`, `serial_test`, `cargo-llvm-cov` (coverage floor in CI),
`pre-commit` (local hooks running the same three commands CI runs).

---

## 4. Development Workflow

### Common commands

```bash
cargo build                                    # debug build
cargo build --release                          # ./target/release/{mesh,mesh-mcp}
cargo fmt --all --check                        # format gate
cargo clippy --all-targets --locked -- -D warnings   # lint gate
cargo test                                     # the whole suite
cargo test --test note_cli                     # one integration file
cargo llvm-cov --locked --fail-under-lines 80  # coverage gate (ratchets up, never down)
./scripts/smoke.sh                             # ~40 verbs against a throwaway vault
cargo run -q -- --help                         # CLI help
cargo install --path .                         # install `mesh` and `mesh-mcp`
```

> The Rust rewrite delivers phases 1–3: five spaces, the live task dependency graph, the 37-tool
> MCP surface, and the daemon's removal. `mesh --version` prints `0.2.0`.

**Lints are load-bearing.** `unwrap` / `expect` / `panic` / `todo` / `string_slice` are denied in
`src/` (allowed under `#[cfg(test)]` and in `tests/`). Use `?`, `Option` combinators, and
`chars().take(n)` — a byte slice panics on a multibyte boundary, which is exactly what
`clippy::string_slice` exists to prevent. `sort_unstable*` is banned in `src/domain/select.rs` and
`src/search/**`: every documented ordering is a composition of stable sorts.

### Git commit standards

Format: **`type(scope): subject`** — imperative, lowercase, ≤ 50 chars, no trailing period.

Allowed types: `feat`, `fix`, `refactor`, `perf`, `style`, `test`, `docs`, `build`, `ci`, `chore`, `revert`.

```
feat(task): add atomic claim via O_EXCL lockfile
fix(search): fall back to the builtin engine when indexed is down
docs(spec): wrap indexed for ranked retrieval
```

### Branching & pushing

- Develop on the assigned feature branch; never push to the default branch without permission.
- Run the full test suite **before** any push.
- Do **not** open a pull request unless the user explicitly asks.

---

## 5. Repository Structure

```
mesh/
├── AGENTS.md            # this guide  (CLAUDE.md is a symlink to it)
├── CLAUDE.md -> AGENTS.md
├── README.md            # user-facing overview: install, config, surface, MCP
├── docs/concepts.md     # architecture Q&A: spaces, memories vs notes vs scratch, assets,
│                        #   derived readiness, atomicity, vault/config resolution
├── Cargo.toml, Cargo.lock, rust-toolchain.toml, rustfmt.toml, clippy.toml
├── config.example.toml  # every config key, documented; loaded by tests/bundle.rs
├── scripts/             # smoke.sh (verb smoke test), install.sh (cargo install wrapper)
├── .spec/               # THE SPEC — source of truth (managed via the `spec` skill)
│   ├── product.md  tech.md  design.md  plan.md  lessons.md
│   └── features/rust-rewrite/{product,tech,plan}.md
├── plugins/mesh/        # the Claude Code plugin: .mcp.json, hooks, skills/mesh/SKILL.md
└── src/
    ├── main.rs lib.rs ctx.rs        # parse → dispatch → exit code; module surface; invocation ctx
    ├── bin/mesh-mcp.rs              # shim binary the plugin bundle wires up
    ├── error.rs                     # MeshError; code() IS the exit status
    ├── config.rs spaces.rs          # config load + the five-space resolution and sandbox set
    ├── ids.rs timefmt.rs text.rs render.rs
    ├── fm/                          # frontmatter: value, load (yaml-rust2), canonical emit, doc
    ├── storage/                     # atomic write, O_EXCL locks, sandbox, THE walk
    ├── model/                       # per-space typed views + field order
    ├── domain/                      # the verbs + select/tags/owner/wikilinks/deps/lenses
    ├── search/                      # route, corpus, tokenize, builtin, tagpull, indexed, health
    ├── cli/                         # one file per verb family + globals, out, admin, watch
    └── mcp/                         # stdio JSON-RPC server, schemas, the 37-tool table
tests/                               # one integration file per verb family, plus
                                     #   compat_corpus, race, bundle, foundation_cli
```

**One primitive, one implementation, one owner.** Frontmatter round-trip, atomic write, locks, the
vault walk, filter/sort/limit, JSON rendering and the error→exit mapping each live in exactly one
module. If you find yourself writing a second walk or a second sort, stop.

---

## 6. Key Design Constraints

- **No daemon.** Every command reads disk directly and behaves identically whether or not a
  watcher runs. `mesh watch` is an optional foreground accelerator for search freshness and
  folder reconciliation only — never a gatekeeper, never a dependency.
- **Spaces are configuration, not layout.** Each of the five spaces is a folder relative to the
  vault, an absolute folder, the vault root itself, or `false` (disabled). The sandbox is the
  union of the enabled roots; type/status routing is relative to a **space root**, never the
  vault root; folders are created lazily on first write. An omitted `[spaces]` table reproduces
  the pre-rewrite layout exactly.
- **Derived state is never stored.** Task readiness is computed at read time from the union of
  both edge directions. No verb writes another entity's file as part of its own transaction: the
  unblock cascade is a *report*, and `blocks` mirrors are single-lock, one at a time, best-effort.
- **Canonical frontmatter.** Read everything the Python era wrote; write one canonical form —
  model-declaration key order, RFC 3339 `T…Z` timestamps for values mesh sets, unmodified scalars
  re-emitted from preserved raw text, no anchors, no line folding, one trailing newline. Unknown
  keys round-trip in place. Compatibility is semantic, not byte-level; machine JSON pins key
  *order*, not whitespace. Never inject machinery into bodies.
- **One walk, one skip set.** Every scan goes through `storage::walk::iter_md`, which skips
  dot-prefixed path components, nested space roots, files over 4 MiB and non-UTF-8 files, and
  through one safe reader that yields nothing rather than failing.
- **All writes are atomic, single-entity and idempotent.** Temp file plus rename, mode-preserving;
  every mutation holds that entity's lock and **re-resolves its target inside it**; lock removal
  is always a `(dev, ino)` compare-and-swap, never a blind unlink. A no-op never rewrites the
  file — assert that byte-for-byte in the tests.
- **Identity is validated across every space** at one write boundary. `$MESH_AGENT` / `--owner` /
  `claimed_by` say who an agent *claims* to be; `[tasks].collections` gates which identities are
  valid, but nothing verifies that a caller actually *is* the owner it names. That is fine for a
  trusted local fleet on one operator's machine; it is not a security boundary and must not be
  treated as one if mesh ever crosses a multi-user or multi-machine trust line.
- **Path sandboxing.** Every resolved path must equal or sit beneath one enabled space root;
  reject traversal and symlink escapes.
- **No panics on user input.** `main` catches anything that escapes and prints
  `internal error: {msg}` at exit 1 rather than a trace.
- **Agent content is data**, never instructions or shell input.
- **Hash IDs** (`n-`, `t-`, `m-`, `a-`), never sequential; asset ids digest the content, making
  the id the content address. Existing ids are never recomputed.
- **Delete is a hard `unlink`, by design.** No soft-delete, no trash: the vault belongs to the
  operator, and versioning/backup is the vault owner's concern (git, a sync client, a Time
  Machine, or nothing). Mesh is not a backup tool and promises no recovery. A `.trash/` would add
  a second delete lifecycle to keep in sync with whatever history mechanism the operator already
  runs — evaluated and deferred, not built. The trade-off is explicit: an unversioned vault has
  no recovery path for a deleted note, and that is the operator's call.

---

## 7. Claude Code Skills

- **`spec`** — *required* for all spec work (see §0). Invoke with `/spec`.
- **`feature-dev`** (if available) — multi-file features needing architectural guidance.
- **`find-skills`** — discover additional capabilities.

`plugins/mesh/skills/mesh/SKILL.md` is a *product* artefact, not a dev skill: it is the playbook
shipped to agents that consume mesh. Its invariants are pinned by `tests/bundle.rs`.

---

## 8. DO / DON'T

**DO ✅**
- Read the spec and route spec changes through the `spec` skill, first.
- Ask → plan → confirm → execute; get approval before coding.
- Keep the surface at granular verbs over five spaces; add tests alongside every behaviour.
- Make writes atomic, single-entity and idempotent; keep every command working with no watcher.
- Reuse the one walk, the one reader, the one select engine, the one output surface.

**DON'T ❌**
- Never hand-edit the spec or skip the `spec` skill.
- Never reintroduce a daemon, a warm index, or any read path that is not "read the folder".
- Never reintroduce a separate memory store, a Todoist/external task backend, or a standalone
  handoff primitive.
- Never store derived state (readiness, counts, backlinks) on disk, and never write another
  entity's file inside your own verb's transaction.
- Never add sequential IDs, a vector DB to operate, or git-sync logic.
- Never `unwrap`/`expect`/`panic` in `src/`, and never slice a string by bytes.
- Never push to the default branch or open a PR without explicit permission.

---

**Remember:** spec first, then ASK → PLAN → CONFIRM → EXECUTE. Quality and simplicity over speed.
