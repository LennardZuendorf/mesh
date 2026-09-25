---
type: feature-tech
feature: note-adoption
sibling: product.md
parent: ../../tech.md
updated: 2026-09-25
---

# Feature: Note Adoption & Claims — Architecture

Adoption is a new write path assembled entirely from existing primitives: the one frontmatter
reader/emitter, the one id minter, the one lock module, the one atomic write. Claims are the task
claim machinery minus the lifecycle. The one deep change is in the read model: **mesh-native
becomes "the frontmatter carries a mesh id", decoupled from the `n-`-stem the namer mints on
create** — foreign-stemmed, id-bearing files must list, resolve and seed everywhere.

**Parent:** [../../tech.md](../../tech.md)
**Requirements:** [product.md](product.md)

---

## Files

```
src/domain/notes.rs        # adopt(), note claim/release; meta-id resolution; classification
src/model/note.rs         # from_meta read tolerance; claimed_by in the view + field order
src/cli/note.rs            # Adopt/Claim/Release subcommands; --owner; --mine on list
src/cli/mod.rs             # NoteSub variants
src/domain/owner.rs       # owner validation reused as-is
src/domain/lenses.rs      # status census: note ownership/claims, keys appended last
src/mcp/mod.rs            # TOOL_NAMES 37 → 40
src/mcp/schema.rs         # 3 ToolDefs + annotations
src/mcp/tools.rs          # dispatch arms
plugins/mesh/skills/mesh/SKILL.md  # playbook surface (pinned by tests/bundle.rs)
tests/note_cli.rs tests/race.rs tests/mcp_cli.rs tests/admin_cli.rs tests/lens_cli.rs
```

---

## Contract / API

```rust
// src/domain/notes.rs
pub fn adopt(cfg: &Cfg, paths: &[PathBuf], owner: Option<&str>) -> Result<AdoptReport>;
pub fn claim(cfg: &Cfg, id: &str, claimer: &str) -> Result<Note>;      // no `strict`: no deps on notes
pub fn release(cfg: &Cfg, id: &str, releaser: &str, force: bool) -> Result<Note>;
```

### Adoption

- **Sandbox**: each path `safe_resolve`d against the enabled notes root(s) — in-vault only, no
  copy-in (the `asset add` source precedent is deliberately rejected: adopted files keep their
  homes). A path inside a **nested space root** (e.g. the tasks folder inside a vault-root notes
  space) is not a note — exit 2. Outside the sandbox — exit 2. Missing — exit 3.
- **Frontmatter discrimination** (this is the safety core): read the raw file and
  `split_frontmatter`. No delimited block → prepend a fresh one (`Doc::new` + `write_doc`; the
  machinery already handles frontmatter-less files). A delimited block that fails to parse →
  **exit 3 corrupt** — a broken block must never be pushed into the body. A parsed meta carrying
  a mesh id → report it, **no write** (byte-identical no-op). A parsed meta carrying a
  non-mesh `id` key → exit 2 (one id per file).
- **Injection set**: `id` (always); `created`, `updated`, `title`, `owner` only when absent —
  `owner` additionally only when the flag is given. `created`/`updated` stamp adoption time;
  an existing `created` is reused as the digest input and never overwritten. `title` falls back
  to the existing `derived_title` (first H1, else stem; notes.rs already has it).
- **Id minting**: `generate_id("n", created_iso, title, exists)` — the `exists` closure checks
  **frontmatter-id membership** from a walk, not stem membership (foreign stems stay foreign).
  Collision extension unchanged.
- **Locking**: the notes space's `create_lock` held across mint + write (mirrors `note new`),
  with the idempotency check **recomputed inside the lock** (§ Invariants 6). Batch = sequential
  single-entity transactions; on failure the envelope names what committed and the healing
  command, per the multi-transaction contract.
- **No restamp**: adoption never touches the body, never recomputes `related`.

### Claims

`claim` mirrors `tasks::claim` minus status/deps: resolve by meta id → `hold(entity_lock(notes_root,
id))` → **re-resolve inside the lock** → four-branch test-and-set on `claimed_by` only. Conflict
= `MeshError::ClaimConflict` (exit 4) with the **same envelope shape as a task claim conflict**
(`kind: claim_conflict` — deliberately no `retry_after_ms`; task claim conflicts don't carry one
either). Same-identity re-claim: no-op reporting the found state. `release` mirrors
`tasks::release`: idempotent, `--force` to override another holder, `claimed_by` emitted as
`null` when cleared, never a rewrite on a no-op. Identity validated through the existing
`validate_owner` / `[tasks].collections` boundary — notes already route through it.

### Read-model widening (the load-bearing change)

- `Note::from_meta` (model/note.rs) **stops rejecting foreign `type` values** — `type` reads as
  an opaque scalar; write-side validation (`validate_type` for `note new`/`update --type` input)
  is unchanged. This is what makes an adopted `type: Project` file listable.
- `claimed_by` joins the notes model, declared **after `owner`** (matching the task layout);
  absent on notes that were never claimed, `null` after release.
- **Mesh-native unification**: every classification surface (`notes::foreign_count`, the seed
  gate, list corpora) answers "does the frontmatter carry a mesh id". `is_mesh_stem` survives as
  the **namer-inverse** (which files mesh would mint / delete safety), and `note delete` extends
  to foreign-stem files **when the entity resolved by meta id** — adoption is the operator's
  explicit opt-in, the same rationale § Invariants 11 gives for the asset namer-inverse.

<!-- merge -->
### Mesh-native is an id, not a filename

A mesh note is a file whose frontmatter carries a mesh id. The `n-` stem the namer mints on
create remains the mint/delete-safety test (`is_mesh_stem`), but addressability, seed gating
and foreign counts key on the id key, so an adopted foreign-stem file is mesh-native without
losing its path.
<!-- /merge -->

<!-- merge -->
### Foreign files are adoptable by the operator's explicit verb

The root contract "foreign files are never mutated" gains its one sanctioned exception:
`note adopt`, which inserts the minimal mesh block and preserves everything else. Nothing else
mutates a foreign file, and adoption is never automatic.
<!-- /merge -->

---

## Implementation Detail

- Resolution by meta id: the id lookup walks rows and matches the frontmatter `id` (the rows
  scan already exists; only the matching key changes). Slug resolution keeps stem/title
  matching and must not match foreign-stem files that are already adopted (they resolve by id).
- `--mine` on `note list` routes through `ctx.coalesce_mine` and widens the owner filter to
  owner-**or**-claimed_by (mirroring task list).
- Status census: per-agent `notes_owned` / `notes_claimed` lines in the human block; JSON payload
  keys **appended last** (the `notes_foreign` precedent).
- MCP annotations: adopt **idempotent** (write that is safe to re-run), claim **write**, release
  **idempotent**. Both tool-count pins (`tests/mcp_cli.rs`, `src/mcp/mod.rs` test) move 37 → 40,
  and `plugins/mesh/skills/mesh/SKILL.md` lists the three tools (pinned by `tests/bundle.rs`).

---

## Performance Budget

Adoption is a walk + read + one atomic write per file; bulk adoption of ~650 files is seconds,
not minutes — no index, no batching machinery. Claims are the same cost as a task claim.

---

## Open Questions

1. **Adopted notes and `--type` updates** — `note update --type` on an adopted note engages the
   existing folder-routing move. Recommendation: leave the existing behaviour standing
   (explicit update = operator's choice); adoption itself never moves anything.
