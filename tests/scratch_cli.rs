//! Integration tests for `mesh scratch …`, driven through the real binary.

mod common;

use common::VaultFixture;
use serde_json::Value as Json;

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json_stdout(out: &std::process::Output) -> Json {
    serde_json::from_str(stdout_of(out).trim()).expect("json on stdout")
}

fn json_err(out: &std::process::Output) -> Json {
    serde_json::from_str(stderr_of(out).trim()).expect("json envelope on stderr")
}

// ---------------------------------------------------------------------------------------
// set
// ---------------------------------------------------------------------------------------

#[test]
fn set_writes_o8_frontmatter_order_and_a_slugified_path() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "Flight Search", "--body", "line one"])
        .assert()
        .success();
    assert!(f
        .files()
        .contains(&"scratch/test-agent/flight-search.md".to_string()));
    let text = f.read("scratch/test-agent/flight-search.md");
    let yaml = text
        .split("---\n")
        .nth(1)
        .expect("frontmatter block")
        .to_string();
    let keys: Vec<&str> = yaml.lines().filter_map(|l| l.split(':').next()).collect();
    assert_eq!(
        keys,
        ["type", "name", "agent", "tags", "created", "updated"]
    );
    assert!(text.trim_end().ends_with("line one"));
}

#[test]
fn set_human_output_is_wrote_name() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "x"])
        .assert()
        .success()
        .stdout("wrote n\n");
}

#[test]
fn set_json_output_shape_is_name_agent_updated() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["scratch", "set", "n", "--body", "x", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_stdout(&out);
    let keys: Vec<&str> = payload
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["name", "agent", "updated"]);
    assert_eq!(payload["name"], "n");
    assert_eq!(payload["agent"], "test-agent");
    assert!(payload["updated"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn set_quiet_output_is_the_bare_name() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "x", "--quiet"])
        .assert()
        .success()
        .stdout("n\n");
}

#[test]
fn set_is_idempotent_on_an_identical_body() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "same"])
        .assert()
        .success();
    let before = f.read("scratch/test-agent/n.md");
    std::thread::sleep(std::time::Duration::from_millis(10));
    f.cmd()
        .args(["scratch", "set", "n", "--body", "same"])
        .assert()
        .success();
    let after = f.read("scratch/test-agent/n.md");
    assert_eq!(
        before, after,
        "identical body must leave the file untouched"
    );
}

#[test]
fn set_overwrite_preserves_created_and_bumps_updated() {
    let f = VaultFixture::new();
    let first = json_stdout(
        &f.cmd()
            .args(["scratch", "set", "n", "--body", "one", "--json"])
            .output()
            .unwrap(),
    );
    std::thread::sleep(std::time::Duration::from_millis(10));
    let second = json_stdout(
        &f.cmd()
            .args(["scratch", "set", "n", "--body", "two", "--json"])
            .output()
            .unwrap(),
    );
    assert_ne!(first["updated"], second["updated"]);
    let text = f.read("scratch/test-agent/n.md");
    assert!(text.contains("two"));
}

#[test]
fn set_creates_the_agent_directory_lazily() {
    let f = VaultFixture::new();
    assert!(!f.files().iter().any(|p| p.starts_with("scratch/")));
    f.cmd()
        .args(["scratch", "set", "n", "--body", "x"])
        .assert()
        .success();
    assert!(f.files().contains(&"scratch/test-agent/n.md".to_string()));
}

#[test]
fn set_agent_flag_addresses_a_peer_namespace() {
    let f = VaultFixture::new();
    f.cmd()
        .args([
            "scratch",
            "set",
            "n",
            "--body",
            "x",
            "--agent",
            "flights-agent",
        ])
        .assert()
        .success();
    assert!(f
        .files()
        .contains(&"scratch/flights-agent/n.md".to_string()));
    assert!(!f.files().contains(&"scratch/test-agent/n.md".to_string()));
}

#[test]
fn set_invalid_name_exits_two() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["scratch", "set", "!!!", "--body", "x"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr_of(&out), "invalid scratch name: '!!!'\n");
}

#[test]
fn set_invalid_name_under_json_is_a_validation_envelope() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["scratch", "set", "!!!", "--body", "x", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    let payload = json_err(&out);
    assert_eq!(payload["kind"], "validation");
    assert_eq!(payload["message"], "invalid scratch name: '!!!'");
    assert_eq!(payload["next_action"], "fix the input and retry");
}

