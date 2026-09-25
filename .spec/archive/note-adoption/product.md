---
type: feature-product
feature: note-adoption
sibling: tech.md
parent: ../../product.md
updated: 2026-09-25
---

# Feature: Note Adoption & Claims — Product

mesh exposes an existing Markdown vault as first-class mesh content. `note adopt` mints mesh ids
into foreign files without disturbing them; `note claim` / `note release` give agents task-like
atomic coordination over long-lived notes; `owner` records the durable area a note belongs to —
distinct from the transient claim. This reverses one word of the root contract: foreign files
were "never mutated"; adoption is now the **explicit, operator-driven on-ramp** that turns a
foreign file into a mesh-native one, and it mutates nothing else.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)

---

## Scope

| | |
|---|---|
| **Owns** | The note-verb surface for adoption, claims and owner assignment; `claimed_by` on the notes model; the status census extension for note ownership/claims; the MCP note tools for adopt/claim/release; docs for these verbs |
| **Does not own** | Task lifecycle semantics (notes gain no status field); the project envelope lens ([project-envelope](../project-envelope/product.md)); foreign vault conventions (preserved untouched); wikilink mechanics (they only benefit) |

---

## Requirements

### Requirement: Minimal, idempotent adoption

The system SHALL mint a mesh id into an existing Markdown file inside an enabled notes space by
inserting **only** frontmatter keys the file lacks: `id` (always); `created`, `updated` and
`title` (when absent — `title` derived from the first H1, else the filename stem). Every
pre-existing key and value MUST round-trip unchanged, in place, per the canonical frontmatter
contract. A file that already carries a mesh id MUST be reported as adopted with its existing id
and MUST NOT be rewritten — a no-op never rewrites the file.

#### Scenario: Foreign file with rich frontmatter

- **Given** a vault-root notes space containing a file with foreign frontmatter (`type: Project`,
  `status: captured`, `belongs_to: []`, `schema: 3`)
- **When** the operator adopts it
- **Then** the file gains `id` plus any absent `created`/`updated`/`title` and nothing else;
  every foreign key and value reads back identical; `note list` now shows it

#### Scenario: Re-adoption is a no-op

- **Given** a file that already has a mesh id
- **When** it is adopted again
- **Then** the command reports the existing id, exits 0, and the file is byte-identical

#### Scenario: No frontmatter block

- **Given** a body-only Markdown file in the notes space
- **When** it is adopted
- **Then** a frontmatter block is prepended with the minimal keys and the body is untouched

#### Scenario: Hostile or outside targets

- **Given** a file over the writer size bound, or outside the sandbox, or nonexistent, or with
  corrupt frontmatter
- **When** adoption is attempted
- **Then** the command refuses with the contract's exit code (2 for size/sandbox, 3 for
  not-found or corrupt) and writes nothing

### Requirement: Foreign type values stay opaque

An adopted note keeps whatever `type` value the file already had. List, lens and select surfaces
MUST treat values outside mesh's own note types as opaque labels, never as errors.

#### Scenario: Foreign type survives and lists

- **Given** an adopted note with `type: Team`
- **When** `note list` and the lenses read it
- **Then** it is listed with its foreign type intact and nothing errors

### Requirement: Adoption is in place

Adoption MUST NOT move, rename or reformat a file: paths and filenames stay human. Ids resolve
from the frontmatter, never from the filename, and a foreign filename stem MUST NOT block
id-addressing. The mesh-native classification (status counts, lens seed gate, list surfaces)
MUST become "the frontmatter carries a mesh id" everywhere — an adopted note leaves the foreign
count and enters the mesh-native count the moment it is adopted.

#### Scenario: Foreign stem, id-addressable

- **Given** an adopted note at `Team & Organization/team-sol.md` (stem `team-sol`)
- **When** `mesh note get <its id>` runs
- **Then** the note is found and read; the file's path is unchanged

#### Scenario: Status reclassifies adopted notes

- **Given** a vault with three foreign files, one of which is adopted
- **When** `mesh status` runs
- **Then** the foreign count reads two and the mesh-native note count includes the adopted file

### Requirement: Batch adoption is a sequence of single-entity transactions

Adoption MUST accept multiple paths. Each file is its own atomic transaction; on failure the
command stops, names what already committed, and the re-run heals (idempotent), per the
multi-transaction error-envelope contract.

