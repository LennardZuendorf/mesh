//! The stdio MCP server, driven as a real process: `mesh mcp` and the `mesh-mcp` shim.
//!
//! Every test scripts newline-delimited JSON-RPC into the binary's stdin and reads the frames
//! back off stdout, so the protocol loop, the tool table and the domain seams are all exercised
//! exactly as an MCP client would exercise them.

mod common;

use common::VaultFixture;
use serde_json::{json, Value as Json};

// ---------------------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------------------

/// Run a scripted session against `mesh mcp` and return one parsed frame per output line.
fn session(f: &VaultFixture, lines: &[String]) -> Vec<Json> {
    frames(f.cmd().arg("mcp").write_stdin(script(lines)).output())
}

/// The same, against the `mesh-mcp` shim, which finds its config through `$MESH_CONFIG_PATH`.
fn shim_session(f: &VaultFixture, lines: &[String]) -> Vec<Json> {
    let mut cmd = assert_cmd::Command::cargo_bin("mesh-mcp").expect("mesh-mcp binary");
    cmd.env_remove("MESH_AGENT")
        .env_remove("MESH_VAULT")
        .env_remove("MESH_INDEXED_BIN")
        .env("MESH_CONFIG_PATH", &f.config);
    frames(cmd.write_stdin(script(lines)).output())
}

/// A session with no config anywhere.
fn bare_session(f: &VaultFixture, lines: &[String]) -> Vec<Json> {
    frames(f.bare_cmd().arg("mcp").write_stdin(script(lines)).output())
}

fn script(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn frames(output: std::io::Result<std::process::Output>) -> Vec<Json> {
    let output = output.expect("run the mcp server");
    assert!(output.status.success(), "the server exited non-zero");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("a JSON-RPC frame per line"))
        .collect()
}

fn request(id: i64, method: &str, params: Json) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn initialize() -> String {
    request(
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}}),
    )
}

