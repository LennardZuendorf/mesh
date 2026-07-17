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
