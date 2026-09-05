//! Foundation CLI contract: exit codes, flag placement, help ordering, no panics.
//!
//! These only need the stubbed verbs — every row asserts the shell around a verb, not the
//! verb itself.

mod common;

use common::VaultFixture;
use predicates::prelude::*;

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn version_prints_the_bare_number() {
    let fixture = VaultFixture::new();
    fixture
        .cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout("0.2.0\n")
        .stderr("");
}

#[test]
fn no_args_prints_help_to_stdout_and_exits_two() {
    let fixture = VaultFixture::new();
    let out = fixture.cmd().output().expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("Three verbs, one folder, one mesh"),
        "{stdout}"
    );
    assert!(stdout.contains("Usage: mesh"), "{stdout}");
    assert_eq!(stderr_of(&out), "", "help must not go to stderr");
}

#[test]
fn a_sub_app_with_no_subcommand_prints_its_help_to_stdout_and_exits_two() {
    let fixture = VaultFixture::new();
    for (command, marker) in [
        ("note", "Capture knowledge as Markdown."),
        ("task", "Coordinate work as claimable task files."),
        (
            "memory",
            "Remember what an agent learned about the operator.",
        ),
        ("scratch", "Keep this session's working state, per agent."),
        ("asset", "Store files beside the vault, content-addressed."),
        ("config", "Inspect and edit the mesh config."),
    ] {
        let out = fixture.cmd().arg(command).output().expect("run mesh");
        assert_eq!(out.status.code(), Some(2), "{command}");
        assert!(
            stdout_of(&out).contains(marker),
            "{command}: {}",
            stdout_of(&out)
        );
        assert_eq!(stderr_of(&out), "", "{command} help must not go to stderr");
    }
}

#[test]
fn root_help_lists_the_commands_in_the_fixed_order() {
    let fixture = VaultFixture::new();
    let out = fixture.cmd().arg("--help").output().expect("run mesh");
    assert_eq!(out.status.code(), Some(0));
    let stdout = stdout_of(&out);
    let order = [
        "note",
        "task",
        "search",
        "memory",
        "scratch",
        "asset",
        "init",
        "status",
        "reindex",
        "recent-activity",
        "build-context",
        "graph",
        "project",
        "session-start",
        "watch",
        "config",
        "completions",
        "mcp",
    ];
    let mut cursor = 0usize;
    for name in order {
        let needle = format!("\n  {name}");
        let found = stdout
            .get(cursor..)
            .and_then(|rest| rest.find(&needle))
            .unwrap_or_else(|| panic!("{name} missing or out of order in:\n{stdout}"));
        cursor += found + needle.len();
    }
    assert!(!stdout.contains("daemon"), "the daemon shim stays hidden");
}