fn call(id: i64, name: &str, arguments: Json) -> String {
    request(
        id,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
}

/// Run one tool and return its `tools/call` result object.
fn tool(f: &VaultFixture, name: &str, arguments: Json) -> Json {
    let out = session(f, &[call(1, name, arguments)]);
    assert_eq!(out.len(), 1, "expected one frame, got {out:?}");
    out[0]["result"].clone()
}

/// The structured payload of a successful call.
fn structured(result: &Json) -> Json {
    assert!(
        result.get("isError").is_none(),
        "unexpected error: {}",
        result["content"][0]["text"]
    );
    result["structuredContent"].clone()
}

/// The rows of a list-returning tool.
fn list(result: &Json) -> Vec<Json> {
    structured(result)["result"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The parsed error envelope of a failed call.
fn envelope(result: &Json) -> Json {
    assert_eq!(
        result["isError"],
        json!(true),
        "expected an error: {result}"
    );
    let text = result["content"][0]["text"].as_str().expect("error text");
    serde_json::from_str(text).expect("the envelope is JSON")
}

/// The `tools/list` payload.
fn tools(f: &VaultFixture) -> Vec<Json> {
    let out = session(f, &[request(1, "tools/list", json!({}))]);
    out[0]["result"]["tools"]
        .as_array()
        .cloned()
        .expect("a tool array")
}

fn seed_note(f: &VaultFixture, title: &str, body: &str) -> String {
    let out = f
        .cmd()
        .args(["note", "new", title, "--body", body, "--json"])
        .output()
        .expect("seed a note");
    let value: Json = serde_json::from_slice(&out.stdout).expect("note new --json");
    value["id"].as_str().expect("an id").to_string()
}

fn seed_task(f: &VaultFixture, title: &str) -> String {
    let out = f
        .cmd()
        .args(["task", "new", title, "--json"])
        .output()
        .expect("seed a task");
    let value: Json = serde_json::from_slice(&out.stdout).expect("task new --json");
    value["id"].as_str().expect("an id").to_string()
}

// ---------------------------------------------------------------------------------------
// the protocol
// ---------------------------------------------------------------------------------------

#[test]
fn initialize_answers_with_the_pinned_protocol_shape() {
    let f = VaultFixture::new();
    let out = session(&f, &[initialize()]);
    assert_eq!(out.len(), 1);
    let result = &out[0]["result"];
    assert_eq!(out[0]["jsonrpc"], json!("2.0"));
    assert_eq!(out[0]["id"], json!(1));
    assert_eq!(result["protocolVersion"], json!("2025-06-18"));
    assert_eq!(result["capabilities"]["tools"]["listChanged"], json!(false));
    assert_eq!(result["serverInfo"]["name"], json!("mesh"));
    assert_eq!(result["serverInfo"]["version"], json!("0.2.0"));
}

#[test]
fn initialize_carries_the_instructions_block() {
    let f = VaultFixture::new();
    let out = session(&f, &[initialize()]);
    let text = out[0]["result"]["instructions"]
        .as_str()
        .expect("instructions");
    assert!(!text.is_empty());
    assert!(text.starts_with("# mesh"));
    assert!(text.contains("## Coordination protocol"));
    assert!(text.contains("test-agent"));
}

#[test]
fn the_instructions_block_stays_inside_its_byte_budget() {
    let f = VaultFixture::new();
    let out = session(&f, &[initialize()]);
    let text = out[0]["result"]["instructions"]
        .as_str()
        .unwrap_or_default();
    assert!(text.len() <= 2048, "instructions are {} bytes", text.len());
}

#[test]
fn the_instructions_block_degrades_without_a_config() {
    let f = VaultFixture::new();
    let out = bare_session(&f, &[initialize()]);
    let text = out[0]["result"]["instructions"]
        .as_str()
        .unwrap_or_default();
    assert!(text.contains("mesh init"));
    assert!(text.len() <= 2048);
}

#[test]
fn the_tag_sentence_is_byte_identical_in_the_block_and_the_schemas() {
    let f = VaultFixture::new();
    let out = session(&f, &[initialize(), request(2, "tools/list", json!({}))]);
    let text = out[0]["result"]["instructions"]
        .as_str()
        .unwrap_or_default();
    let sentence = "Bare 'x,y' adds tags (additive, idempotent); '+x,-y' adds/removes; '=x,y' replaces the whole list.";
    assert!(text.contains(sentence), "the block omits the tag sentence");
    let tools = out[1]["result"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for name in ["mesh_note_update", "mesh_task_update", "mesh_memory_update"] {
        let tool = tools.iter().find(|t| t["name"] == json!(name)).expect(name);
        assert_eq!(
            tool["inputSchema"]["properties"]["tags"]["description"],
            json!(sentence),
            "{name}"
        );
    }
}

#[test]
fn a_notification_is_never_answered() {
    let f = VaultFixture::new();
    let line = json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
    let out = session(&f, &[initialize(), line, request(3, "ping", json!({}))]);
    assert_eq!(out.len(), 2, "the notification produced a frame: {out:?}");
    assert_eq!(out[1]["id"], json!(3));
}

#[test]
fn ping_answers_with_an_empty_object() {
    let f = VaultFixture::new();
    let out = session(&f, &[request(9, "ping", json!({}))]);
    assert_eq!(out[0]["result"], json!({}));
    assert_eq!(out[0]["id"], json!(9));
}

#[test]
fn an_unknown_method_is_a_json_rpc_error() {
    let f = VaultFixture::new();
    let out = session(&f, &[request(4, "resources/list", json!({}))]);
    assert_eq!(out[0]["error"]["code"], json!(-32601));
    assert_eq!(out[0]["id"], json!(4));
    assert!(out[0].get("result").is_none());
}

#[test]
fn a_malformed_line_is_a_parse_error_and_the_loop_survives_it() {
    let f = VaultFixture::new();
    let out = session(
        &f,
        &["{ not json".to_string(), request(2, "ping", json!({}))],
    );
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["error"]["code"], json!(-32700));
    assert_eq!(out[0]["id"], Json::Null);
    assert_eq!(out[1]["result"], json!({}));
}

#[test]
fn a_frame_with_no_method_is_an_invalid_request() {
    let f = VaultFixture::new();
    let line = json!({"jsonrpc": "2.0", "id": 5}).to_string();
    let out = session(&f, &[line]);
    assert_eq!(out[0]["error"]["code"], json!(-32600));
    assert_eq!(out[0]["id"], json!(5));
}

#[test]
fn tools_call_without_a_name_is_invalid_params() {
    let f = VaultFixture::new();
    let out = session(&f, &[request(6, "tools/call", json!({"arguments": {}}))]);
    assert_eq!(out[0]["error"]["code"], json!(-32602));
    assert_eq!(out[0]["id"], json!(6));
}

#[test]
fn scalar_params_are_invalid_params() {
    let f = VaultFixture::new();
    let line = json!({"jsonrpc": "2.0", "id": 7, "method": "ping", "params": 3}).to_string();
    let out = session(&f, &[line]);
    assert_eq!(out[0]["error"]["code"], json!(-32602));
}

#[test]
fn blank_lines_are_skipped_and_ids_round_trip() {
    let f = VaultFixture::new();
    let line = json!({"jsonrpc": "2.0", "id": "abc", "method": "ping"}).to_string();
    let out = session(&f, &[String::new(), line, "   ".to_string()]);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["id"], json!("abc"));
}

#[test]
fn several_requests_are_answered_in_order() {
    let f = VaultFixture::new();
    let out = session(
        &f,
        &[
            initialize(),
            request(2, "ping", json!({})),
            request(3, "tools/list", json!({})),
        ],
    );
    assert_eq!(out.len(), 3);
    assert_eq!(out[0]["id"], json!(1));
    assert_eq!(out[1]["id"], json!(2));
    assert_eq!(out[2]["id"], json!(3));
}

#[test]
fn the_shim_binary_speaks_the_same_protocol() {
    let f = VaultFixture::new();
    let out = shim_session(&f, &[initialize(), request(2, "tools/list", json!({}))]);
    assert_eq!(out[0]["result"]["serverInfo"]["name"], json!("mesh"));
    assert_eq!(out[1]["result"]["tools"].as_array().map(Vec::len), Some(37));
}

// ---------------------------------------------------------------------------------------
// tools/list
// ---------------------------------------------------------------------------------------

#[test]
fn the_tool_list_is_thirty_seven_names_in_registration_order() {
    let f = VaultFixture::new();
    let all = tools(&f);
    let names: Vec<&str> = all.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(names.len(), 37);
    assert_eq!(
        names,
        vec![
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
        ]
    );
}

#[test]
fn every_tool_carries_three_explicit_annotation_booleans() {
    let f = VaultFixture::new();
    for tool in tools(&f) {
        let ann = &tool["annotations"];
        for hint in ["readOnlyHint", "idempotentHint", "destructiveHint"] {
            assert!(ann[hint].is_boolean(), "{} has no {hint}", tool["name"]);
        }
    }
}

#[test]
fn the_destructive_set_is_exactly_task_cancel() {
    let f = VaultFixture::new();
    let destructive: Vec<String> = tools(&f)
        .iter()
        .filter(|t| t["annotations"]["destructiveHint"] == json!(true))
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(destructive, vec!["mesh_task_cancel".to_string()]);
}

#[test]
fn the_read_only_tools_are_the_eleven_legacy_reads_plus_the_new_ones() {
    let f = VaultFixture::new();
    let read_only: Vec<String> = tools(&f)
        .iter()
        .filter(|t| t["annotations"]["readOnlyHint"] == json!(true))
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    for name in [
        "mesh_note_get",
        "mesh_task_list",
        "mesh_health",
        "mesh_memory_recall",
        "mesh_scratch_list",
        "mesh_asset_get",
    ] {
        assert!(
            read_only.contains(&name.to_string()),
            "{name} is not read-only"
        );
    }
    for name in ["mesh_note_new", "mesh_task_cancel", "mesh_task_block"] {
        assert!(
            !read_only.contains(&name.to_string()),
            "{name} is read-only"
        );
    }
}

#[test]
fn no_withheld_verb_is_reachable_by_name() {
    let f = VaultFixture::new();
    for tool in tools(&f) {
        let name = tool["name"].as_str().unwrap_or_default().to_string();
        for banned in [
            "delete", "daemon", "reindex", "status", "init", "watch", "config", "forget", "remove",
            "clear", "gc",
        ] {
            assert!(!name.contains(banned), "{name} contains {banned}");
        }
    }
}

#[test]
fn the_serialised_tool_list_fits_thirty_two_kilobytes() {
    let f = VaultFixture::new();
    let out = session(&f, &[request(1, "tools/list", json!({}))]);
    let text = serde_json::to_string(&out[0]).expect("serialise the frame");
    assert!(
        text.len() <= 32 * 1024,
        "tools/list is {} bytes",
        text.len()
    );
}

#[test]
fn every_parameter_of_every_tool_has_a_description() {
    let f = VaultFixture::new();
    for tool in tools(&f) {
        let properties = tool["inputSchema"]["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (name, schema) in properties {
            let description = schema["description"].as_str().unwrap_or_default();
            assert!(
                !description.is_empty(),
                "{}.{name} has no description",
                tool["name"]
            );
            assert!(!name.starts_with('-'), "{name} looks like a flag");
        }
    }
}

#[test]
fn the_enums_are_generated_and_task_list_status_is_free_text() {
    let f = VaultFixture::new();
    let all = tools(&f);
    let find = |name: &str| {
        all.iter()
            .find(|t| t["name"] == json!(name))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(
        find("mesh_note_new")["inputSchema"]["properties"]["note_type"]["enum"],
        json!(["note", "log", "decision", "reference", "project"])
    );
    assert_eq!(
        find("mesh_graph")["inputSchema"]["properties"]["direction"]["enum"],
        json!(["out", "in", "both"])
    );
    assert_eq!(
        find("mesh_memory_new")["inputSchema"]["properties"]["scope"]["enum"],
        json!(["shared", "private"])
    );
    let status = &find("mesh_task_list")["inputSchema"]["properties"]["status"];
    assert!(
        status.get("enum").is_none(),
        "task_list.status was narrowed"
    );
}

#[test]
fn health_takes_nothing_and_release_never_grows_a_force_flag() {
    let f = VaultFixture::new();
    let all = tools(&f);
    let find = |name: &str| {
        all.iter()
            .find(|t| t["name"] == json!(name))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(find("mesh_health")["inputSchema"]["properties"], json!({}));
    let release = find("mesh_task_release")["inputSchema"]["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let names: Vec<&String> = release.keys().collect();
    assert_eq!(names, vec!["task_id", "owner"]);
}

#[test]
fn required_parameters_are_declared() {
    let f = VaultFixture::new();
    let all = tools(&f);
    let find = |name: &str| {
        all.iter()
            .find(|t| t["name"] == json!(name))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(
        find("mesh_note_get")["inputSchema"]["required"],
        json!(["id"])
    );
    assert_eq!(
        find("mesh_task_block")["inputSchema"]["required"],
        json!(["task_id", "on"])
    );
    assert!(find("mesh_note_list")["inputSchema"]
        .get("required")
        .is_none());
}

// ---------------------------------------------------------------------------------------
// notes
// ---------------------------------------------------------------------------------------

#[test]
fn note_new_returns_frontmatter_and_an_empty_warnings_array() {
    let f = VaultFixture::new();
    let result = tool(
        &f,
        "mesh_note_new",
        json!({"title": "Alpha", "body": "hello", "tags": ["x"]}),
    );
    let payload = structured(&result);
    assert_eq!(payload["type"], json!("note"));
    assert_eq!(payload["title"], json!("Alpha"));
    assert_eq!(payload["tags"], json!(["x"]));
    assert_eq!(payload["warnings"], json!([]));
    assert!(payload.get("path").is_none(), "new must not return a path");
    assert!(payload.get("body").is_none(), "new must not return a body");
    let id = payload["id"].as_str().unwrap_or_default().to_string();
    assert!(f.files().iter().any(|p| p.contains(&id)));
}

#[test]
fn note_new_warns_about_a_duplicate_title() {
    let f = VaultFixture::new();
    let first = seed_note(&f, "Shared Title", "one");
    let result = tool(&f, "mesh_note_new", json!({"title": "shared  title"}));
    assert_eq!(
        structured(&result)["warnings"],
        json!([format!("duplicate title, also used by {first}")])
    );
}

#[test]
fn note_get_returns_frontmatter_body_and_path() {
    let f = VaultFixture::new();
    let id = seed_note(&f, "Alpha", "the body");
    let payload = structured(&tool(&f, "mesh_note_get", json!({"id": &id})));
    assert_eq!(payload["id"], json!(id));
    assert_eq!(payload["body"], json!("the body"));
    assert!(payload["path"]
        .as_str()
        .unwrap_or_default()
        .ends_with(".md"));
    let keys: Vec<&String> = payload
        .as_object()
        .map(|m| m.keys().collect())
        .unwrap_or_default();
    assert_eq!(keys.last().map(|k| k.as_str()), Some("path"));
}

#[test]
fn note_list_wraps_its_rows_and_carries_a_path_but_no_body() {
    let f = VaultFixture::new();
    seed_note(&f, "Alpha", "a");
    seed_note(&f, "Beta", "b");
    let result = tool(&f, "mesh_note_list", json!({"limit": 10}));
    let rows = list(&result);
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert!(row["path"].is_string());
        assert!(row.get("body").is_none());
    }
    // The text content is the bare array; the structured content is the wrapper.
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    let parsed: Json = serde_json::from_str(text).expect("array text");
    assert!(parsed.is_array());
}

#[test]
fn note_list_filters_by_type_and_tags() {
    let f = VaultFixture::new();
    seed_note(&f, "Alpha", "a");
    f.cmd()
        .args([
            "note", "new", "Log one", "--type", "log", "--tags", "ops", "--body", "x",
        ])
        .assert()
        .success();
    let rows = list(&tool(&f, "mesh_note_list", json!({"note_type": "log"})));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["type"], json!("log"));
    let rows = list(&tool(&f, "mesh_note_list", json!({"tags": ["ops"]})));
    assert_eq!(rows.len(), 1);
    let rows = list(&tool(&f, "mesh_note_list", json!({"tags": ["nope"]})));
    assert!(rows.is_empty());
}

#[test]
fn note_append_writes_the_body_and_returns_bare_frontmatter() {
    let f = VaultFixture::new();
    let id = seed_note(&f, "Alpha", "first");
    let payload = structured(&tool(
        &f,
        "mesh_note_append",
        json!({"target": &id, "text": "second", "section": "Notes"}),
    ));
    assert_eq!(payload["id"], json!(id));
    assert!(payload.get("path").is_none());
    assert!(payload.get("body").is_none());
    let text = f.read(&format!("notes/{id}.md"));
    assert!(text.contains("## Notes"));
    assert!(text.contains("second"));
}

#[test]
fn note_update_runs_the_shared_tag_grammar() {
    let f = VaultFixture::new();
    let id = seed_note(&f, "Alpha", "a");
    tool(
        &f,
        "mesh_note_update",
        json!({"target": &id, "tags": "x,y"}),
    );
    let payload = structured(&tool(
        &f,
        "mesh_note_update",
        json!({"target": &id, "tags": "-x,+z"}),
    ));
    assert_eq!(payload["tags"], json!(["y", "z"]));
}

#[test]
fn a_mixed_tag_spec_is_refused_before_any_write() {
    let f = VaultFixture::new();
    let id = seed_note(&f, "Alpha", "a");
    let before = f.read(&format!("notes/{id}.md"));
    let env = envelope(&tool(
        &f,
        "mesh_note_update",
        json!({"target": &id, "tags": "+x,y"}),
    ));
    assert_eq!(env["kind"], json!("validation"));
    assert!(env["message"]
        .as_str()
        .unwrap_or_default()
        .contains("ambiguous tag spec"));
    assert_eq!(f.read(&format!("notes/{id}.md")), before);
}

#[test]
fn note_update_moves_the_file_when_the_type_changes() {
    let f = VaultFixture::new();
    let id = seed_note(&f, "Alpha", "a");
    let payload = structured(&tool(
        &f,
        "mesh_note_update",
        json!({"target": &id, "new_type": "decision"}),
    ));
    assert_eq!(payload["type"], json!("decision"));
    assert!(f.files().contains(&format!("notes/decisions/{id}.md")));
    assert!(!f.files().contains(&format!("notes/{id}.md")));
}

// ---------------------------------------------------------------------------------------
// tasks
// ---------------------------------------------------------------------------------------

#[test]
fn task_new_then_get_reports_derived_readiness() {
    let f = VaultFixture::new();
    let id = seed_task(&f, "Do a thing");
    let payload = structured(&tool(&f, "mesh_task_get", json!({"id": &id})));
    assert_eq!(payload["status"], json!("open"));
    assert_eq!(payload["ready"], json!(true));
    assert!(payload["path"].is_string());
    assert!(payload["body"].is_string());
}

#[test]
fn task_new_returns_warnings_and_writes_the_file() {
    let f = VaultFixture::new();
    let payload = structured(&tool(
        &f,
        "mesh_task_new",
        json!({"title": "Ship it", "priority": "high", "tags": ["release"]}),
    ));
    assert_eq!(payload["priority"], json!("high"));
    assert_eq!(payload["warnings"], json!([]));
    let id = payload["id"].as_str().unwrap_or_default().to_string();
    assert!(f.files().contains(&format!("tasks/open/{id}.md")));
}

#[test]
fn task_claim_is_a_test_and_set_and_a_conflict_is_structured() {
    let f = VaultFixture::new();
    let id = seed_task(&f, "Do a thing");
    let payload = structured(&tool(&f, "mesh_task_claim", json!({"task_id": &id})));
    assert_eq!(payload["claimed_by"], json!("test-agent"));
    assert_eq!(payload["status"], json!("claimed"));

    let env = envelope(&tool(
        &f,
        "mesh_task_claim",
        json!({"task_id": &id, "claimer": "other-agent"}),
    ));
    assert_eq!(env["kind"], json!("claim_conflict"));
    assert_eq!(env["task_id"], json!(id));
    assert_eq!(env["existing_owner"], json!("test-agent"));
    assert_eq!(
        env["next_action"],
        json!("pick a different task, wait, or ask the named agent to release it")
    );
}

#[test]
fn task_release_returns_the_task_to_open() {
    let f = VaultFixture::new();
    let id = seed_task(&f, "Do a thing");
    tool(&f, "mesh_task_claim", json!({"task_id": &id}));
    let payload = structured(&tool(&f, "mesh_task_release", json!({"task_id": &id})));
    assert_eq!(payload["status"], json!("open"));
    assert_eq!(payload["claimed_by"], Json::Null);
}

#[test]
fn task_finish_moves_the_file_and_records_the_outcome() {
    let f = VaultFixture::new();
    let id = seed_task(&f, "Do a thing");
    let payload = structured(&tool(
        &f,
        "mesh_task_finish",
        json!({"task_id": &id, "outcome": "shipped"}),
    ));
    assert_eq!(payload["status"], json!("done"));
    assert!(f.files().contains(&format!("tasks/done/{id}.md")));
    assert!(f.read(&format!("tasks/done/{id}.md")).contains("shipped"));
}

#[test]
fn task_cancel_moves_the_file_and_records_the_reason() {
    let f = VaultFixture::new();
    let id = seed_task(&f, "Do a thing");
    let payload = structured(&tool(
        &f,
        "mesh_task_cancel",
        json!({"task_id": &id, "reason": "overtaken"}),
    ));
    assert_eq!(payload["status"], json!("cancelled"));
    assert!(f.read(&format!("tasks/done/{id}.md")).contains("overtaken"));
}

#[test]
fn task_update_reassigns_without_touching_the_claim() {
    let f = VaultFixture::new();
    let id = seed_task(&f, "Do a thing");
    tool(&f, "mesh_task_claim", json!({"task_id": &id}));
    let payload = structured(&tool(
        &f,
        "mesh_task_update",
        json!({"task_id": &id, "owner": "other-agent", "priority": "low"}),
    ));
    assert_eq!(payload["owner"], json!("other-agent"));
    assert_eq!(payload["claimed_by"], json!("test-agent"));
    assert_eq!(payload["priority"], json!("low"));
}

#[test]
fn task_list_honours_a_status_union_and_the_computed_sort() {
    let f = VaultFixture::new();
    let open = seed_task(&f, "Still open");
    let done = seed_task(&f, "Already done");
    tool(&f, "mesh_task_finish", json!({"task_id": &done}));
    let rows = list(&tool(
        &f,
        "mesh_task_list",
        json!({"status": "open,claimed"}),
    ));
    let ids: Vec<&str> = rows.iter().filter_map(|r| r["id"].as_str()).collect();
    assert_eq!(ids, vec![open.as_str()]);
    let rows = list(&tool(&f, "mesh_task_list", json!({"available": true})));
    assert_eq!(rows.len(), 1);
    for row in &rows {
        assert!(row["path"].is_string());
    }
}

#[test]
fn task_list_rejects_an_unknown_status_token() {
    let f = VaultFixture::new();
    let env = envelope(&tool(&f, "mesh_task_list", json!({"status": "nope"})));
    assert_eq!(env["kind"], json!("validation"));
    assert!(env["message"]
        .as_str()
        .unwrap_or_default()
        .contains("unknown status"));
}

#[test]
fn task_block_unblock_and_next_walk_the_dependency_graph() {
    let f = VaultFixture::new();
    let blocker = seed_task(&f, "First");
    let blocked = seed_task(&f, "Second");
    let payload = structured(&tool(
        &f,
        "mesh_task_block",
        json!({"task_id": &blocked, "on": [&blocker]}),
    ));
    assert_eq!(payload["blocked_by"], json!([blocker]));

    // `next` picks the ready one, never the blocked one.
    let picked = structured(&tool(&f, "mesh_task_next", json!({})));
    assert_eq!(picked["id"], json!(blocker));
    assert!(picked["path"].is_string());

    let payload = structured(&tool(
        &f,
        "mesh_task_unblock",
        json!({"task_id": &blocked, "all": true}),
    ));
    assert_eq!(payload["blocked_by"], json!([]));
}

#[test]
fn a_strict_claim_on_a_blocked_task_is_a_blocked_envelope() {
    let f = VaultFixture::new();
    let blocker = seed_task(&f, "First");
    let blocked = seed_task(&f, "Second");
    tool(
        &f,
        "mesh_task_block",
        json!({"task_id": &blocked, "on": [&blocker]}),
    );
    let env = envelope(&tool(
        &f,
        "mesh_task_claim",
        json!({"task_id": &blocked, "strict": true}),
    ));
    assert_eq!(env["kind"], json!("blocked"));
    assert_eq!(env["task_id"], json!(blocked));
    assert_eq!(
        env["next_action"],
        json!("finish or cancel the blocking tasks, then retry")
    );
}

#[test]
fn a_non_strict_claim_reports_its_unsatisfied_blockers_in_the_payload() {
    let f = VaultFixture::new();
    let blocker = seed_task(&f, "First");
    let blocked = seed_task(&f, "Second");
    tool(
        &f,
        "mesh_task_block",
        json!({"task_id": &blocked, "on": [&blocker]}),
    );
    let payload = structured(&tool(&f, "mesh_task_claim", json!({"task_id": &blocked})));
    assert_eq!(payload["status"], json!("claimed"));
    assert_eq!(payload["blocked_by_unsatisfied"], json!([blocker]));
}

#[test]
fn task_next_on_an_empty_queue_is_a_not_found_envelope() {
    let f = VaultFixture::new();
    let env = envelope(&tool(&f, "mesh_task_next", json!({})));
    assert_eq!(env["kind"], json!("not_found"));
    assert_eq!(env["message"], json!("no ready task"));
}

#[test]
fn task_next_can_claim_what_it_picks() {
    let f = VaultFixture::new();
    let id = seed_task(&f, "Only one");
    let payload = structured(&tool(&f, "mesh_task_next", json!({"claim": true})));
    assert_eq!(payload["id"], json!(id));
    assert_eq!(payload["claimed_by"], json!("test-agent"));
}

// ---------------------------------------------------------------------------------------
// memories, scratch, search, health
// ---------------------------------------------------------------------------------------

#[test]
fn memory_new_get_and_list_round_trip() {
    let f = VaultFixture::new();
    let payload = structured(&tool(
        &f,
        "mesh_memory_new",
        json!({"title": "Operator prefers tabs", "kind": "preference", "importance": 5,
               "body": "they said so", "tags": ["style"]}),
    ));
    assert_eq!(payload["kind"], json!("preference"));
    assert_eq!(payload["importance"], json!(5));
    assert_eq!(payload["warnings"], json!([]));
    let id = payload["id"].as_str().unwrap_or_default().to_string();

    let got = structured(&tool(&f, "mesh_memory_get", json!({"target": &id})));
    assert_eq!(got["body"], json!("they said so"));
    assert!(got["path"].is_string());

    let rows = list(&tool(&f, "mesh_memory_list", json!({"kind": "preference"})));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], json!(id));
    let rows = list(&tool(&f, "mesh_memory_list", json!({"min_importance": 5})));
    assert_eq!(rows.len(), 1);
}

#[test]
fn memory_recall_returns_the_standard_hit_array() {
    let f = VaultFixture::new();
    tool(
        &f,
        "mesh_memory_new",
        json!({"title": "Deployment window", "body": "deploys happen on tuesday"}),
    );
    let hits = list(&tool(&f, "mesh_memory_recall", json!({"query": "tuesday"})));
    assert_eq!(hits.len(), 1);
    assert!(hits[0]["score"].is_number());
    assert!(hits[0]["path"].is_string());
    assert!(hits[0].get("mode").is_none(), "recall carries no mode key");
}

#[test]
fn memory_update_clears_an_expiry_with_the_literal_none() {
    let f = VaultFixture::new();
    let payload = structured(&tool(
        &f,
        "mesh_memory_new",
        json!({"title": "Temporary", "expires": "7d"}),
    ));
    assert!(payload["expires"].is_string());
    let id = payload["id"].as_str().unwrap_or_default().to_string();
    let payload = structured(&tool(
        &f,
        "mesh_memory_update",
        json!({"target": &id, "expires": "none"}),
    ));
    assert_eq!(payload["expires"], Json::Null);
}

#[test]
fn memory_append_extends_the_body() {
    let f = VaultFixture::new();
    let payload = structured(&tool(
        &f,
        "mesh_memory_new",
        json!({"title": "Belief", "body": "one"}),
    ));
    let id = payload["id"].as_str().unwrap_or_default().to_string();
    tool(
        &f,
        "mesh_memory_append",
        json!({"target": &id, "text": "two"}),
    );
    let got = structured(&tool(&f, "mesh_memory_get", json!({"target": &id})));
    let body = got["body"].as_str().unwrap_or_default();
    assert!(body.contains("one") && body.contains("two"));
}

#[test]
fn scratch_set_get_and_list_round_trip() {
    let f = VaultFixture::new();
    let payload = structured(&tool(
        &f,
        "mesh_scratch_set",
        json!({"name": "plan", "body": "step one"}),
    ));
    assert_eq!(payload["name"], json!("plan"));
    assert_eq!(payload["agent"], json!("test-agent"));

    let got = structured(&tool(&f, "mesh_scratch_get", json!({"name": "plan"})));
    assert_eq!(got["body"], json!("step one"));
    assert!(got["path"].is_string());

    tool(
        &f,
        "mesh_scratch_append",
        json!({"name": "plan", "text": "step two"}),
    );
    let got = structured(&tool(&f, "mesh_scratch_get", json!({"name": "plan"})));
    assert!(got["body"]
        .as_str()
        .unwrap_or_default()
        .contains("step two"));

    let rows = list(&tool(&f, "mesh_scratch_list", json!({})));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], json!("plan"));
    let rows = list(&tool(&f, "mesh_scratch_list", json!({"all_agents": true})));
    assert_eq!(rows.len(), 1);
}

