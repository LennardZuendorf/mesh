#!/usr/bin/env bash
#
# mesh smoke test — ~35 verbs against a throwaway vault, asserting exit codes only.
#
#   ./scripts/smoke.sh                 # uses ./target/release/mesh
#   MESH_BIN=./target/debug/mesh ./scripts/smoke.sh
#
# Every step prints PASS or FAIL with the code it got; the script exits non-zero if any
# step failed. While a verb is still a stub it reports `not implemented: …` at exit 2, so a
# run against a partial build is expected to fail — the failures name exactly what is left.

set -uo pipefail

BIN="${MESH_BIN:-./target/release/mesh}"
if [ ! -x "$BIN" ]; then
  resolved="$(command -v "$BIN" 2>/dev/null || true)"
  BIN="$resolved"
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "smoke: no mesh binary — run 'cargo build --release' or set \$MESH_BIN" >&2
  exit 2
fi
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
VAULT="$WORK/vault"
CONFIG="$WORK/config.toml"
BLOB="$WORK/attachment.txt"
printf 'a smoke-test attachment\n' > "$BLOB"

# The vault under test must never inherit an operator's real environment.
unset MESH_CONFIG_PATH MESH_AGENT MESH_VAULT MESH_INDEXED_BIN

PASSED=0
FAILED=0
STEP=0

# step <label> <expected-exit> <mesh args…>
step() {
  local label="$1" want="$2"
  shift 2
  STEP=$((STEP + 1))
  local out code
  out="$("$BIN" --config "$CONFIG" "$@" 2>&1)"
  code=$?
  if [ "$code" -eq "$want" ]; then
    PASSED=$((PASSED + 1))
    printf 'PASS  %2d  %-34s exit %d\n' "$STEP" "$label" "$code"
  else
    FAILED=$((FAILED + 1))
    printf 'FAIL  %2d  %-34s exit %d (want %d)\n' "$STEP" "$label" "$code" "$want"
    printf '          %s\n' "$(printf '%s' "$out" | head -3 | tr '\n' '|')"
  fi
}

# capture <mesh args…> — the quiet stdout of a command, or "" when it failed.
capture() {
  "$BIN" --config "$CONFIG" --quiet "$@" 2>/dev/null | head -1
}

echo "smoke: $BIN"
echo "smoke: vault $VAULT"
echo

# ---- admin ---------------------------------------------------------------------------------
step "--version"              0 --version
step "init"                   0 init --path "$VAULT" --agent smoke --collections "smoke, peer"
step "init refuses"           2 init --path "$VAULT"
step "init --force"           0 init --path "$VAULT" --agent smoke --collections "smoke, peer" --force
step "config path"            0 config path
step "config show"            0 config show
step "config show --json"     0 --json config show
step "config get"             0 config get core.agent
step "config set"             0 config set search.hybrid false
step "completions bash"       0 completions bash
step "status"                 0 status
step "status --json"          0 --json status
step "reindex"                0 reindex
step "daemon status"          0 daemon status
step "watch --once"           0 watch --once --no-index

# ---- notes ---------------------------------------------------------------------------------
step "note new"               0 note new "Alpha note" --body "hello from smoke"
NOTE_ID="$(capture note new "Beta note" --body "second body")"
NOTE_ID="${NOTE_ID:-n-MISSING}"
step "note list"              0 note list
step "note list --json"       0 --json note list
step "note get"               0 note get "$NOTE_ID"
step "note append"            0 note append "$NOTE_ID" "an appended line"
step "note update"            0 note update "$NOTE_ID" --tags "+smoke"
step "note delete"            0 note delete "$NOTE_ID" --force

# ---- tasks ---------------------------------------------------------------------------------
TASK_ID="$(capture task new "Ship the smoke test" --owner smoke)"
TASK_ID="${TASK_ID:-t-MISSING}"
step "task new"               0 task new "A second task" --owner smoke
step "task list"              0 task list
step "task claim"             0 --owner smoke task claim "$TASK_ID"
step "task append"            0 task append "$TASK_ID" "progress"
step "task get"               0 task get "$TASK_ID"
step "task finish"            0 --owner smoke task finish "$TASK_ID" --outcome "done"
step "task next"              0 task next

# ---- memories, scratch, assets --------------------------------------------------------------
MEM_ID="$(capture memory new "Prefers dark mode" --body "observed" --kind fact)"
MEM_ID="${MEM_ID:-m-MISSING}"
step "memory list"            0 memory list
step "memory recall"          0 memory recall "dark"
step "memory forget"          0 memory forget "$MEM_ID" --force
step "scratch set"            0 scratch set smoke-state --body "working"
step "scratch get"            0 scratch get smoke-state
step "scratch list"           0 scratch list
step "scratch clear"          0 scratch clear smoke-state --force
step "asset add"              0 asset add "$BLOB"
step "asset list"             0 asset list
step "asset gc"               0 asset gc

# ---- search and lenses ----------------------------------------------------------------------
step "search"                 0 search "smoke"
step "search --tags"          0 search --tags smoke
step "search --health"        0 search --health
step "recent-activity"        0 recent-activity
step "session-start"          0 session-start

echo
echo "smoke: $PASSED passed, $FAILED failed, $STEP total"
[ "$FAILED" -eq 0 ]
