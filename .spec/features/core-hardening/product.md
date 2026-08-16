---
type: feature-product
feature: core-hardening
sibling: tech.md
parent: ../../product.md
updated: 2026-08-15
---

# Feature: Core Hardening — Product

Makes the behaviour shards already promises actually true. A repo-wide review found seven
correctness bugs, two falsified invariants, a dead acceleration path, and spec text that no
longer matches the shipped binary — all inside surfaces already marked DONE. This feature
closes that gap and adds **no new user-facing capability**: every requirement below restores a
promise already made in root [product.md](../../product.md), [tech.md](../../tech.md), or
[lessons.md](../../lessons.md).

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | Reader/writer symmetry for foreign files; create-path race safety; infrastructure-failure exit codes; fallback body-match recall; whole-vault dangling-link counting; the daemon warm-index wiring decision and its stub cull; exit codes on domain exception classes; the duplication cull that survives the DRY filter; test coverage for locks, sandbox, MCP tool bodies, and daemon lifecycle; CI gate hardening; reconciling shipped surface vs. spec text (root corrections raised as gated follow-ups). |
| **Does not own** | MCP tool instructions/descriptions, the shipped Skill, `shards init`, and the user-facing CLI flag contract (incl. the global `--owner` semantics) — **agent-usability**. Backlinks, `task append`, `task release`, `--stale`, `session-start --team`, priority ordering, duplicate-title warning — **team-awareness**. Phase-3 task-graph. Any new verb, flag, or output mode. Editing root `.spec/*.md` (gated — see [plan.md](plan.md) § Root follow-ups). |
| **Deferred** | Removing `build-context` in favour of `graph` (specified here, gated on a root `product.md` edit). Serving the substring fallback's title/tag tiers from the warm index. Merging the five vault-walk implementations beyond a shared parameterised iterator. |

---

## Problem

Phase 1–2 is green: 678 tests, 93% coverage, `ruff` and `ty` clean. Green proved the tests
that exist pass — the recorded lesson *"A green gate is not a complete feature"*. What it did
not prove:

- **Two lessons regressed.** *"Foreign-file tolerance must be symmetric across every reader"*
  was fixed on the note side and left broken on the task side. *"One mechanic, one home — and
  read from the warm index"* was recorded as fixed while eight of nine daemon read methods
  stayed permanent `503` stubs.
- **Invariant 7 is false.** The warm index accelerates one lens. Every `task list` full-scans
  and YAML-parses the vault with the daemon up and holding that same data in RAM.
- **Infrastructure failures traceback.** A contended lock and a full disk both exit with a
  Python traceback, indistinguishable from a bug.
- **Body search returns nothing at default config** when `indexed` is absent — the exact state
  a first-run install is in.
- **The spec describes a binary that does not ship.** A `project` verb exists that a feature
  spec forbids; gates demand a type-checker that is not a dependency.

---

## Requirements

### Requirement: Foreign-file tolerance is symmetric

Every reader that parses a vault Markdown file MUST route through the single safe reader that
guards **both** `OSError` and malformed YAML. No reader may hand-roll a narrower guard.

#### Scenario: One unreadable file does not crash a task lens

- **Given** a vault containing a `.md` file that cannot be read (permissions, vanished mid-scan)
- **When** an agent runs the task list, status, or session-start lens
- **Then** the offending file is skipped silently and every other entry is returned — the same
  outcome the note lens already produces over that identical vault

#### Scenario: Malformed frontmatter on a write path degrades, not crashes

- **Given** a task or note file whose frontmatter is unparseable YAML
- **When** an agent claims, finishes, updates, or appends to a *different* entity
- **Then** the operation succeeds; when the malformed file **is** the target, the caller gets
  the documented not-found/validation exit code, never a traceback

### Requirement: Creates are race-safe

Creating a note or task MUST serialize id allocation against the write, so two concurrent
creates can never resolve to the same file path and silently destroy one another's content.

#### Scenario: Concurrent colliding creates both survive

- **Given** two agents creating entities that hash to the same id candidate at the same instant
- **When** both creates run to completion
- **Then** two distinct files exist with two distinct ids (the loser extends its id), and
  neither body is lost

#### Scenario: Create matches every other write path

- **Given** the documented invariant that all writes are atomic and serialized per entity
- **When** any write path is inspected
- **Then** create holds the same per-entity lock discipline as claim, update, append, finish,
  and cancel — no write path is the exception

### Requirement: Infrastructure failures exit like failures

Every CLI and MCP boundary MUST convert an infrastructure failure — a contended lock, a
filesystem error — into the documented exit code and a one-line message. No user-reachable
path may terminate with a traceback.

#### Scenario: A contended lock reports a conflict

- **Given** another process holding an entity lock past the wait budget
- **When** an agent writes to that entity
- **Then** the command exits with the documented code and a single stderr line naming the
  entity — no traceback, no ~15s silent hang without explanation

#### Scenario: A full disk reports a filesystem error

- **Given** a vault on a filesystem with no free space
- **When** an agent writes
- **Then** the command exits `1` with a message identifying the write failure

### Requirement: Exit codes live on the exception classes

The exit-code convention MUST have one owner. Either each domain exception carries its code
and the CLI boundary maps it once, or root [tech.md](../../tech.md) is corrected to describe
what ships. The two MUST NOT disagree.

#### Scenario: Adding a boundary cannot invent a code

- **Given** a new command that raises an existing domain exception
- **When** the boundary maps it
- **Then** the code comes from the exception, not from a literal repeated at the call site

### Requirement: Body matches are returned at default configuration

Substring-fallback search MUST return body matches on a default install. A default
configuration value MUST NOT silently suppress a whole match tier.