#[test]
fn scratch_addresses_a_peers_namespace() {
    let f = VaultFixture::new();
    tool(
        &f,
        "mesh_scratch_set",
        json!({"name": "plan", "body": "mine", "agent": "other-agent"}),
    );
    let env = envelope(&tool(&f, "mesh_scratch_get", json!({"name": "plan"})));
    assert_eq!(env["kind"], json!("not_found"));
    let got = structured(&tool(
        &f,
        "mesh_scratch_get",
        json!({"name": "plan", "agent": "other-agent"}),
    ));
    assert_eq!(got["body"], json!("mine"));
}

#[test]
fn search_reports_the_mode_it_observed_and_a_tag_pull_does_not() {
    let f = VaultFixture::new();
    f.cmd()
        .args([
            "note",
            "new",
            "Alpha",
            "--body",
            "quicksilver",
            "--tags",
            "ops",
        ])
        .assert()
        .success();
    let hits = list(&tool(&f, "mesh_search", json!({"query": "quicksilver"})));
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["mode"], json!("fallback"));
    let keys: Vec<&String> = hits[0]
        .as_object()
        .map(|m| m.keys().collect())
        .unwrap_or_default();
    assert_eq!(keys.last().map(|k| k.as_str()), Some("mode"));

    let hits = list(&tool(&f, "mesh_search", json!({"tags": ["ops"]})));
    assert_eq!(hits.len(), 1);
    assert!(hits[0].get("mode").is_none(), "a tag pull carries no mode");
}

