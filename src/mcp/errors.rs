//! Tool-call results and the structured error payload.
//!
//! A domain failure is never a JSON-RPC error: the call succeeds at the protocol level and
//! returns `isError: true` whose text is the same JSON envelope the CLI writes to stderr under
//! `--json` (final.md §9.2). One `MeshError`, two renderers, no drift.

use serde_json::Value as Json;

use crate::cli::out;
use crate::error::MeshError;

/// The error envelope, serialised — what a `ToolError` message carries.
pub fn envelope_text(e: &MeshError) -> String {
    serde_json::to_string(&out::error_envelope(e))
        .unwrap_or_else(|_| "{\"kind\":\"error\"}".to_string())
}

/// A failed `tools/call` result: the envelope as text, `isError: true`, no structured content.
pub fn failure(e: &MeshError) -> Json {
    serde_json::json!({
        "content": [{"type": "text", "text": envelope_text(e)}],
        "isError": true,
    })
}

/// A successful `tools/call` result.
///
/// `content[0].text` is the value serialised; `structuredContent` is the object itself, or the
/// `{"result": [...]}` wrapper a list-returning tool needs to stay a JSON-Schema object.
pub fn success(value: &Json, wrap_list: bool) -> Json {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    let structured = if wrap_list {
        serde_json::json!({ "result": value })
    } else {
        value.clone()
    };
    serde_json::json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": structured,
    })
}

/// A required parameter was absent or null.
pub fn missing(name: &str) -> MeshError {
    MeshError::Validation(format!("missing required parameter: '{name}'"))
}

/// A parameter carried the wrong JSON type.
pub fn wrong_type(name: &str, expected: &str) -> MeshError {
    MeshError::Validation(format!("parameter '{name}' must be {expected}"))
}

/// The tool name is not registered.
pub fn unknown_tool(name: &str) -> MeshError {
    MeshError::Validation(format!("unknown tool: '{name}'"))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_carries_the_envelope_as_text() {
        let value = failure(&MeshError::NoteNotFound("japan".into()));
        assert_eq!(value["isError"], Json::Bool(true));
        assert!(value.get("structuredContent").is_none());
        let text = value["content"][0]["text"].as_str().unwrap();
        let parsed: Json = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["kind"], Json::from("not_found"));
        assert_eq!(parsed["message"], Json::from("note not found: japan"));
        assert_eq!(
            parsed["next_action"],
            Json::from("check the id and retry, or list to find the right one")
        );
        assert_eq!(parsed["id_or_slug"], Json::from("japan"));
    }

    #[test]
    fn a_list_result_is_wrapped_but_its_text_is_the_bare_array() {
        let array = serde_json::json!([{"id": "n-A"}]);
        let value = success(&array, true);
        assert_eq!(value["structuredContent"]["result"], array);
        assert_eq!(
            value["content"][0]["text"],
            Json::from("[{\"id\":\"n-A\"}]")
        );
        assert!(value.get("isError").is_none());
    }

    #[test]
    fn an_object_result_is_not_wrapped() {
        let object = serde_json::json!({"id": "n-A"});
        let value = success(&object, false);
        assert_eq!(value["structuredContent"], object);
    }

    #[test]
    fn the_argument_errors_are_validation_failures() {
        assert_eq!(missing("id").code(), 2);
        assert_eq!(
            missing("id").to_string(),
            "missing required parameter: 'id'"
        );
        assert_eq!(
            wrong_type("tags", "an array of strings").to_string(),
            "parameter 'tags' must be an array of strings"
        );
        assert_eq!(unknown_tool("x").kind(), "validation");
    }
}
