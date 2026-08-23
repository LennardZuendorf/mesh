# Lessons

Mistakes made and rules to prevent repeating them. Review at the start of every session.
Tags make entries retrievable — scan for tags matching the work in hand.

<!-- Format for each lesson:
### [Short description]
**Pattern:** What went wrong and why
**Rule:** The concrete rule that prevents this
**Tags:** comma, separated, keywords
**Date:** YYYY-MM-DD
-->

### A green gate is not a complete feature
**Pattern:** pytest + mypy + ruff all passed while `shards daemon start` (admin unit) and the MCP server were entirely unbuilt, and while `search --status` was silently dropped on the hybrid path — because no test exercised the missing behaviour. A green gate only proves the tests that exist pass.
**Rule:** Verify features against the spec's requirement list and by running the assembled binary end-to-end, not just by gate colour. Cross-check units-built vs required surface before declaring done; a missing verb/flag has no failing test to turn the gate red.
**Tags:** verification, testing, cli, done-definition
**Date:** 2026-07-04

### Daemon-down fallback must catch the whole transport-failure surface
**Pattern:** the socket client caught only `ConnectionRefusedError`/`FileNotFoundError`, so a hung daemon (`TimeoutError`), a mid-response crash (`BrokenPipeError`/`ConnectionResetError`), or a truncated reply (`json.JSONDecodeError`) escaped to the CLI — breaking the "fallback on every RPC path" contract exactly when the daemon misbehaves.
**Rule:** An accelerator's degrade path must catch every way the accelerator can fail, not just "absent". Catch `(OSError, json.JSONDecodeError)` and let genuine domain errors propagate. Also treat a live daemon's `503` (reserved/unwired method) as fallback-eligible, or the client's bound fallback never fires.
**Tags:** daemon, fallback, resilience, rpc
**Date:** 2026-07-04

### An exception in a watcher thread silently kills freshness
**Pattern:** an unguarded `os.replace` inside the watchdog event handler could raise `FileNotFoundError` on a raced source-vanish; watchdog swallows nothing useful, so the observer thread dies and folder-reconcile + index freshness freeze for the daemon's whole life — with no error surfaced.
**Rule:** Never let an exception escape a watchdog/observer callback. Guard filesystem ops (`try/except OSError: return`) so a transient race degrades one event, not the whole watcher.
**Tags:** daemon, watcher, threads, resilience
**Date:** 2026-07-04

### Foreign-file tolerance must be symmetric across every reader
**Pattern:** the task-side readers caught `yaml.YAMLError` around `frontmatter.loads`, but the note-side readers (`get_note`, `list_notes`, slug-resolve, `wikilinks._title_index`/`find_dangling`) caught only `OSError` — so one malformed `.md` in `notes/` crashed `note list/get`, slug ops, `status`, and the wikilink write-path, flatly contradicting the "foreign/corrupt files skip silently" contract the sibling task code honoured.
**Rule:** "Skip foreign/corrupt files silently" is a whole-corpus invariant — it must hold in *every* reader, not most. Route all read-parse through one guarded reader that catches both `OSError` and `yaml.YAMLError`; never hand-roll a second, narrower guard.
**Tags:** parsing, resilience, notes, tasks, dry
**Date:** 2026-07-04

### Reclaiming a stale lock must not unlink a lock you no longer own
**Pattern:** stale-lock reclaim did `_is_stale()` (stat+read) → `_clear()` (unconditional `unlink`) → re-create. Two acquirers could both judge a dead-PID lock stale; the second's `unlink` removed the *fresh* lock the first had just taken, yielding two simultaneous holders and breaking the `O_EXCL` atomic test-and-set.
**Rule:** A reclaim is a compare-and-swap, not a blind delete. Re-attempt the `O_EXCL` create and treat `FileExistsError` as "someone won" rather than unlinking whatever is at the path; only remove a lock whose identity you have re-verified.
**Tags:** locks, concurrency, toctou, atomicity
**Date:** 2026-07-04

### Write-then-move must survive a crash without an unrecoverable state
**Pattern:** `finish`/`cancel` wrote `status=done` in place, then `os.replace`d the file `open/`→`done/`. A crash between the two left a `done`-status file in `tasks/open/`; the idempotent terminal no-op then refused to move it, stranding it there forever.
**Rule:** When a state transition spans two atomic steps, order and recover them so no crash point yields a state the normal path can't repair. Either move into the destination first, or have the idempotent path reconcile a mismatched status↔folder instead of short-circuiting on status alone.
**Tags:** atomicity, crash-consistency, tasks, idempotency
**Date:** 2026-07-04