#[test]
fn search_takes_the_new_space_and_engine_parameters() {
    let f = VaultFixture::new();
    tool(
        &f,
        "mesh_memory_new",
        json!({"title": "Quicksilver", "body": "a memory about quicksilver"}),
    );
    let hits = list(&tool(
        &f,
        "mesh_search",
        json!({"query": "quicksilver", "spaces": ["memories"], "engine": "substring"}),
    ));
    assert_eq!(hits.len(), 1);
    let env = envelope(&tool(
        &f,
        "mesh_search",
        json!({"query": "x", "engine": "nope"}),
    ));
    assert_eq!(env["kind"], json!("validation"));
    assert!(env["message"]
        .as_str()
        .unwrap_or_default()
        .contains("invalid engine"));
}

#[test]
fn health_reports_the_gates_without_shelling_indexed() {
    let f = VaultFixture::new();
    let payload = structured(&tool(&f, "mesh_health", json!({})));
    for key in [
        "mode",
        "hybrid_configured",
        "collection",
        "daemon_up",
        "indexed_binary_available",
    ] {
        assert!(payload.get(key).is_some(), "health has no {key}");
    }
    assert_eq!(payload["mode"], json!("fallback"));
}

#[test]
fn session_start_answers_with_an_array_or_a_structured_error() {
    let f = VaultFixture::new();
    seed_task(&f, "Mine");
    let result = tool(&f, "mesh_session_start", json!({"meta_only": true}));
    if result.get("isError").is_some() {
        // The lens lane has not landed yet; the failure is still a structured envelope.
        let env = envelope(&result);
        assert!(env["kind"].is_string());
        assert!(env["message"].is_string());
    } else {
        assert!(structured(&result)["result"].is_array());
    }
}