#### Scenario: First-run recall works without `indexed`

- **Given** a fresh install with no `indexed` binary, default config, and a note whose body
  (not title, not tags) contains the query term
- **When** an agent searches for that term
- **Then** the note is returned

#### Scenario: An explicit threshold still filters

- **Given** the same vault
- **When** an agent passes an explicit threshold above the body tier
- **Then** body-only matches are excluded — the flag keeps its documented meaning

### Requirement: Vault-health counts cover the whole vault

Health counts reported by the status lens MUST cover both `notes/` and `tasks/`. A count that
silently covers half the corpus is a wrong number, not a partial one.

#### Scenario: A dangling link in a task body is counted

- **Given** a task whose body contains a title-form wikilink matching no note
- **When** an agent runs the status lens
- **Then** that link is included in the dangling count

### Requirement: The daemon accelerates the reads it claims to

The daemon MUST either serve the read lenses from its warm index or stop claiming to. Every
RPC method the spec lists MUST have a real handler and at least one production caller;
methods with neither MUST be removed from both the code and the method table.

#### Scenario: A warm daemon does not re-parse the vault

- **Given** a running daemon holding parsed frontmatter for the whole vault
- **When** an agent lists tasks or notes
- **Then** the result is served from the warm index — the CLI performs no full-vault YAML parse

#### Scenario: Degradation is unchanged

- **Given** the daemon is down, hung, or answers an error
- **When** an agent runs any read lens
- **Then** the identical result is produced by the file-op fallback, with one stderr notice —
  the accelerator never becomes a dependency

#### Scenario: No permanently-stubbed method survives

- **Given** the shipped method table
- **When** it is compared against the handlers and their callers
- **Then** every listed method resolves to a real handler, and the client exposes no method
  reachable only from tests

### Requirement: The spec matches the shipped surface

Where spec text and the shipped binary disagree, the disagreement MUST be resolved
explicitly — by correcting the spec or by removing the surface — never left standing.

#### Scenario: A shipped verb is not forbidden by its own spec

- **Given** a verb present in `--help`
- **When** the feature spec that introduced it is read
- **Then** the spec describes that verb as shipped, or the verb is removed

#### Scenario: Every quality gate is runnable

- **Given** a verification command named in any spec
- **When** it is run against the current toolchain
- **Then** it executes — no gate names a tool that is not a dependency

### Requirement: Concurrency and sandbox claims are tested under their own conditions

The claims shards makes about atomic claiming and path sandboxing MUST be covered by tests
that run under the conditions the claim is about. A single-process thread barrier does not
exercise a cross-process race.

#### Scenario: The claim race is proven across processes

- **Given** N separate OS processes racing to claim one task
- **When** they all run
- **Then** exactly one wins and the rest report the conflict — and the stale-reclaim
  compare-and-swap path is executed, not structurally unreachable

#### Scenario: Escape vectors are enumerated

- **Given** an out-of-vault absolute path, a symlinked directory component mid-path, a `..`
  segment inside a filename, and a hardlink
- **When** each is supplied through the CLI and the MCP surface
- **Then** each is rejected

#### Scenario: Mutating agent tools are executed by tests

- **Given** the MCP tool surface
- **When** the suite runs
- **Then** every mutating tool body executes — registration metadata alone is not coverage

### Requirement: One mechanic, one home — where the shapes match

Duplicated mechanics MUST be reduced to one implementation **only** where the callers share
one shape. An extraction that requires a discriminator, a strategy callable, or a per-caller
branch MUST be rejected and the duplication kept.

#### Scenario: The filter is applied and recorded

- **Given** the catalogue of duplicated mechanics
- **When** each is evaluated
- **Then** each is either merged or explicitly rejected with the reason, and no merged helper
  carries a per-caller discriminator

---

## Non-Goals

- No new verb, flag, output mode, or MCP tool. Every change is behaviour-preserving except
  where the current behaviour is the bug.
- No change to the on-disk Markdown contract, the id scheme, or frontmatter keys.
- No Phase-3 task-graph work.
- No edits to root `.spec/*.md` from this feature's branch — corrections are listed as
  follow-ups in [plan.md](plan.md) § Root follow-ups and applied at compound.
- No re-litigation of the delete stance, the soft-delete decision, or the Rust rewrite.

---

## Open Questions

1. **Global `--owner` plumbing.** The flag is parsed then dropped everywhere except `task
   claim` and the recent-activity lens. The *contract* (what `--owner` should mean per command)
   belongs to **agent-usability**. This feature treats the drop as a bug only if that track
   specs `--owner` as global; otherwise the fix ships there. Recommendation: agent-usability
   states the contract, core-hardening ships the plumbing under their requirement ID. Do not
   duplicate the flag surface in both specs.
2. **`build-context` removal.** `build-context` is a strict subset of `graph` — same
   traversal, one is the other's first return value. Removing it deletes ~390 LOC and one MCP
   tool but retires a shipped lens named in root [product.md](../../product.md), which is a
   gated edit. Recommendation: spec the removal, ship it only after the root edit is approved
   at compound.
3. **`search --health` vs `shards status`.** `--health` reports engine reachability that the
   status lens should own. Folding it in is a user-visible surface change and therefore
   agent-usability's call, not this feature's. Recommendation: raise, do not act.
4. **Real-`indexed` contract test.** Every `indexed` invocation is subprocess-mocked, so
   upstream drift is invisible. A contract test needs the real binary in CI. Recommendation:
   an opt-in job that skips when the binary is absent — never a hard CI dependency, since
   `indexed` absence is a supported runtime state.
