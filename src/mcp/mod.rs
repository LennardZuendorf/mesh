//! The stdio MCP server: JSON-RPC 2.0 over newline-delimited stdin/stdout.
//!
//! Thirty-seven tools over the same domain seams the CLI calls, an instructions block built
//! once from the config, and a hand-rolled protocol loop — no MCP crate, no async runtime, and
//! nothing that can make a tool failure kill the process.

pub mod errors;
pub mod instructions;
pub mod proto;
pub mod schema;
pub mod tools;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::config::Config;
use crate::error::MeshError;

/// The server name every client sees.
pub const SERVER_NAME: &str = "mesh";

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
///
/// `mesh mcp` is dispatched without its `Ctx` (`cli/mod.rs` is frozen), so the `--config` /
/// `--vault` this process was started with are recovered from its own argv — the same values
/// clap already accepted. `mesh-mcp` passes neither and falls back to `$MESH_CONFIG_PATH`.
pub fn serve_stdio_code() -> i32 {
    let argv: Vec<String> = std::env::args().collect();
    let config = flag_value(&argv, "--config").map(PathBuf::from);
    let vault = flag_value(&argv, "--vault").map(PathBuf::from);
    serve(
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
        config,
        vault,
    )
}

/// `--flag value` or `--flag=value`, whichever form the caller used.
fn flag_value(argv: &[String], flag: &str) -> Option<String> {
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        if arg == flag {
            return it.next().cloned();
        }
        if let Some(rest) = arg.strip_prefix(flag).and_then(|r| r.strip_prefix('=')) {
            return Some(rest.to_string());
        }
    }
    None
}

/// The loop itself: one request line in, at most one response line out.
///
/// Nothing here can fail the process. A malformed line is a JSON-RPC error frame, an unknown
/// method is `-32601`, and a domain failure is a successful call carrying `isError`.
pub fn serve(
    input: &mut impl BufRead,
    output: &mut impl Write,
    config: Option<PathBuf>,
    vault: Option<PathBuf>,
) -> i32 {
    let server = tools::Server::new(config, vault);
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => return 0,
            Ok(_) => {}
        }
        let Some(parsed) = proto::parse(&line) else {
            continue;
        };
        let response = match parsed {
            proto::Parsed::Error(frame) => Some(frame),
            proto::Parsed::Request(request) => handle(&server, &request),
        };
        if let Some(frame) = response {
            if output.write_all(proto::frame(&frame).as_bytes()).is_err() {
                return 0;
            }
            let _ = output.flush();
        }
    }
}

/// Answer one request, or `None` when the frame is a notification.
fn handle(server: &tools::Server, request: &proto::Request) -> Option<serde_json::Value> {
    let id = request.reply_id();
    let result = match request.method.as_str() {
        "initialize" => Ok(initialize_result(server)),
        "notifications/initialized" => return None,
        "ping" => Ok(serde_json::json!({})),
        "tools/list" => Ok(serde_json::json!({"tools": schema::tools_list()})),
        "tools/call" => call_result(server, &request.params),
        _ if request.method.starts_with("notifications/") => return None,
        other => Err(proto::failure(
            &id,
            proto::METHOD_NOT_FOUND,
            &format!("method not found: {other}"),
        )),
    };
    if request.is_notification() {
        return None;
    }
    Some(match result {
        Ok(value) => proto::success(&id, value),
        Err(frame) => reframe(frame, &id),
    })
}

/// A protocol-level failure built against a placeholder id, re-stamped with the real one.
fn reframe(mut frame: serde_json::Value, id: &serde_json::Value) -> serde_json::Value {
    if let Some(map) = frame.as_object_mut() {
        map.insert("id".to_string(), id.clone());
    }
    frame
}

/// The `initialize` result: protocol version, capabilities, identity, instructions.
fn initialize_result(server: &tools::Server) -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": SERVER_NAME, "version": crate::VERSION},
        "instructions": server.instructions(),
    })
}

/// `tools/call`: dispatch by name, then render success or the error envelope.
fn call_result(
    server: &tools::Server,
    params: &serde_json::Value,
) -> std::result::Result<serde_json::Value, serde_json::Value> {
    let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
        return Err(proto::failure(
            &serde_json::Value::Null,
            proto::INVALID_PARAMS,
            "invalid params: tools/call needs a tool name",
        ));
    };
    let empty = serde_json::json!({});
    let arguments = match params.get("arguments") {
        None | Some(serde_json::Value::Null) => empty,
        Some(value @ serde_json::Value::Object(_)) => value.clone(),
        Some(_) => {
            return Err(proto::failure(
                &serde_json::Value::Null,
                proto::INVALID_PARAMS,
                "invalid params: arguments must be an object",
            ))
        }
    };
    Ok(match server.call(name, &arguments) {
        Ok(outcome) => errors::success(&outcome.value, outcome.list),
        Err(e) => errors::failure(&e),
    })
}

/// The instructions block, built once at startup. Pure, and at most 2048 UTF-8 bytes.
pub fn build_instructions(cfg: Option<&Config>) -> String {
    instructions::build(cfg)
}

/// The JSON error payload an MCP `ToolError` carries as its message.
pub fn tool_error(e: &MeshError) -> String {
    errors::envelope_text(e)
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