// ---------------------------------------------------------------------------------------
// error surfaces
// ---------------------------------------------------------------------------------------

#[test]
fn a_not_found_envelope_carries_its_candidates() {
    let f = VaultFixture::new();
    let id = seed_note(&f, "Alpha", "a");
    let near = format!("{id}x");
    let env = envelope(&tool(&f, "mesh_note_get", json!({"id": near})));
    assert_eq!(env["kind"], json!("not_found"));
    assert_eq!(env["id_or_slug"], json!(format!("{id}x")));
    assert_eq!(env["candidates"], json!([id]));
}

#[test]
fn an_unknown_tool_is_a_validation_error_not_a_protocol_error() {
    let f = VaultFixture::new();
    let out = session(&f, &[call(1, "mesh_note_delete", json!({}))]);
    assert!(out[0].get("error").is_none(), "it became a JSON-RPC error");
    let env = envelope(&out[0]["result"]);
    assert_eq!(env["kind"], json!("validation"));
    assert_eq!(env["message"], json!("unknown tool: 'mesh_note_delete'"));
}

#[test]
fn a_missing_required_argument_is_a_validation_envelope() {
    let f = VaultFixture::new();
    let env = envelope(&tool(&f, "mesh_note_get", json!({})));
    assert_eq!(env["kind"], json!("validation"));
    assert_eq!(env["message"], json!("missing required parameter: 'id'"));
    assert_eq!(env["next_action"], json!("fix the input and retry"));
}

