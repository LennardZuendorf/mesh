#!/usr/bin/env bash
#
# Install the `mesh` and `mesh-mcp` binaries from this checkout.
#
#   ./scripts/install.sh              # into ~/.cargo/bin
#   ./scripts/install.sh --root /usr/local
#
# Every argument is forwarded to `cargo install`. Requires the toolchain pinned in
# rust-toolchain.toml (Rust 1.94+).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
  echo "install: cargo not found — install Rust 1.94+ from https://rustup.rs" >&2
  exit 1
fi

cargo install --path "$ROOT" --locked "$@"

echo
echo "installed: mesh, mesh-mcp"
echo "next: mesh init   (writes ~/.mesh/config.toml, or \$MESH_CONFIG_PATH when set)"
