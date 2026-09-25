---
type: feature-product
feature: project-envelope
sibling: tech.md
parent: ../../product.md
updated: 2026-09-25
---

# Feature: Project Envelope — Product

A project becomes the workstream envelope: one project note gathering the notes, tasks and
memories that belong to that workstream. `mesh project` grows from "a project note plus its
tasks" to the full envelope, and `--project` scopes search and memory recall to it. The
operator's existing workstream/project files become usable the moment
note-adoption mints them ids — the envelope hangs off a real id,
so foreign workstream notes stop exiting 3.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)

---

## Scope

| | |
|---|---|
| **Owns** | The project lens output extension (notes + memories sections); the `--project` filter on `search` and `memory recall`; the MCP parity for both (mesh_project, mesh_search, mesh_memory_recall params) |
| **Does not own** | Adoption and id minting (note-adoption — upstream gate, DONE and compounded into [tech.md](../../tech.md)); the memory `scope` enum (untouched — membership uses a new optional `project` field, mirroring tasks); the task model (`project` is reused, not redefined); any new verb family — the envelope extends existing surfaces only |

---

## Requirements

### Requirement: Envelope membership has one rule per space

The envelope of a project MUST be: tasks whose `project` names the project id, notes whose
`related` contains the project id, and memories whose `project` names the project id. Memories
gain an optional `project` field (mirroring tasks); the existing `scope` enum is untouched.
Notes join the envelope by wikilinking the project in their body — the existing `related`
mechanics — so no new note field is introduced.

#### Scenario: A wikilink joins a note to the envelope

- **Given** an adopted project note `n-P1` and a note whose body links `[[n-P1]]`
- **When** `mesh project n-P1` runs
- **Then** the note appears in the envelope's notes section

#### Scenario: Memories point at the project

- **Given** a memory created with `project: n-P1`
- **When** `mesh project n-P1` runs
- **Then** the memory appears in the envelope's memories section

### Requirement: Lens output extends by append only

The project lens MUST keep its existing output shape and append the new `notes` and `memories`
sections after the existing keys, per the append contract. The lens stays read-only and keeps
its space filter.

#### Scenario: Machine consumers see stable order

- **Given** a project with tasks and a consumer pinning the lens payload key order
- **When** the envelope sections are added
- **Then** the pre-existing keys appear in the same positions, with the new sections appended
  last

### Requirement: `--project` scopes search

`search` MUST accept a `--project` argument that filters hits to the envelope's members across
spaces. A project id that does not resolve MUST exit 3 with the not-found envelope and
candidates, consistent with the seed gate.

#### Scenario: Scoped search over an envelope

- **Given** an envelope with three notes, two tasks and one memory
- **When** `mesh search --project n-P1 "pricing"` runs
- **Then** only envelope members matching the query are returned, and non-member hits are
  excluded on both engine branches

#### Scenario: Unknown project id

- **Given** no note with id `n-NOPE`
- **When** `mesh search --project n-NOPE "x"` runs
- **Then** the command exits 3 with the not-found envelope

### Requirement: `--project` scopes memory recall

`memory recall` MUST accept the same `--project` argument, filtering to memories whose
`project` names the project id, composing with the existing filters.

#### Scenario: Recall within a workstream

- **Given** two memories about the operator, one whose `project` is `n-P1`
- **When** `mesh memory recall --project n-P1 "preferences"` runs
- **Then** only the project's memory is eligible, ranked by the existing rules

### Requirement: MCP parity

The MCP project tool MUST return the extended envelope, and the search and memory-recall tools
MUST expose the project parameter with the same semantics. The tool count does not change.

#### Scenario: Agent scopes recall over MCP

- **Given** an MCP session
- **When** the agent calls memory recall with the project parameter
- **Then** results match the CLI's scoped recall exactly

### Requirement: The seed gate is unchanged

The lens, search and recall MUST keep requiring a mesh-native project id; a foreign file that
names a workstream stays unaddressable until adopted — exit 3 `seed is not mesh-native`, never
reinterpreted. Adoption is the only on-ramp.

#### Scenario: Foreign workstream note

- **Given** a foreign `type: Project` note with no mesh id
- **When** it is used as a project seed
- **Then** the command exits 3 with the existing mesh-native message

---

## User Experience

```
mesh project n-P1
# the workstream at a glance: what it is, the notes that reference it,
# the tasks pointed at it, the memories scoped to it

mesh search --project n-P1 "traveler complaints"
mesh memory recall --project n-P1 "pricing model"
```

The operator's retired "workstream" concept maps onto mesh's project: a workstream **is** a
project note; the lens treats them identically.

---

## Non-Goals

- No new verb family, no sixth space, no `project create` scaffolding verb — the envelope extends
  existing surfaces.
- No hierarchy: projects do not nest; a note or memory belongs via its one reference, and
  anything else stays outside the envelope.
- No envelope writes: membership changes by writing the entity's own reference (wikilink,
  `project`, `scope`), never by a verb that edits other entities.
