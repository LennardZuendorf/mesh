//! The `mesh-mcp` compat shim: `plugins/mesh/.mcp.json` names this binary.

use std::process::ExitCode;

fn main() -> ExitCode {
    mesh::mcp::serve_stdio()
}