### Resolve user paths — expanduser() every config path
**Pattern:** `SHARDS_CONFIG_PATH` was `expanduser()`'d but `[core].tolaria_path` was not, and `realpath` does not expand `~`. A natural `path = "~/vault"` became a literal `./~/vault` under the process CWD — silently writing the whole vault to the wrong place and rooting the sandbox there.
**Rule:** Every path that comes from a human-authored config or env var gets `expanduser()` at the parse boundary (a pydantic field validator), consistently across *all* path fields. `realpath`/`resolve` is not a substitute for `~` expansion.
**Tags:** config, paths, sandbox
**Date:** 2026-07-04

### Instant CLI: import heavy and daemon-only deps lazily
**Pattern:** `index/watch.py` imported `watchdog.observers` at module top, and the CLI entrypoint pulled `watch` in transitively — so every `shards note new` / `task claim` loaded watchdog and its fsevents C-extension though only the daemon process ever instantiates an `Observer`, taxing the "instant CLI, heavy work in the daemon" mandate on every invocation.
**Rule:** Keep daemon-only / heavy imports out of any module on the CLI import path. Import them lazily inside the function that needs them (`Watcher.start()`), so command startup pays only for what it uses.
**Tags:** cli, startup, performance, imports
**Date:** 2026-07-04

### One mechanic, one home — and read from the warm index
**Pattern:** the lock-hold CM, safe-read idiom, vault walk, filter/sort/limit tail, note/task→JSON envelope, and the whole `finish`≈`cancel` body were copy-pasted across `notes.py`/`tasks.py` and the CLI/MCP/daemon surfaces; separately, tag-pull and recent-activity re-walked and re-parsed the whole vault on every call even when the daemon's warm index already held that metadata in RAM. Copies drift (the YAML-guard and exit-code bugs were exactly this), and the redundant disk scans fought invariant 7.
**Rule:** For a two-verb core, each cross-cutting mechanic gets exactly one implementation both verbs import; a second copy is a bug waiting to diverge. And when the daemon is up, serve read-lenses from the warm index — don't re-parse from disk what the index already holds.
**Tags:** kiss, dry, duplication, performance, daemon
**Date:** 2026-07-04