#[test]
fn a_wrongly_typed_argument_names_the_parameter() {
    let f = VaultFixture::new();
    let env = envelope(&tool(&f, "mesh_note_list", json!({"tags": "a,b"})));
    assert_eq!(env["kind"], json!("validation"));
    assert_eq!(
        env["message"],
        json!("parameter 'tags' must be an array of strings")
    );
}

#[test]
fn a_bad_sort_field_is_rejected_with_the_pinned_wording() {
    let f = VaultFixture::new();
    let env = envelope(&tool(&f, "mesh_note_list", json!({"sort": "bogus"})));
    assert_eq!(
        env["message"],
        json!("invalid sort field: 'bogus' (use updated, created, title)")
    );
}

#[test]
fn every_tool_reports_config_missing_instead_of_exiting() {
    let f = VaultFixture::new();
    let names = [
        "mesh_note_get",
        "mesh_task_list",
        "mesh_health",
        "mesh_scratch_set",
    ];
    let lines: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            call(
                i as i64 + 1,
                name,
                json!({"id": "n-A", "target": "n-A", "name": "x", "body": "y"}),
            )
        })
        .collect();
    let out = bare_session(&f, &lines);
    assert_eq!(out.len(), names.len());
    for frame in out {
        let env = envelope(&frame["result"]);
        assert_eq!(env["kind"], json!("config_missing"));
        assert!(env["message"]
            .as_str()
            .unwrap_or_default()
            .contains("mesh init"));
        assert!(env["cfg_path"].is_string());
        assert!(env["next_action"]
            .as_str()
            .unwrap_or_default()
            .contains("mesh init"));
    }
}

