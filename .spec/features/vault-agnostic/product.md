---
type: feature-product
feature: vault-agnostic
sibling: tech.md
parent: ../../product.md
updated: 2026-08-18
---

# Feature: Vault-Agnostic — Product

Shards is documented and named as if it sits on top of Tolaria: the vault config key is
`tolaria_path`, the root specs hand Tolaria ownership of the vault and Git, the shipped plugin
skill lists a "configured Tolaria vault" as an install prerequisite, and the hard-delete decision
is justified by Tolaria's git-backed vault being the recovery path. None of that is true in code —
there is no Tolaria package, import, subprocess, or MCP call anywhere, and `shards init` already
creates its own vault. This feature closes the gap in the *stated* contract: shards works over
**any** Markdown folder, Obsidian included, and nothing shipped implies otherwise.

**Parent:** [../../product.md](../../product.md)
**Architecture:** [tech.md](tech.md)
**Plan:** [plan.md](plan.md)

---

## Scope

| | |
|---|---|
| **Owns** | The vault-path config key and its aliases; every user- and agent-facing string that names Tolaria (CLI help, error messages, MCP instructions block, package description, plugin/marketplace metadata, shipped `SKILL.md`); the delete-policy and foreign-file rationales in `AGENTS.md` and root `.spec/`; example agent identities and foreign-frontmatter fixture keys. |
| **Does not own** | The folder layout (`notes/`, `tasks/{open,done}/`) — unchanged. The `indexed` search dependency — unrelated and stays. The three-verb surface — no verbs added or removed. Repo (`mesh`) and package (`shards`) names — out of scope. Delete *behaviour* — the policy is re-justified, not changed. Frozen historical records (`.spec/lessons.md` dated entries, `.spec/features/shards-rebrand/`) — superseded in place by new statements, never rewritten. |

---

## Requirements

### Requirement: Tool-neutral vault key

The config key naming the Markdown folder MUST be tool-neutral. `[core].vault_path` SHALL be the
canonical spelling. Both prior spellings — `[core].tolaria_path` and the existing `[core].path`
alias — MUST continue to load without warning or error, so no existing config file needs editing.

#### Scenario: New canonical key loads

- **Given** a config whose `[core]` table sets `vault_path`
- **When** config is loaded
- **Then** the vault root resolves to that value, with `~` expanded

#### Scenario: Legacy key still loads

- **Given** a config whose `[core]` table sets `tolaria_path` and no `vault_path`
- **When** config is loaded
- **Then** the vault root resolves to that value, identically to the canonical spelling, and no error or warning is emitted

#### Scenario: Generated config uses the canonical key

- **Given** a machine with no shards config
- **When** the operator runs `shards init`
- **Then** the written config file spells the key `vault_path`, and re-loading that file succeeds

#### Scenario: Missing config names the canonical key

- **Given** no config file exists
- **When** any command runs
- **Then** the error names `[core].vault_path` and does not mention Tolaria

### Requirement: No shipped artifact names Tolaria as a dependency

No artifact that reaches a user or an agent at runtime SHALL describe Tolaria as required,
inherited, or coexisting-by-design. This MUST hold for the MCP instructions block, MCP and CLI
error messages, CLI help text, the distribution description, and the plugin bundle's marketplace
entry, plugin manifest, and skill metadata.

#### Scenario: MCP surface is tool-neutral

- **Given** an agent connects to the shards MCP server
- **When** it reads the server instructions block
- **Then** the block describes a Markdown vault without naming Tolaria

#### Scenario: Plugin metadata states no prerequisite

- **Given** a user browses the shards plugin in a marketplace listing
- **When** they read the description and the skill's compatibility line
- **Then** the stated prerequisite is the shards CLI or MCP server plus a configured vault folder — not any particular notes application

### Requirement: Delete policy stands on its own rationale

The hard-`unlink` delete policy MUST be justified without assuming a git-backed vault. The specs
SHALL state that versioning and backup are the vault owner's responsibility, and that shards is
not a backup tool. Delete behaviour itself does not change.

#### Scenario: Rationale survives a non-versioned vault

- **Given** a reader whose vault is a plain folder with no version control
- **When** they read why shards has no trash
- **Then** the stated reason holds for their setup, and the responsibility for recovery is explicitly theirs

### Requirement: Coexistence is stated generically

Foreign-file tolerance and unknown-frontmatter round-tripping MUST be described as applying to any
other tool writing into the same folder, not to Tolaria specifically. The behaviour is already
generic; only its description changes.

#### Scenario: Foreign file from any tool

- **Given** a vault containing a `.md` file written by another tool, carrying no shards id and unrecognised frontmatter keys
- **When** shards lists, searches, and re-writes around it
- **Then** the file is skipped by id-scoped reads and its unknown keys survive untouched — as the specs describe for any foreign writer

### Requirement: Obsidian is an example, never a dependency

Obsidian MAY be named as the reference pairing in the README and the example config comments. It
MUST NOT appear in the product contract, the MCP instructions block, plugin metadata, or any code
path. No Obsidian-specific behaviour SHALL be added.

#### Scenario: Obsidian vault needs no special handling

- **Given** the vault path points at an Obsidian vault root containing `.obsidian/` and a root `.trash/`
- **When** shards walks the vault
- **Then** only `notes/` and `tasks/` are traversed, and no Obsidian-specific configuration was required

### Requirement: Root specs corrected, history preserved

Root `.spec/product.md`, `.spec/tech.md`, and `AGENTS.md` MUST NOT contain statements that are
false without Tolaria. Dated lesson entries and completed feature specs MUST be left byte-intact;
where they are superseded, the superseding statement lives in the current layer.

#### Scenario: No false ownership claim remains

- **Given** a reader of the root specs
- **When** they look for who owns the vault and Git
- **Then** they find the vault owner (the operator and whatever tooling they choose) — not a claim that Tolaria owns it

#### Scenario: History is not rewritten

- **Given** the dated lesson citing `[core].tolaria_path` and the `shards-rebrand` feature spec recording "Tolaria coupling unchanged"
- **When** this feature completes
- **Then** both still read exactly as written when they were recorded

---

## Outputs

- A config surface whose canonical key is `[core].vault_path`, accepting two legacy spellings.
- Shipped artifacts (MCP instructions, plugin bundle, distribution metadata, help, errors) with no Tolaria reference.
- Root `.spec/product.md`, `.spec/tech.md`, `AGENTS.md`, and `README.md` stating a tool-neutral contract with a self-standing delete rationale.

---

## Non-Goals

- No change to delete, claim, or any other behaviour — this feature changes names, strings, and stated rationale only.
- No Obsidian-specific code, ignore rules, or vault detection.
- No new config migration tooling — the alias chain is the migration.
- No rename of the repo, the package, or the `indexed` search dependency.

---

## Open Questions

None. The three decisions this feature turned on — canonical key spelling with aliases, keep hard
delete and re-justify, Obsidian named once as reference pairing — were settled with the maintainer
before the spec was written.
