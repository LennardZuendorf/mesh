//! JSON-RPC 2.0 over stdio, newline-delimited: parsing, framing and the four error codes.
//!
//! Hand-rolled on purpose — one response object per request line, nothing for a notification,
//! and no async runtime anywhere near a CLI that has to start in milliseconds.

use serde_json::{Map, Value as Json};

/// The line could not be parsed as JSON.
pub const PARSE_ERROR: i64 = -32700;
/// The line parsed but is not a JSON-RPC request.
pub const INVALID_REQUEST: i64 = -32600;
/// The method is not one this server implements.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// The method is known but its params are not usable.
pub const INVALID_PARAMS: i64 = -32602;

/// The JSON-RPC version every frame carries.
pub const JSONRPC: &str = "2.0";

/// One decoded request. A `None` id makes it a notification, which is never answered.
#[derive(Clone, Debug)]
pub struct Request {
    pub id: Option<Json>,
    pub method: String,
    pub params: Json,
}

impl Request {
    /// Whether this frame expects no response.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    /// The `id` to answer with — `null` for a frame whose id was absent or unusable.
    pub fn reply_id(&self) -> Json {
        self.id.clone().unwrap_or(Json::Null)
    }
}

/// What one input line decoded to.
#[derive(Clone, Debug)]
pub enum Parsed {
    /// A usable request or notification.
    Request(Request),
    /// A complete error response to write back verbatim.
    Error(Json),
}

fn usable_id(value: Option<&Json>) -> Option<Json> {
    match value {
        Some(Json::String(s)) => Some(Json::String(s.clone())),
        Some(Json::Number(n)) => Some(Json::Number(n.clone())),
        _ => None,
    }
}

/// Decode one input line. A blank line is skipped entirely (`None`).
pub fn parse(line: &str) -> Option<Parsed> {
    if line.trim().is_empty() {
        return None;
    }
    let value: Json = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => {
            return Some(Parsed::Error(failure(
                &Json::Null,
                PARSE_ERROR,
                "parse error",
            )))
        }
    };
    let Some(object) = value.as_object() else {
        return Some(Parsed::Error(failure(
            &Json::Null,
            INVALID_REQUEST,
            "invalid request: not a JSON-RPC object",
        )));
    };
    let id = usable_id(object.get("id"));
    let Some(method) = object.get("method").and_then(Json::as_str) else {
        return Some(Parsed::Error(failure(
            &id.clone().unwrap_or(Json::Null),
            INVALID_REQUEST,
            "invalid request: no method",
        )));
    };
    let params = match object.get("params") {
        None | Some(Json::Null) => Json::Object(Map::new()),
        Some(value @ Json::Object(_)) => value.clone(),
        Some(_) => {
            return Some(Parsed::Error(failure(
                &id.clone().unwrap_or(Json::Null),
                INVALID_PARAMS,
                "invalid params: expected an object",
            )))
        }
    };
    Some(Parsed::Request(Request {
        id,
        method: method.to_string(),
        params,
    }))
}

/// A successful response frame.
pub fn success(id: &Json, result: Json) -> Json {
    serde_json::json!({"jsonrpc": JSONRPC, "id": id, "result": result})
}

/// An error response frame.
pub fn failure(id: &Json, code: i64, message: &str) -> Json {
    serde_json::json!({
        "jsonrpc": JSONRPC,
        "id": id,
        "error": {"code": code, "message": message},
    })
}

/// One compact JSON frame plus its newline.
pub fn frame(value: &Json) -> String {
    let mut text = serde_json::to_string(value)
        .unwrap_or_else(|_| "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"internal error\"}}".to_string());
    text.push('\n');
    text
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
    fn a_blank_line_is_skipped() {
        assert!(parse("").is_none());
        assert!(parse("   \t ").is_none());
    }

    #[test]
    fn malformed_json_is_a_parse_error_with_a_null_id() {
        let Some(Parsed::Error(value)) = parse("{not json") else {
            panic!("expected an error frame");
        };
        assert_eq!(value["error"]["code"], Json::from(PARSE_ERROR));
        assert_eq!(value["id"], Json::Null);
        assert_eq!(value["jsonrpc"], Json::from("2.0"));
    }

    #[test]
    fn a_non_object_frame_is_an_invalid_request() {
        let Some(Parsed::Error(value)) = parse("[1,2,3]") else {
            panic!("expected an error frame");
        };
        assert_eq!(value["error"]["code"], Json::from(INVALID_REQUEST));
    }

    #[test]
    fn a_frame_with_no_method_is_an_invalid_request_that_keeps_its_id() {
        let Some(Parsed::Error(value)) = parse("{\"jsonrpc\":\"2.0\",\"id\":7}") else {
            panic!("expected an error frame");
        };
        assert_eq!(value["error"]["code"], Json::from(INVALID_REQUEST));
        assert_eq!(value["id"], Json::from(7));
    }

    #[test]
    fn scalar_params_are_invalid_params() {
        let Some(Parsed::Error(value)) =
            parse("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"params\":4}")
        else {
            panic!("expected an error frame");
        };
        assert_eq!(value["error"]["code"], Json::from(INVALID_PARAMS));
    }

    #[test]
    fn a_missing_id_makes_a_notification() {
        let Some(Parsed::Request(req)) =
            parse("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}")
        else {
            panic!("expected a request");
        };
        assert!(req.is_notification());
        assert_eq!(req.reply_id(), Json::Null);
        assert_eq!(req.params, serde_json::json!({}));
    }

    #[test]
    fn a_string_id_survives_the_round_trip() {
        let Some(Parsed::Request(req)) =
            parse("{\"jsonrpc\":\"2.0\",\"id\":\"abc\",\"method\":\"ping\",\"params\":{\"a\":1}}")
        else {
            panic!("expected a request");
        };
        assert!(!req.is_notification());
        assert_eq!(req.reply_id(), Json::from("abc"));
        assert_eq!(req.params["a"], Json::from(1));
    }

    #[test]
    fn frames_are_one_compact_line() {
        let text = frame(&success(&Json::from(1), serde_json::json!({"ok": true})));
        assert!(text.ends_with('\n'));
        assert_eq!(text.matches('\n').count(), 1);
        assert!(text.contains("\"jsonrpc\":\"2.0\""));
    }
}