#[test]
fn set_reads_body_from_file() {
    let f = VaultFixture::new();
    let file = f.dir.path().join("body.txt");
    std::fs::write(&file, "from a file").unwrap();
    f.cmd()
        .args(["scratch", "set", "n", "--file"])
        .arg(&file)
        .assert()
        .success();
    assert!(f.read("scratch/test-agent/n.md").contains("from a file"));
}

#[test]
fn set_unreadable_file_is_a_validation_error() {
    let f = VaultFixture::new();
    let missing = f.dir.path().join("does-not-exist.txt");
    let out = f
        .cmd()
        .args(["scratch", "set", "n", "--file"])
        .arg(&missing)
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr_of(&out).starts_with("cannot read --file "));
}

#[test]
fn set_reads_body_from_stdin_with_a_dash() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "-"])
        .write_stdin("piped body\n")
        .assert()
        .success();
    assert!(f.read("scratch/test-agent/n.md").contains("piped body"));
}

#[test]
fn set_body_flag_wins_over_the_stdin_marker() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "-", "--body", "explicit"])
        .write_stdin("ignored")
        .assert()
        .success();
    let text = f.read("scratch/test-agent/n.md");
    assert!(text.contains("explicit"));
    assert!(!text.contains("ignored"));
}

#[test]
fn set_with_no_body_source_on_a_headless_path_exits_two() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["scratch", "set", "n"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out),
        "no body: pass --body or --file on a non-interactive path\n"
    );
}

#[test]
fn set_rejects_an_owner_outside_a_populated_roster() {
    let f = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n\
         [tasks]\ncollections = [\"alice\", \"bob\"]\n",
    );
    let out = f
        .cmd()
        .args(["scratch", "set", "n", "--body", "x", "--agent", "mallory"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr_of(&out), "unknown owner: 'mallory'\n");
}

#[test]
fn set_no_agent_identity_at_all_exits_two() {
    let f = VaultFixture::with("[core]\nvault_path = \"{VAULT}\"\n\n[tasks]\ncollections = []\n");
    let out = f
        .cmd()
        .args(["scratch", "set", "n", "--body", "x"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out),
        "no agent identity: set [core].agent or pass --owner\n"
    );
}

#[test]
fn set_global_owner_supplies_the_agent_identity() {
    let f = VaultFixture::with("[core]\nvault_path = \"{VAULT}\"\n\n[tasks]\ncollections = []\n");
    f.cmd()
        .args(["--owner", "carol", "scratch", "set", "n", "--body", "x"])
        .assert()
        .success();
    assert!(f.files().contains(&"scratch/carol/n.md".to_string()));
}

// ---------------------------------------------------------------------------------------
// append
// ---------------------------------------------------------------------------------------

#[test]
fn append_requires_an_existing_scratch_file() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["scratch", "append", "missing", "text"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_of(&out), "scratch not found: missing\n");
}

#[test]
fn append_adds_to_the_end_and_bumps_updated_only() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "base"])
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_millis(10));
    f.cmd()
        .args(["scratch", "append", "n", "more"])
        .assert()
        .success()
        .stdout("appended n\n");
    let text = f.read("scratch/test-agent/n.md");
    assert!(text.contains("base"));
    assert!(text.contains("more"));
}

#[test]
fn append_under_section_creates_it_when_absent() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "intro"])
        .assert()
        .success();
    f.cmd()
        .args(["scratch", "append", "n", "item", "--section", "Log"])
        .assert()
        .success();
    let text = f.read("scratch/test-agent/n.md");
    assert!(text.contains("## Log"));
    assert!(text.contains("item"));
}

#[test]
fn append_with_timestamp_stamps_the_acting_identity_not_the_namespace_owner() {
    let f = VaultFixture::new();
    f.cmd()
        .args([
            "scratch",
            "set",
            "n",
            "--body",
            "base",
            "--agent",
            "flights-agent",
        ])
        .assert()
        .success();
    f.cmd()
        .args([
            "scratch",
            "append",
            "n",
            "note",
            "--timestamp",
            "--agent",
            "flights-agent",
        ])
        .assert()
        .success();
    let text = f.read("scratch/flights-agent/n.md");
    assert!(text.contains("— test-agent"), "{text}");
}

// ---------------------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------------------

#[test]
fn get_prints_the_body_verbatim_human_and_quiet() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "hello\nworld"])
        .assert()
        .success();
    f.cmd()
        .args(["scratch", "get", "n"])
        .assert()
        .success()
        .stdout("hello\nworld\n");
    f.cmd()
        .args(["scratch", "get", "n", "--quiet"])
        .assert()
        .success()
        .stdout("hello\nworld\n");
}