#[test]
fn an_ambiguous_slug_names_its_candidates() {
    let f = VaultFixture::new();
    seed_note(&f, "Same Name", "one");
    seed_note(&f, "Same Name", "two");
    let env = envelope(&tool(&f, "mesh_note_get", json!({"id": "same-name"})));
    assert_eq!(env["kind"], json!("ambiguous_slug"));
    assert_eq!(env["slug"], json!("same-name"));
    assert_eq!(env["ids"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        env["next_action"],
        json!("retry using one of the listed ids instead of the slug")
    );
}

#[test]
fn no_next_action_reads_as_an_authorization_decision() {
    let f = VaultFixture::new();
    let cases = [
        ("mesh_note_get", json!({"id": "n-NOPE"})),
        ("mesh_task_get", json!({"id": "t-NOPE"})),
        ("mesh_note_list", json!({"sort": "x"})),
        ("mesh_task_next", json!({})),
    ];
    for (name, args) in cases {
        let env = envelope(&tool(&f, name, args));
        let action = env["next_action"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        for banned in ["not authorized", "denied", "permission", "forbidden"] {
            assert!(!action.contains(banned), "{name}: {action}");
        }
    }
}

// ---------------------------------------------------------------------------------------
// pending lanes
// ---------------------------------------------------------------------------------------

#[test]

fn asset_get_reads_a_sidecar() {
    let f = VaultFixture::new();
    let src = f.dir.path().join("pixel.bin");
    std::fs::write(&src, b"mcp asset bytes").expect("write fixture");
    let out = f
        .cmd()
        .arg("--quiet")
        .arg("asset")
        .arg("add")
        .arg(&src)
        .output()
        .expect("run mesh");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let payload = structured(&tool(&f, "mesh_asset_get", json!({"asset_id": id})));
    assert!(payload["path"].is_string());
    assert_eq!(payload["id"], json!(id));
}

#[test]

fn asset_list_returns_sidecar_rows() {
    let f = VaultFixture::new();
    let rows = list(&tool(&f, "mesh_asset_list", json!({})));
    assert!(rows.iter().all(|r| r["path"].is_string()));
}

#[test]

fn recent_activity_returns_the_seven_key_rows() {
    let f = VaultFixture::new();
    seed_note(&f, "Alpha", "a");
    let rows = list(&tool(&f, "mesh_recent_activity", json!({"limit": 5})));
    assert!(!rows.is_empty());
    for row in rows {
        for key in [
            "id",
            "type",
            "title",
            "path",
            "mtime",
            "owner",
            "claimed_by",
        ] {
            assert!(row.get(key).is_some(), "activity row has no {key}");
        }
    }
}

#[test]

fn graph_returns_seed_nodes_and_edges() {
    let f = VaultFixture::new();
    let id = seed_note(&f, "Alpha", "a");
    let payload = structured(&tool(&f, "mesh_graph", json!({"seed_id": &id})));
    assert_eq!(payload["seed"], json!(id));
    assert!(payload["nodes"].is_array());
    assert!(payload["edges"].is_array());
}

#[test]
fn graph_validates_its_direction_before_the_seed() {
    let f = VaultFixture::new();
    let env = envelope(&tool(
        &f,
        "mesh_graph",
        json!({"seed_id": "n-NOPE", "direction": "sideways"}),
    ));
    assert_eq!(env["kind"], json!("validation"));
    assert_eq!(
        env["message"],
        json!("invalid direction: 'sideways' (use out, in, both)")
    );
}

// ---------------------------------------------------------------------------------------
// result framing
// ---------------------------------------------------------------------------------------

#[test]
fn an_object_tool_puts_the_same_json_in_both_channels() {
    let f = VaultFixture::new();
    let id = seed_note(&f, "Alpha", "a");
    let result = tool(&f, "mesh_note_get", json!({"id": &id}));
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    let parsed: Json = serde_json::from_str(text).expect("object text");
    assert_eq!(parsed, result["structuredContent"]);
    assert_eq!(result["content"][0]["type"], json!("text"));
}

#[test]
fn task_append_extends_the_body_without_moving_the_file() {
    let f = VaultFixture::new();
    let id = seed_task(&f, "Do a thing");
    let payload = structured(&tool(
        &f,
        "mesh_task_append",
        json!({"task_id": &id, "text": "progress", "timestamp": true}),
    ));
    assert_eq!(payload["status"], json!("open"));
    let text = f.read(&format!("tasks/open/{id}.md"));
    assert!(text.contains("progress"));
    assert!(text.contains("test-agent"), "the stamp names the caller");
}

#[test]
fn memory_update_rewrites_the_typed_fields() {
    let f = VaultFixture::new();
    let payload = structured(&tool(
        &f,
        "mesh_memory_new",
        json!({"title": "Belief", "body": "b"}),
    ));
    let id = payload["id"].as_str().unwrap_or_default().to_string();
    let payload = structured(&tool(
        &f,
        "mesh_memory_update",
        json!({"target": &id, "kind": "insight", "scope": "private", "importance": 2,
               "source": "a chat", "title": "Sharper belief"}),
    ));
    assert_eq!(payload["kind"], json!("insight"));
    assert_eq!(payload["scope"], json!("private"));
    assert_eq!(payload["importance"], json!(2));
    assert_eq!(payload["source"], json!("a chat"));
    assert_eq!(payload["title"], json!("Sharper belief"));
}

#[test]
fn a_zero_limit_yields_an_empty_list_not_an_error() {
    let f = VaultFixture::new();
    seed_note(&f, "Alpha", "a");
    let rows = list(&tool(&f, "mesh_note_list", json!({"limit": 0})));
    assert!(rows.is_empty());
}

#[test]
fn an_invalid_enum_value_is_rejected_by_the_domain_not_by_a_crash() {
    let f = VaultFixture::new();
    let env = envelope(&tool(
        &f,
        "mesh_note_new",
        json!({"title": "Alpha", "note_type": "memo"}),
    ));
    assert_eq!(env["kind"], json!("validation"));
    assert!(env["message"]
        .as_str()
        .unwrap_or_default()
        .contains("invalid note type"));
    assert!(!f.files().iter().any(|p| p.starts_with("notes/n-")));
}
