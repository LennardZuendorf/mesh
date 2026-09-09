#!/bin/sh
# A reachable `indexed` that fails at runtime: the gates all read healthy, yet the query must
# still degrade to the built-in engine and report mode "fallback".
exit 3