#[test]
fn a_missing_config_is_the_three_line_message_at_exit_two() {
    let fixture = VaultFixture::new();
    let missing = fixture.dir.path().join("nope.toml");
    let out = assert_cmd::Command::cargo_bin("mesh")
        .expect("mesh binary")
        .env_remove("MESH_AGENT")
        .env_remove("MESH_VAULT")
        .args(["--config", &missing.to_string_lossy(), "note", "list"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    let stderr = stderr_of(&out);
    let lines: Vec<&str> = stderr.trim_end().split('\n').collect();
    assert_eq!(lines.len(), 3, "{stderr}");
    assert_eq!(
        lines[0],
        format!("mesh: no config found at {}", missing.display())
    );
    assert_eq!(
        lines[1],
        "run `mesh init` to create one (honours $MESH_CONFIG_PATH), or point $MESH_CONFIG_PATH at an existing config."
    );
    assert_eq!(
        lines[2],
        "required: [core].vault_path (path to your Markdown vault folder); [core].agent, [search], and [tasks] are optional and default."
    );
    assert_eq!(stdout_of(&out), "");
}

#[test]
fn the_missing_config_json_envelope_carries_cfg_path() {
    let fixture = VaultFixture::new();
    let out = fixture
        .bare_cmd()
        .args(["--json", "note", "list"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    let payload: serde_json::Value =
        serde_json::from_str(stderr_of(&out).trim()).expect("json envelope on stderr");
    assert_eq!(payload["kind"], "config_missing");
    assert!(payload["cfg_path"].as_str().is_some());
    assert!(payload["next_action"]
        .as_str()
        .unwrap_or("")
        .contains("mesh init"));
}

#[test]
fn flags_are_byte_identical_on_either_side_of_the_command_name() {
    let fixture = VaultFixture::new();
    for (left, right) in [
        (
            vec!["--json", "note", "list"],
            vec!["note", "list", "--json"],
        ),
        (
            vec!["--quiet", "note", "list"],
            vec!["note", "list", "--quiet"],
        ),
        (
            vec!["--json", "task", "list"],
            vec!["task", "list", "--json"],
        ),
        (
            vec!["--json", "memory", "list"],
            vec!["memory", "list", "--json"],
        ),
        (
            vec!["--json", "scratch", "list"],
            vec!["scratch", "list", "--json"],
        ),
        (
            vec!["--json", "asset", "list"],
            vec!["asset", "list", "--json"],
        ),
        (vec!["--json", "search", "q"], vec!["search", "q", "--json"]),
    ] {
        let a = fixture.cmd().args(&left).output().expect("run left");
        let b = fixture.cmd().args(&right).output().expect("run right");
        assert_eq!(a.status.code(), b.status.code(), "{left:?} vs {right:?}");
        assert_eq!(stdout_of(&a), stdout_of(&b), "{left:?} vs {right:?}");
        assert_eq!(stderr_of(&a), stderr_of(&b), "{left:?} vs {right:?}");
    }
}

#[test]
fn admin_commands_take_the_output_flags_global_side_only() {
    let fixture = VaultFixture::new();
    let ok = fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run mesh");
    assert!(
        matches!(ok.status.code(), Some(0) | Some(2)),
        "{}",
        stderr_of(&ok)
    );

    let rejected = fixture
        .cmd()
        .args(["status", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(rejected.status.code(), Some(2));
    assert!(
        stderr_of(&rejected).contains("unexpected argument"),
        "{}",
        stderr_of(&rejected)
    );
}

#[test]
fn clap_parse_failures_exit_two_on_stderr() {
    let fixture = VaultFixture::new();
    for args in [
        vec!["bogus-command"],
        vec!["note", "bogus-sub"],
        vec!["note", "new"],
        vec!["note", "list", "--limit", "not-a-number"],
        vec!["completions", "nonesuch"],
    ] {
        let out = fixture.cmd().args(&args).output().expect("run mesh");
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(!stderr_of(&out).is_empty(), "{args:?} must explain itself");
        assert_eq!(stdout_of(&out), "", "{args:?} must not print to stdout");
    }
}

#[test]
fn no_invocation_ever_prints_a_rust_panic() {
    let fixture = VaultFixture::from_corpus();
    let matrix: Vec<Vec<&str>> = vec![
        vec!["--version"],
        vec!["--help"],
        vec![],
        vec!["note"],
        vec!["note", "list"],
        vec!["note", "get", "n-6YQY"],
        vec!["note", "delete", "n-6YQY"],
        vec!["task", "list"],
        vec!["task", "claim", "t-TCY1"],
        vec!["task", "next"],
        vec!["memory", "recall", "x"],
        vec!["scratch", "get", "x"],
        vec!["asset", "path", "a-1"],
        vec!["search", "zebra"],
        vec!["search", "--health"],
        vec!["recent-activity"],
        vec!["build-context", "n-6YQY"],
        vec!["graph", "n-6YQY", "--direction", "sideways"],
        vec!["project", "n-19EP"],
        vec!["session-start"],
        vec!["status"],
        vec!["reindex"],
        vec!["config", "path"],
        vec!["config", "show"],
        vec!["daemon", "status"],
        vec!["watch", "--once"],
        vec!["completions", "bash"],
        vec!["bogus"],
    ];
    for args in matrix {
        let out = fixture.cmd().args(&args).output().expect("run mesh");
        let stderr = stderr_of(&out);
        assert!(!stderr.contains("panicked"), "{args:?} panicked: {stderr}");
        assert!(
            !stderr.contains("RUST_BACKTRACE"),
            "{args:?} leaked a backtrace: {stderr}"
        );
        let code = out.status.code().unwrap_or(-1);
        assert!((0..=5).contains(&code), "{args:?} exited {code}");
    }
}

#[test]
fn the_hidden_daemon_shim_is_reachable() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["daemon", "status"])
        .output()
        .expect("run mesh");
    assert!(
        matches!(out.status.code(), Some(0) | Some(2)),
        "{}",
        stderr_of(&out)
    );
    assert!(!stderr_of(&out).contains("panicked"));
}

#[test]
fn config_path_never_requires_a_config_file() {
    let fixture = VaultFixture::new();
    let out = fixture
        .bare_cmd()
        .args(["config", "path"])
        .output()
        .expect("run mesh");
    assert!(
        !stderr_of(&out).contains("no config found"),
        "{}",
        stderr_of(&out)
    );
    let out = fixture
        .bare_cmd()
        .args(["init"])
        .output()
        .expect("run mesh");
    assert!(
        !stderr_of(&out).contains("no config found"),
        "{}",
        stderr_of(&out)
    );
}

#[test]
fn help_text_prints_raw_bracketed_strings() {
    let fixture = VaultFixture::new();
    fixture
        .cmd()
        .args(["note", "new", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Owner identity (must be in [tasks].collections).",
        ));
}

#[test]
fn the_tag_spec_sentence_is_the_shared_one() {
    let fixture = VaultFixture::new();
    for args in [["note", "update"], ["task", "update"], ["memory", "update"]] {
        fixture
            .cmd()
            .args(args)
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "Bare 'x,y' adds tags (additive, idempotent)",
            ));
    }
}
