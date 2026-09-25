#!/bin/sh
# A collection absent from the index. `index update <C>` fails exactly the way the shipped
# CLI does — `{"status":"error","error":"Collection 'X' not found"}` on stdout, exit 1 — so
# mesh routes to `index create files ...`, which exits 0 here.
#
# When $INDEXED_ARGV_LOG is set, each invocation appends its argv on one line, so a test can
# pin exactly what mesh asked for on the missing-collection route.
if [ -n "$INDEXED_ARGV_LOG" ]; then
  printf '%s\n' "$*" >> "$INDEXED_ARGV_LOG"
fi
case "$2" in
  update)
    printf '%s' "{\"status\":\"error\",\"error\":\"Collection 'test-vault' not found\"}"
    exit 1
    ;;
esac
exit 0