### DRY only when the callers share one shape — not when the abstraction carries their differences
**Pattern:** a Phase-1 review proposed three "shared primitives" that scoping then rejected: a `to_entry` wire-envelope merger (the socket wants the full model + body; the activity row is a deliberate 5-key shape that *excludes* `created`/`updated`), a generic filter/sort/limit pipeline (`recent` sorts two-pass mtime-desc/id-asc with a `-1` sentinel; `list_notes` sorts single-pass with a `None` sentinel), and an exit-code decorator (each handler's message carries the target id / conflict owner). Each "shared" helper would have needed a per-caller discriminator, key-callable, or message threaded in — so the abstraction re-encoded the very differences it claimed to erase, costing more than the copies. (The genuinely-shared `cli/_output.emit_mutation` worked precisely because the only difference — `type` vs `status` — collapses to a one-key `fields` dict.)
**Rule:** Extract a shared primitive only when the callers truly share one shape. If unifying them forces a discriminator, a strategy callable, or per-caller branches into the helper, the duplication is the simpler design — keep it. This refines "one mechanic, one home": one mechanic, yes; one *forced* shape, no.
**Tags:** kiss, dry, duplication, abstraction, review
**Date:** 2026-07-05

### A scope claim must cover the process boundary, not just in-process walks
**Pattern:** an analysis enumerated five in-process vault walks, concluded "shards only ever touches `notes/` and `tasks/`", and shipped that claim in `config.example.toml`. It missed `src/shards/index/indexed_client.py:309`, where `full_rebuild()` hands the *whole configured root* to the external `indexed` binary — a traversal that happens outside the process the walks were enumerated in.
**Rule:** When claiming what a tool traverses, enumerate every path that escapes the process — subprocess arguments, external indexers, watchers — not only the in-process iterators.
**Tags:** scope, spec-accuracy, subprocess, vault, indexed
**Date:** 2026-08-20

### Removing a dependency by name leaves the premise behind
**Pattern:** the Tolaria removal was scoped by grepping the string "tolaria", so rationales *derived* from it survived where the word did not — e.g. `.spec/features/team-awareness/product.md` still rejected an `updated_by` key because "the git-backed vault is the audit trail", the exact premise the removal invalidated.
**Rule:** After removing a dependency, grep for the *premises* it supplied — the guarantees other decisions leaned on — not just its name.
**Tags:** spec-accuracy, dependency-removal, grep-scoping, vault-agnostic
**Date:** 2026-08-20

### A per-user accelerator silently serves the wrong vault
**Pattern:** the daemon socket was named from `$XDG_RUNTIME_DIR`/`$HOME` and the RPC envelope carried no vault identity, so there was one daemon per *user* rather than per *vault*. A CLI configured for vault B and a daemon started on vault A talked happily: `note list` returned A's rows, `recent-activity` leaked A's absolute paths out of B's sandbox, and `note get` on those ids said not found. Four independent reviewers hit it — three of them as "flaky tests" first, because the suite pinned no runtime dir either.
**Rule:** an accelerator keyed to a *user* while the data is keyed to a *vault* is a correctness bug, not a config nuisance. Name the socket from a digest of the resolved data root, and have every reply state which root it served so a mismatch degrades to the fallback. Pin the runtime dir in the test suite too, or "cold path" tests are only cold by accident of the environment.
**Tags:** daemon, socket, multi-vault, isolation, test-isolation
**Date:** 2026-08-23

### Freshness that depends on a watcher has no read-your-writes
**Pattern:** the warm index was refreshed only by inotify, so `create` followed immediately by `list` — the commonest agent sequence there is — returned a list without the agent's own new entity (5/5 creations missed). The parity suite never caught it because it only ever tested a *frozen* vault seeded before the daemon started; every divergence found later lived in exactly that untested region.
**Rule:** "the accelerator is invisible" is a claim about a vault that *changes*, so test it against one that does — mutate while the daemon is up, then re-compare. When a writer knows what it changed, tell the accelerator rather than waiting for the filesystem to mention it; keep the notification post-durability and failure-swallowing so the accelerator still never gates a write.
**Tags:** daemon, freshness, read-your-writes, parity, testing
**Date:** 2026-08-23

### A default written into a config file is not the same as a default in code
**Pattern:** the substring fallback was fixed to apply `[search].threshold` only when a caller set it explicitly — then `shards init` wrote `threshold = 0.65` into every config it generated, which made it explicit and restored the exact behaviour the fix removed. Body search returned `[]` on every fresh install. The unit's own test passed because its fixture hand-wrote a config shape `init` cannot produce.
**Rule:** when behaviour keys off "did the user set this?", the tool's own scaffolding must not answer yes on the user's behalf. Omit defaulted keys from generated config, and write the regression test *through* the generator rather than against a hand-built fixture.
**Tags:** config, defaults, init, search, test-fixtures
**Date:** 2026-08-23

### Fix one side of a shared mechanic and the other side is now the bug
**Pattern:** the task verbs were hardened to resolve the entity path *inside* the lock, closing a TOCTOU against a racing folder move. The note verbs were left resolving outside it, so `note delete` racing `note update --type` raised a raw `FileNotFoundError` at exit 1 and did not delete. The same asymmetry appeared twice more in one review: the dangling-link scan walked `tasks/**` recursively while the task reader was deliberately non-recursive, and `_iso_z` was applied to the JSON surfaces but not the human `get` output.
**Rule:** when a repo's thesis is "a task is a note", a fix to one verb family is unfinished until the sibling family is checked. Grep for the *pattern* being fixed, not the symptom that reported it.
**Tags:** dry, symmetry, toctou, notes, tasks, review-scoping
**Date:** 2026-08-23

### A guard nothing tests is a guard that will be deleted
**Pattern:** every defensive guard in `index/reconcile.py` could be removed with all 1331 tests still green — including the one its own docstring calls out as "never remove that guard", citing an existing lesson. The watcher had no try/except around its callback either, so a single malformed `.md` would kill the observer thread and freeze freshness for the daemon's whole life, silently, with the suite green.
**Rule:** a comment saying a guard is load-bearing is an admission that nothing proves it. Pin each guard with a test that exercises the hostile input and asserts the *system survives* — not just that the call returned. Statement coverage cannot see a deleted guard clause; turn branch mode on.
**Tags:** testing, mutation-testing, guards, watcher, coverage
**Date:** 2026-08-23

### Serialisation defaults leak into files other tools have to read
**Pattern:** binding one `datetime` object to both `created` and `updated` made PyYAML emit an anchor/alias pair (`created: &id001 …` / `updated: *id001`) in every file shards created. Valid YAML, and shards round-tripped it fine — but a restricted frontmatter parser reads `updated` as the literal string `*id001`, so the breakage landed entirely on the coexisting tool the spec promises to be safe with. `atomic_write` had the same shape of bug: `mkstemp` creates 0600 and `os.replace` carried that onto a file another tool had checked in at 0644.
**Rule:** "the Markdown stays clean" is a claim about what *other* programs can read, so audit the bytes on disk, not the round-trip through your own reader. Enforce it at the single serialisation boundary — a dumper that cannot emit anchors, a write that preserves the destination's mode — so no caller can reintroduce it.
**Tags:** markdown, yaml, interop, permissions, atomicity
**Date:** 2026-08-23
