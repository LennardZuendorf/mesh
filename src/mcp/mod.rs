// STUB: owned by agent 8 (mcp). `TOOL_NAMES` and `DESTRUCTIVE_TOOLS` are real and frozen.
//! The stdio MCP server.

pub mod errors;
pub mod instructions;
pub mod proto;
pub mod schema;
pub mod tools;

use crate::config::Config;
use crate::error::MeshError;

/// The 37 registered tools, in registration order (final.md §10).
pub const TOOL_NAMES: [&str; 37] = [
    "mesh_note_get",
    "mesh_note_list",
    "mesh_task_get",
    "mesh_task_list",
    "mesh_search",
    "mesh_health",
    "mesh_recent_activity",
    "mesh_build_context",
    "mesh_graph",
    "mesh_project",
    "mesh_session_start",
    "mesh_note_new",
    "mesh_note_append",
    "mesh_task_new",
    "mesh_task_append",
    "mesh_note_update",
    "mesh_task_claim",
    "mesh_task_release",
    "mesh_task_finish",
    "mesh_task_update",
    "mesh_task_cancel",
    "mesh_memory_new",
    "mesh_memory_append",
    "mesh_memory_update",
    "mesh_memory_get",
    "mesh_memory_list",
    "mesh_memory_recall",
    "mesh_scratch_set",
    "mesh_scratch_append",
    "mesh_scratch_get",
    "mesh_scratch_list",
    "mesh_asset_get",
    "mesh_asset_list",
    "mesh_asset_attach",
    "mesh_task_block",
    "mesh_task_unblock",
    "mesh_task_next",
];

/// The only tool carrying `destructiveHint: true`. Pinned by the plugin-bundle test.
pub const DESTRUCTIVE_TOOLS: [&str; 1] = ["mesh_task_cancel"];

/// The MCP protocol version this server speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Serve the stdio MCP loop and report the process exit status.
pub fn serve_stdio() -> std::process::ExitCode {
    std::process::ExitCode::from(u8::try_from(serve_stdio_code()).unwrap_or(1))
}

/// The same loop, as an `i32` the CLI dispatcher can return.
pub fn serve_stdio_code() -> i32 {
    eprintln!("not implemented: mcp");
    2
}

/// The instructions block, built once at startup. Pure, and at most 2048 UTF-8 bytes.
pub fn build_instructions(_cfg: Option<&Config>) -> String {
    String::new()
}

/// The JSON error payload an MCP `ToolError` carries as its message.
pub fn tool_error(e: &MeshError) -> String {
    serde_json::to_string(&crate::cli::out::error_envelope(e))
        .unwrap_or_else(|_| "{\"kind\":\"error\"}".to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_table_is_thirty_seven_unique_names() {
        let mut sorted = TOOL_NAMES.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 37);
        assert_eq!(TOOL_NAMES.first(), Some(&"mesh_note_get"));
        assert_eq!(TOOL_NAMES.last(), Some(&"mesh_task_next"));
    }

    #[test]
    fn no_tool_name_contains_a_withheld_substring() {
        for name in TOOL_NAMES {
            for banned in [
                "delete", "daemon", "reindex", "status", "forget", "remove", "clear",
            ] {
                assert!(!name.contains(banned), "{name} contains {banned}");
            }
        }
    }

    #[test]
    fn destructive_is_exactly_task_cancel() {
        assert_eq!(DESTRUCTIVE_TOOLS, ["mesh_task_cancel"]);
        assert!(TOOL_NAMES.contains(&"mesh_task_cancel"));
    }

    #[test]
    fn tool_error_is_the_cli_envelope() {
        let payload = tool_error(&MeshError::NoteNotFound("x".into()));
        assert!(payload.contains("\"kind\":\"not_found\""));
        assert!(payload.contains("\"message\":\"note not found: x\""));
    }
}