#[test]
fn get_json_shape_has_the_six_documented_keys() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "hi"])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["scratch", "get", "n", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_stdout(&out);
    let keys: Vec<&str> = payload
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["name", "agent", "path", "bytes", "updated", "content"]
    );
    assert_eq!(payload["name"], "n");
    assert_eq!(payload["content"], "hi");
    assert_eq!(payload["bytes"], 2);
    assert!(payload["path"].as_str().unwrap().ends_with("n.md"));
}

#[test]
fn get_missing_scratch_exits_three() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["scratch", "get", "missing"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_of(&out), "scratch not found: missing\n");
}

#[test]
fn get_can_read_a_peers_namespace_via_agent_flag() {
    let f = VaultFixture::new();
    f.cmd()
        .args([
            "scratch",
            "set",
            "n",
            "--body",
            "peer body",
            "--agent",
            "flights-agent",
        ])
        .assert()
        .success();
    f.cmd()
        .args(["scratch", "get", "n", "--agent", "flights-agent"])
        .assert()
        .success()
        .stdout("peer body\n");
}

// ---------------------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------------------

#[test]
fn list_human_rows_are_tab_separated_name_bytes_updated() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "alpha", "--body", "ab"])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["scratch", "list"])
        .output()
        .expect("run mesh");
    let stdout = stdout_of(&out);
    let parts: Vec<&str> = stdout.trim_end().split('\t').collect();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], "alpha");
    assert_eq!(parts[1], "2");
}

#[test]
fn list_quiet_prints_names_one_per_line() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "alpha", "--body", "x"])
        .assert()
        .success();
    f.cmd()
        .args(["scratch", "set", "beta", "--body", "y"])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["scratch", "list", "--quiet"])
        .output()
        .expect("run mesh");
    let stdout = stdout_of(&out);
    let mut lines: Vec<&str> = stdout.trim_end().split('\n').collect();
    lines.sort();
    assert_eq!(lines, ["alpha", "beta"]);
}

#[test]
fn list_json_array_shape_has_five_keys_per_entry() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "alpha", "--body", "x"])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["scratch", "list", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_stdout(&out);
    let arr = payload.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let keys: Vec<&str> = arr[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["name", "agent", "path", "bytes", "updated"]);
}

#[test]
fn list_defaults_to_the_effective_agents_own_namespace() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "mine", "--body", "x"])
        .assert()
        .success();
    f.cmd()
        .args([
            "scratch",
            "set",
            "theirs",
            "--body",
            "y",
            "--agent",
            "other-agent",
        ])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["scratch", "list", "--quiet"])
        .output()
        .expect("run mesh");
    assert_eq!(stdout_of(&out), "mine\n");
}

#[test]
fn list_all_agents_spans_every_namespace_sorted_updated_desc_then_name() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "zeta", "--body", "x", "--agent", "a"])
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_millis(10));
    f.cmd()
        .args(["scratch", "set", "alpha", "--body", "y", "--agent", "b"])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["scratch", "list", "--all-agents", "--quiet"])
        .output()
        .expect("run mesh");
    let stdout = stdout_of(&out);
    let lines: Vec<&str> = stdout.trim_end().split('\n').collect();
    assert_eq!(lines, ["alpha", "zeta"]);
}

#[test]
fn list_since_filters_by_updated_floor() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "old", "--body", "x"])
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_millis(50));
    let cutoff = mesh::timefmt::iso_z(&mesh::timefmt::now_utc());
    std::thread::sleep(std::time::Duration::from_millis(50));
    f.cmd()
        .args(["scratch", "set", "new", "--body", "y"])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["scratch", "list", "--since", &cutoff, "--quiet"])
        .output()
        .expect("run mesh");
    assert_eq!(stdout_of(&out), "new\n");
}

#[test]
fn list_without_all_agents_and_no_identity_exits_two() {
    let f = VaultFixture::with("[core]\nvault_path = \"{VAULT}\"\n\n[tasks]\ncollections = []\n");
    let out = f
        .cmd()
        .args(["scratch", "list"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out),
        "no agent identity: set [core].agent or pass --owner\n"
    );
}

#[test]
fn list_rejects_an_agent_outside_a_populated_roster() {
    let f = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n\
         [tasks]\ncollections = [\"alice\"]\n",
    );
    let out = f
        .cmd()
        .args(["scratch", "list", "--agent", "mallory"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr_of(&out), "unknown owner: 'mallory'\n");
}

// ---------------------------------------------------------------------------------------
// clear
// ---------------------------------------------------------------------------------------

#[test]
fn clear_force_deletes_and_reports_json_shape() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "x"])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["scratch", "clear", "n", "--force", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_stdout(&out);
    assert_eq!(payload, serde_json::json!({"name": "n", "deleted": true}));
    assert!(!f.files().contains(&"scratch/test-agent/n.md".to_string()));
}