#### Scenario: Batch stops at first failure and heals

- **Given** a batch of three paths where the second is outside the sandbox
- **When** adoption runs
- **Then** the first is adopted, the command exits 2 naming the committed file, and re-running
  the same batch after the fix adopts the remaining two

### Requirement: Durable ownership is area, not assignment

The system SHALL record the agent whose area a note belongs to in `owner`. Adoption MUST insert
`owner` only when the flag is given **and** the key is absent; changing an area is an explicit
`note update` action, and identity is validated at the write boundary as everywhere else.

#### Scenario: Owner stamped at adoption when absent

- **Given** a foreign file with no `owner` key
- **When** it is adopted with an owner flag
- **Then** `owner` is inserted with the given identity and no other field changes

#### Scenario: Owner changes are explicit

- **Given** an adopted note owned by agent A
- **When** agent identity B updates the note's owner to itself
- **Then** `owner` reads B; the change is one explicit update, never a side effect of a claim

### Requirement: Atomic note claims

The system SHALL provide `note claim` as an atomic test-and-set on `claimed_by`, holding the
entity's lock and re-resolving the target inside it. A claim by a second identity MUST exit 4
with the claim-conflict envelope — the same shape as a task claim conflict, carrying no
`retry_after_ms`; a re-claim by the same identity MUST be a no-op
that reports the state it found and never rewrites. A claim MUST NOT touch `owner` and MUST NOT
add a status field — a claimed note is still just a note.

#### Scenario: Two agents race

- **Given** an unclaimed adopted note
- **When** two identities claim it concurrently
- **Then** exactly one wins; the loser exits 4 with the conflict envelope

#### Scenario: Claim leaves ownership alone

- **Given** an adopted note owned by agent A
- **When** agent B claims it
- **Then** `claimed_by` reads B and `owner` still reads A

### Requirement: Idempotent release

The system SHALL provide `note release` to remove `claimed_by`. It MUST be idempotent, MUST
report the state it found, MUST never rewrite a no-op, and MUST never touch `owner`.

#### Scenario: Release of an unclaimed note

- **Given** a note with no `claimed_by`
- **When** release runs
- **Then** the command reports the found state, exits 0, and the file is byte-identical

### Requirement: Claim visibility on lists

`note list` MUST support the mine filter like every other list verb: `--mine` (or the global
flag) limits results to notes whose `owner` or `claimed_by` is the acting identity.

#### Scenario: Mine-only note list

- **Given** notes owned and claimed by two different agents
- **When** one of them lists notes with the mine filter
- **Then** only its own and claimed notes are listed

### Requirement: Status census sees note ownership and claims

The agents census in `mesh status` MUST extend to note ownership and claims, and any new payload
keys MUST be appended last per the append contract.

#### Scenario: Adopted-and-claimed note appears in status

- **Given** a vault where an agent owns two notes and has claimed one
- **When** `mesh status` runs
- **Then** the agents census reports those counts, and the JSON payload carries the new keys
  only after the existing ones

### Requirement: MCP parity for the new verbs

The MCP surface SHALL expose adopt, claim and release as note tools with explicit annotations
(adopt and release idempotent; claim a write). Removal verbs stay withheld and the tool-count
assertions MUST be updated to the new count.

#### Scenario: Agent adopts over MCP

- **Given** an MCP session against a vault with foreign files
- **When** the agent calls the adopt tool with a path
- **Then** it gets the minted id, and the tool table's annotations and count match the CLI

---

## User Experience

```
mesh note adopt "Team & Organization/team-sol.md" --owner product-analytics-agent
# adopted n-K3ZP  → Team & Organization/team-sol.md

mesh note claim n-K3ZP        # agent working on the note now
mesh note release n-K3ZP      # done with it; the note itself is unchanged
```

Bulk on-ramp with a shell glob: `mesh note adopt Raw/**/*.md`.

---

## Non-Goals

- No note lifecycle: no status, no done/cancelled, no readiness. A claim is work-in-progress
  annotation on a long-lived artifact, not a state machine.
- No automatic adoption: search-visible foreign files stay foreign until the operator adopts
  them. Nothing scans-and-converts behind the operator's back.
- No re-homing, moving, renaming or reformatting of adopted files — they keep their paths.
