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
**Pattern:** pytest + mypy + ruff all passed while `brain daemon start` (admin unit) and the MCP server were entirely unbuilt, and while `search --status` was silently dropped on the hybrid path — because no test exercised the missing behaviour. A green gate only proves the tests that exist pass.
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