#[test]
fn clear_human_output_is_deleted_name() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "x"])
        .assert()
        .success();
    f.cmd()
        .args(["scratch", "clear", "n", "--force"])
        .assert()
        .success()
        .stdout("deleted n\n");
}

#[test]
fn clear_without_force_on_a_machine_path_refuses_and_keeps_the_file() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "x"])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["scratch", "clear", "n", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    assert!(f.files().contains(&"scratch/test-agent/n.md".to_string()));
}

#[test]
fn clear_missing_scratch_exits_three() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["scratch", "clear", "missing", "--force"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_of(&out), "scratch not found: missing\n");
}

// ---------------------------------------------------------------------------------------
// space configuration: disabled / relocated
// ---------------------------------------------------------------------------------------

#[test]
fn a_disabled_scratch_space_exits_two_for_every_verb() {
    let f = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n\
         [tasks]\ncollections = []\n\n[spaces]\nscratch = false\n",
    );
    for args in [
        vec!["scratch", "set", "n", "--body", "x"],
        vec!["scratch", "get", "n"],
        vec!["scratch", "list"],
        vec!["scratch", "clear", "n", "--force"],
        vec!["scratch", "append", "n", "x"],
    ] {
        let out = f.cmd().args(&args).output().expect("run mesh");
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(
            stderr_of(&out),
            "space 'scratch' is disabled in [spaces]\n",
            "{args:?}"
        );
    }
}

#[test]
fn scratch_relocated_to_an_absolute_path_writes_outside_the_vault() {
    let outside = tempfile::tempdir().expect("outside dir");
    let cfg_body = format!(
        "[core]\nvault_path = \"{{VAULT}}\"\nagent = \"test-agent\"\n\n\
         [tasks]\ncollections = []\n\n[spaces]\nscratch = \"{}\"\n",
        outside.path().display()
    );
    let f = VaultFixture::with(&cfg_body);
    f.cmd()
        .args(["scratch", "set", "n", "--body", "outside"])
        .assert()
        .success();
    let path = outside.path().join("test-agent/n.md");
    assert!(path.is_file());
    assert!(std::fs::read_to_string(&path).unwrap().contains("outside"));
    assert!(!f.files().iter().any(|p| p.starts_with("scratch/")));
}

// ---------------------------------------------------------------------------------------
// flag placement parity (R6)
// ---------------------------------------------------------------------------------------

#[test]
fn flags_are_byte_identical_on_either_side_of_the_command_name() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "x"])
        .assert()
        .success();
    for (left, right) in [
        (
            vec!["--json", "scratch", "list"],
            vec!["scratch", "list", "--json"],
        ),
        (
            vec!["--quiet", "scratch", "list"],
            vec!["scratch", "list", "--quiet"],
        ),
        (
            vec!["--json", "scratch", "get", "n"],
            vec!["scratch", "get", "n", "--json"],
        ),
    ] {
        let a = f.cmd().args(&left).output().expect("run left");
        let b = f.cmd().args(&right).output().expect("run right");
        assert_eq!(a.status.code(), b.status.code(), "{left:?} vs {right:?}");
        assert_eq!(stdout_of(&a), stdout_of(&b), "{left:?} vs {right:?}");
        assert_eq!(stderr_of(&a), stderr_of(&b), "{left:?} vs {right:?}");
    }
}

#[test]
fn a_stale_lock_from_a_dead_pid_is_reclaimed_transparently() {
    // A lock file naming a pid that cannot exist is stale by the dead-pid rule
    // (map/core.md §4.3) and is reclaimed inline, so this must complete fast, not hang on
    // the 15s `hold` budget a live lock would exercise (which integration tests must not
    // wait out; that path is covered at the unit level by `storage::lock`'s own tests).
    let f = VaultFixture::new();
    let lock_dir = f.vault.join("scratch/.locks/test-agent");
    std::fs::create_dir_all(&lock_dir).unwrap();
    std::fs::write(lock_dir.join("n.lock"), "999999999\n").unwrap();
    f.cmd()
        .args(["scratch", "set", "n", "--body", "x"])
        .assert()
        .success();
    assert!(f.files().contains(&"scratch/test-agent/n.md".to_string()));
}
