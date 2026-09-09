//! `init`, `status`, `reindex`, `config`, `completions` and the hidden `daemon` shim,
//! driven through the real binary.

mod common;

use std::path::{Path, PathBuf};

use common::VaultFixture;
use predicates::prelude::*;
use serde_json::Value as Json;

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json_of(out: &std::process::Output) -> Json {
    serde_json::from_str(stdout_of(out).trim()).expect("json stdout")
}

/// A config path that does not exist yet, plus a vault path to point `init` at.
struct InitBed {
    _dir: tempfile::TempDir,
    config: PathBuf,
    vault: PathBuf,
}

impl InitBed {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("config.toml");
        let vault = dir.path().join("vault");
        InitBed {
            _dir: dir,
            config,
            vault,
        }
    }
}

/// `mesh --config <fresh> …` on a clean environment (`bare_cmd` adds no `--config` of its own).
fn init_cmd(bed: &InitBed, fixture: &VaultFixture) -> assert_cmd::Command {
    let mut cmd = fixture.bare_cmd();
    cmd.arg("--config").arg(&bed.config);
    cmd
}

// --------------------------------------------------------------------------------------------
// init
// --------------------------------------------------------------------------------------------

#[test]
fn init_writes_the_documented_config_and_creates_the_vault() {
    let fixture = VaultFixture::new();
    let bed = InitBed::new();
    let out = init_cmd(&bed, &fixture)
        .args(["init", "--path"])
        .arg(&bed.vault)
        .output()
        .expect("run init");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert_eq!(
        stdout_of(&out),
        format!("wrote config to {}\n", bed.config.display())
    );
    assert!(bed.vault.is_dir(), "init creates vault_path");
    let written = std::fs::read_to_string(&bed.config).expect("read config");
    assert_eq!(
        written,
        format!(
            "[core]\nvault_path = \"{}\"\nagent = \"agent\"\n\n\
             [spaces]\nnotes = \"notes\"\ntasks = \"tasks\"\nmemories = \"memories\"\n\
             scratch = \"scratch\"\nassets = \"assets\"\n\n\
             [search]\nhybrid = true\n\n[tasks]\ncollections = []\n",
            bed.vault.display()
        )
    );
    assert!(written.ends_with('\n'));
    assert!(!written.contains("threshold"), "threshold stays unwritten");
    assert!(!written.contains("engine"), "engine = auto stays unwritten");
}

#[test]
fn the_config_init_writes_reloads() {
    let fixture = VaultFixture::new();
    let bed = InitBed::new();
    init_cmd(&bed, &fixture)
        .args(["init", "--path"])
        .arg(&bed.vault)
        .args(["--agent", "alice", "--collections", "alice, bob"])
        .assert()
        .success();
    let out = init_cmd(&bed, &fixture)
        .args(["--json", "config", "show"])
        .output()
        .expect("run config show");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    let payload = json_of(&out);
    assert_eq!(payload["core"]["agent"], Json::String("alice".into()));
    assert_eq!(
        payload["tasks"]["collections"],
        serde_json::json!(["alice", "bob"])
    );
}

#[test]
fn init_json_carries_exactly_three_keys_in_order() {
    let fixture = VaultFixture::new();
    let bed = InitBed::new();
    let out = init_cmd(&bed, &fixture)
        .args(["--json", "init", "--path"])
        .arg(&bed.vault)
        .args(["--agent", "zoe"])
        .output()
        .expect("run init");
    assert_eq!(out.status.code(), Some(0));
    let payload = json_of(&out);
    let keys: Vec<&str> = payload
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["path", "vault_path", "agent"]);
    assert_eq!(
        payload["path"],
        Json::String(bed.config.display().to_string())
    );
    assert_eq!(
        payload["vault_path"],
        Json::String(bed.vault.display().to_string())
    );
    assert_eq!(payload["agent"], Json::String("zoe".into()));
}

#[test]
fn init_quiet_prints_nothing_at_all() {
    let fixture = VaultFixture::new();
    let bed = InitBed::new();
    let out = init_cmd(&bed, &fixture)
        .args(["--quiet", "init", "--path"])
        .arg(&bed.vault)
        .output()
        .expect("run init");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out), "");
    assert_eq!(stderr_of(&out), "");
    assert!(bed.config.is_file());
}

#[test]
fn init_refuses_an_existing_config_and_never_opens_it_for_writing() {
    let fixture = VaultFixture::new();
    let bed = InitBed::new();
    std::fs::write(&bed.config, "# hand written\nkeep = true\n").expect("seed config");
    let before = std::fs::metadata(&bed.config).expect("stat");
    let out = init_cmd(&bed, &fixture)
        .args(["init", "--path"])
        .arg(&bed.vault)
        .output()
        .expect("run init");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out),
        format!(
            "config already exists at {} — pass --force to overwrite\n",
            bed.config.display()
        )
    );
    assert_eq!(
        std::fs::read_to_string(&bed.config).expect("read"),
        "# hand written\nkeep = true\n"
    );
    let after = std::fs::metadata(&bed.config).expect("stat");
    assert_eq!(before.len(), after.len());
    assert!(!bed.vault.exists(), "a refusal creates no vault either");
}

#[test]
fn the_refusal_envelope_is_a_validation_error() {
    let fixture = VaultFixture::new();
    let bed = InitBed::new();
    std::fs::write(&bed.config, "x = 1\n").expect("seed config");
    let out = init_cmd(&bed, &fixture)
        .args(["--json", "init"])
        .output()
        .expect("run init");
    assert_eq!(out.status.code(), Some(2));
    let payload: Json = serde_json::from_str(stderr_of(&out).trim()).expect("envelope");
    assert_eq!(payload["kind"], "validation");
    assert!(payload["message"]
        .as_str()
        .expect("message")
        .ends_with("— pass --force to overwrite"));
}

#[test]
fn init_force_overwrites() {
    let fixture = VaultFixture::new();
    let bed = InitBed::new();
    std::fs::write(&bed.config, "# hand written\n").expect("seed config");
    init_cmd(&bed, &fixture)
        .args(["init", "--force", "--path"])
        .arg(&bed.vault)
        .assert()
        .success();
    let written = std::fs::read_to_string(&bed.config).expect("read");
    assert!(!written.contains("hand written"));
    assert!(written.starts_with("[core]\n"));
}

#[test]
fn init_honours_every_switch() {
    let fixture = VaultFixture::new();
    let bed = InitBed::new();
    init_cmd(&bed, &fixture)
        .args(["init", "--path"])
        .arg(&bed.vault)
        .args([
            "--agent",
            "demo",
            "--collections",
            "a, b ,,c",
            "--search-collection",
            "myvault",
            "--threshold",
            "0.4",
            "--engine",
            "builtin",
            "--no-hybrid",
            "--no-spaces",
        ])
        .assert()
        .success();
    let written = std::fs::read_to_string(&bed.config).expect("read");
    assert_eq!(
        written,
        format!(
            "[core]\nvault_path = \"{}\"\nagent = \"demo\"\n\n\
             [search]\nhybrid = false\nthreshold = 0.4\ncollection = \"myvault\"\n\
             engine = \"builtin\"\n\n[tasks]\ncollections = [\"a\", \"b\", \"c\"]\n",
            bed.vault.display()
        )
    );
    // And it still loads.
    init_cmd(&bed, &fixture)
        .args(["config", "get", "search.engine"])
        .assert()
        .success()
        .stdout("builtin\n");
}

#[test]
fn init_obsidian_points_notes_at_the_vault_root() {
    let fixture = VaultFixture::new();
    let bed = InitBed::new();
    init_cmd(&bed, &fixture)
        .args(["init", "--obsidian", "--path"])
        .arg(&bed.vault)
        .assert()
        .success();
    let written = std::fs::read_to_string(&bed.config).expect("read");
    assert!(written.contains("notes = \".\""), "{written}");
    assert!(written.contains("tasks = \"tasks\""), "{written}");
    let out = init_cmd(&bed, &fixture)
        .args(["--json", "config", "show"])
        .output()
        .expect("run config show");
    let payload = json_of(&out);
    assert_eq!(payload["spaces"]["notes"], payload["core"]["vault_path"]);
}

#[test]
fn init_expands_a_leading_tilde() {
    let fixture = VaultFixture::new();
    let bed = InitBed::new();
    // `~` alone is the home directory, which already exists, so nothing is scattered.
    let out = init_cmd(&bed, &fixture)
        .args(["--json", "init", "--path", "~"])
        .output()
        .expect("run init");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    let vault = json_of(&out)["vault_path"]
        .as_str()
        .expect("vault_path")
        .to_string();
    assert!(!vault.contains('~'), "the tilde must be expanded: {vault}");
    assert!(Path::new(&vault).is_absolute(), "{vault}");
}

#[test]
fn init_rejects_an_unknown_engine_before_writing_anything() {
    let fixture = VaultFixture::new();
    let bed = InitBed::new();
    let out = init_cmd(&bed, &fixture)
        .args(["init", "--engine", "magic", "--path"])
        .arg(&bed.vault)
        .output()
        .expect("run init");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out),
        "invalid engine: 'magic' (use auto, indexed, builtin, substring)\n"
    );
    assert!(!bed.config.exists());
    assert!(!bed.vault.exists());
}

// --------------------------------------------------------------------------------------------
// status
// --------------------------------------------------------------------------------------------

#[test]
fn status_is_an_object_and_exits_zero() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run status");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert!(json_of(&out).is_object());
}

#[test]
fn status_exits_zero_with_a_missing_vault() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("not-here");
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!("[core]\nvault_path = \"{}\"\n", missing.display()),
    )
    .expect("write config");
    let mut cmd = assert_cmd::Command::cargo_bin("mesh").expect("mesh binary");
    let out = cmd
        .env_remove("MESH_CONFIG_PATH")
        .env_remove("MESH_AGENT")
        .env_remove("MESH_VAULT")
        .arg("--config")
        .arg(&config)
        .arg("status")
        .output()
        .expect("run status");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert!(!missing.exists(), "status creates nothing");
}

#[test]
fn a_daemon_table_earns_exactly_one_notice() {
    let fixture = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n[daemon]\nsocket = \"x\"\n",
    );
    let out = fixture.cmd().arg("status").output().expect("run status");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stderr_of(&out),
        "config: [daemon] is ignored — the daemon was removed; see 'mesh watch'\n"
    );
    // `--quiet` suppresses the notice; the payload still prints.
    let quiet = fixture
        .cmd()
        .args(["--quiet", "status"])
        .output()
        .expect("run status");
    assert_eq!(stderr_of(&quiet), "");
}

#[test]
fn a_config_without_a_daemon_table_is_silent() {
    let fixture = VaultFixture::new();
    let out = fixture.cmd().arg("status").output().expect("run status");
    assert_eq!(stderr_of(&out), "");
}

#[test]
fn status_touches_nothing_in_the_vault() {
    let fixture = VaultFixture::new();
    fixture.write("notes/n-AAAA.md", "---\nid: n-AAAA\ntype: note\n---\n\nx\n");
    let before = fixture.files();
    let text_before = fixture.read("notes/n-AAAA.md");
    fixture.cmd().arg("status").assert().success();
    assert_eq!(fixture.files(), before);
    assert_eq!(fixture.read("notes/n-AAAA.md"), text_before);
}

// --------------------------------------------------------------------------------------------
// reindex
// --------------------------------------------------------------------------------------------

#[test]
fn reindex_is_a_silent_no_op_without_a_collection() {
    let fixture = VaultFixture::new();
    let out = fixture.cmd().arg("reindex").output().expect("run reindex");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out), "");
    assert_eq!(stderr_of(&out), "");
}

#[test]
fn reindex_with_a_collection_still_exits_zero() {
    let fixture = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n\
         [search]\ncollection = \"demo\"\n",
    );
    let out = fixture.cmd().arg("reindex").output().expect("run reindex");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
}

#[test]
fn reindex_accepts_a_space_csv_and_rejects_nonsense() {
    let fixture = VaultFixture::new();
    fixture
        .cmd()
        .args(["reindex", "--space", "notes,tasks"])
        .assert()
        .success();
    let out = fixture
        .cmd()
        .args(["reindex", "--space", "nope"])
        .output()
        .expect("run reindex");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out),
        "unknown space: 'nope' (use notes, tasks, memories, scratch, assets)\n"
    );
}

#[test]
fn reindex_reports_a_disabled_space() {
    let fixture =
        VaultFixture::with("[core]\nvault_path = \"{VAULT}\"\n\n[spaces]\nscratch = false\n");
    let out = fixture
        .cmd()
        .args(["reindex", "--space", "scratch"])
        .output()
        .expect("run reindex");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr_of(&out), "space 'scratch' is disabled in [spaces]\n");
}

// --------------------------------------------------------------------------------------------
// config
// --------------------------------------------------------------------------------------------

#[test]
fn config_path_answers_without_a_config_file() {
    let fixture = VaultFixture::new();
    let out = fixture
        .bare_cmd()
        .args(["config", "path"])
        .output()
        .expect("run config path");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert!(
        stdout_of(&out).trim().ends_with("missing.toml"),
        "{}",
        stdout_of(&out)
    );
}

#[test]
fn config_path_json_carries_the_path_key() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["--json", "config", "path"])
        .output()
        .expect("run config path");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        json_of(&out)["path"],
        Json::String(fixture.config.display().to_string())
    );
}

#[test]
fn config_show_prints_loadable_toml_by_default() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["config", "show"])
        .output()
        .expect("run config show");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    let parsed: toml::Table = text.parse().expect("valid toml");
    assert!(parsed.contains_key("core"));
    assert!(parsed.contains_key("spaces"));
    assert!(parsed.contains_key("search"));
    assert!(parsed.contains_key("tasks"));
    assert!(parsed.contains_key("sandbox"));
    assert!(parsed.contains_key("config_path"));
    // Every space is resolved to an absolute path.
    let spaces = parsed["spaces"].as_table().expect("spaces table");
    for name in ["notes", "tasks", "memories", "scratch", "assets"] {
        let value = spaces[name].as_str().expect("space path");
        assert!(Path::new(value).is_absolute(), "{name} = {value}");
    }
}

#[test]
fn config_show_json_lists_the_sandbox_roots() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["--json", "config", "show"])
        .output()
        .expect("run config show");
    let payload = json_of(&out);
    assert_eq!(
        payload["sandbox"].as_array().expect("sandbox").len(),
        5,
        "{payload}"
    );
    assert_eq!(payload["core"]["agent"], Json::String("test-agent".into()));
    assert_eq!(payload["search"]["hybrid"], Json::Bool(true));
    assert_eq!(payload["search"]["threshold_explicit"], Json::Bool(false));
}

#[test]
fn config_show_takes_its_json_flag_on_either_side() {
    let fixture = VaultFixture::new();
    let left = fixture
        .cmd()
        .args(["--json", "config", "show"])
        .output()
        .expect("run");
    let right = fixture
        .cmd()
        .args(["config", "show", "--json"])
        .output()
        .expect("run");
    assert_eq!(left.status.code(), right.status.code());
    assert_eq!(stdout_of(&left), stdout_of(&right));
}

#[test]
fn config_show_reports_a_disabled_space_as_false() {
    let fixture =
        VaultFixture::with("[core]\nvault_path = \"{VAULT}\"\n\n[spaces]\nscratch = false\n");
    let out = fixture
        .cmd()
        .args(["--json", "config", "show"])
        .output()
        .expect("run config show");
    assert_eq!(json_of(&out)["spaces"]["scratch"], Json::Bool(false));
    let text = stdout_of(
        &fixture
            .cmd()
            .args(["config", "show"])
            .output()
            .expect("run"),
    );
    assert!(text.contains("scratch = false"), "{text}");
}

#[test]
fn config_get_walks_a_dotted_key() {
    let fixture = VaultFixture::new();
    fixture
        .cmd()
        .args(["config", "get", "core.agent"])
        .assert()
        .success()
        .stdout("test-agent\n");
    fixture
        .cmd()
        .args(["config", "get", "search.hybrid"])
        .assert()
        .success()
        .stdout("true\n");
    let out = fixture
        .cmd()
        .args(["--json", "config", "get", "core.vault_path"])
        .output()
        .expect("run config get");
    let value: Json = serde_json::from_str(stdout_of(&out).trim()).expect("json");
    assert_eq!(
        value,
        Json::String(
            std::fs::canonicalize(&fixture.vault)
                .expect("canonicalize")
                .display()
                .to_string()
        )
    );
}

#[test]
fn config_get_renders_a_list_and_an_unset_value() {
    let fixture = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\n\n[tasks]\ncollections = [\"a\", \"b\"]\n",
    );
    fixture
        .cmd()
        .args(["config", "get", "tasks.collections"])
        .assert()
        .success()
        .stdout("a, b\n");
    fixture
        .cmd()
        .args(["config", "get", "search.collection"])
        .assert()
        .success()
        .stdout("\n");
}

#[test]
fn config_get_rejects_an_unknown_key() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["config", "get", "core.nope"])
        .output()
        .expect("run config get");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr_of(&out), "unknown config key: 'core.nope'\n");
}

#[test]
fn config_set_preserves_comments_and_ordering() {
    let dir = tempfile::tempdir().expect("tempdir");
    let vault = dir.path().join("vault");
    std::fs::create_dir_all(&vault).expect("vault");
    let config = dir.path().join("config.toml");
    let original = format!(
        "# mesh config — hand written\n[core]\n\
         vault_path = \"{}\"   # where the notes live\nagent = \"old\"\n\n\
         # search settings\n[search]\nhybrid = true\n",
        vault.display()
    );
    std::fs::write(&config, &original).expect("write config");

    let mut cmd = assert_cmd::Command::cargo_bin("mesh").expect("mesh binary");
    let out = cmd
        .env_remove("MESH_CONFIG_PATH")
        .env_remove("MESH_AGENT")
        .env_remove("MESH_VAULT")
        .arg("--config")
        .arg(&config)
        .args(["config", "set", "core.agent", "new"])
        .output()
        .expect("run config set");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert_eq!(stdout_of(&out), "set core.agent\n");

    let after = std::fs::read_to_string(&config).expect("read");
    assert!(after.contains("# mesh config — hand written"), "{after}");
    assert!(after.contains("# where the notes live"), "{after}");
    assert!(after.contains("# search settings"), "{after}");
    assert!(after.contains("agent = \"new\""), "{after}");
    assert!(!after.contains("\"old\""), "{after}");
    assert!(
        after.find("[core]").expect("core") < after.find("[search]").expect("search"),
        "table order survives"
    );
}

#[test]
fn config_set_parses_toml_scalars_and_falls_back_to_a_string() {
    let fixture = VaultFixture::new();
    for (key, value, expect) in [
        ("search.hybrid", "false", "hybrid = false"),
        ("tasks.strict", "true", "strict = true"),
        ("search.threshold", "0.4", "threshold = 0.4"),
        (
            "tasks.collections",
            "[\"a\", \"b\"]",
            "collections = [\"a\", \"b\"]",
        ),
        ("core.agent", "bare-word", "agent = \"bare-word\""),
        ("spaces.notes", ".", "notes = \".\""),
    ] {
        fixture
            .cmd()
            .args(["config", "set", key, value])
            .assert()
            .success();
        let after = std::fs::read_to_string(&fixture.config).expect("read");
        assert!(after.contains(expect), "{key} = {value}: {after}");
    }
    // The file still loads after all of that.
    fixture.cmd().args(["config", "show"]).assert().success();
}

#[test]
fn config_set_creates_a_missing_table() {
    let fixture = VaultFixture::new();
    assert!(!fixture
        .cmd()
        .get_args()
        .any(|a| a.to_string_lossy() == "[search]"));
    fixture
        .cmd()
        .args(["config", "set", "search.collection", "demo"])
        .assert()
        .success();
    let after = std::fs::read_to_string(&fixture.config).expect("read");
    assert!(after.contains("[search]"), "{after}");
    fixture
        .cmd()
        .args(["config", "get", "search.collection"])
        .assert()
        .success()
        .stdout("demo\n");
}

#[test]
fn config_set_json_and_quiet_follow_class_m() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["--json", "config", "set", "core.agent", "zed"])
        .output()
        .expect("run config set");
    let payload = json_of(&out);
    assert_eq!(payload["name"], Json::String("core.agent".into()));
    assert_eq!(payload["value"], Json::String("\"zed\"".into()));

    let quiet = fixture
        .cmd()
        .args(["--quiet", "--json", "config", "set", "core.agent", "yak"])
        .output()
        .expect("run config set");
    assert_eq!(stdout_of(&quiet), "core.agent\n", "quiet beats json");
}

#[test]
fn config_set_rejects_a_key_that_is_not_dotted() {
    let fixture = VaultFixture::new();
    for key in ["agent", "core.agent.deep", ""] {
        let out = fixture
            .cmd()
            .args(["config", "set", key, "x"])
            .output()
            .expect("run config set");
        assert_eq!(out.status.code(), Some(2), "{key}");
        assert!(
            stderr_of(&out).contains("config set expects a dotted key like core.agent"),
            "{key}: {}",
            stderr_of(&out)
        );
    }
}

#[test]
fn every_config_form_except_path_needs_a_config_file() {
    let fixture = VaultFixture::new();
    for args in [
        vec!["config", "show"],
        vec!["config", "get", "core.agent"],
        vec!["config", "set", "core.agent", "x"],
    ] {
        let out = fixture.bare_cmd().args(&args).output().expect("run");
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            stderr_of(&out).starts_with("mesh: no config found at "),
            "{args:?}: {}",
            stderr_of(&out)
        );
    }
}

// --------------------------------------------------------------------------------------------
// completions
// --------------------------------------------------------------------------------------------

#[test]
fn completions_render_for_every_supported_shell() {
    let fixture = VaultFixture::new();
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let out = fixture
            .cmd()
            .args(["completions", shell])
            .output()
            .expect("run completions");
        assert_eq!(out.status.code(), Some(0), "{shell}: {}", stderr_of(&out));
        let script = stdout_of(&out);
        assert!(script.len() > 100, "{shell} script looks empty: {script}");
        assert!(script.contains("mesh"), "{shell} script must name mesh");
        assert_eq!(stderr_of(&out), "", "{shell}");
    }
}

#[test]
fn completions_never_need_a_config_file() {
    let fixture = VaultFixture::new();
    fixture
        .bare_cmd()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mesh"));
}

// --------------------------------------------------------------------------------------------
// the daemon shim
// --------------------------------------------------------------------------------------------

#[test]
fn daemon_status_reports_the_watch_lock_and_a_stopped_watcher() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["daemon", "status"])
        .output()
        .expect("run daemon status");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    let human = stdout_of(&out);
    assert!(human.starts_with("stopped — socket "), "{human}");
    assert!(human.trim_end().ends_with(".watch.lock"), "{human}");

    let json = fixture
        .cmd()
        .args(["--json", "daemon", "status"])
        .output()
        .expect("run daemon status");
    let payload = json_of(&json);
    let keys: Vec<&str> = payload
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["running", "pid", "socket"]);
    assert_eq!(payload["running"], Json::Bool(false));
    assert_eq!(payload["pid"], Json::Null);
    assert!(payload["socket"]
        .as_str()
        .expect("socket")
        .ends_with(".watch.lock"));
}

#[test]
fn daemon_start_never_spawns_and_says_so() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["daemon", "start"])
        .output()
        .expect("run daemon start");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stderr_of(&out), "daemon: removed — use 'mesh watch'\n");
    assert_eq!(stdout_of(&out), "daemon not running\n");

    let json = fixture
        .cmd()
        .args(["--json", "daemon", "start"])
        .output()
        .expect("run daemon start");
    let payload = json_of(&json);
    let keys: Vec<&str> = payload
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["running", "started", "pid"]);
    assert_eq!(payload["started"], Json::Bool(false));
}

#[test]
fn daemon_stop_is_idempotent_when_nothing_runs() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["daemon", "stop"])
        .output()
        .expect("run daemon stop");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out), "daemon not running\n");
    let json = fixture
        .cmd()
        .args(["--json", "daemon", "stop"])
        .output()
        .expect("run daemon stop");
    let payload = json_of(&json);
    assert_eq!(payload["running"], Json::Bool(false));
    assert_eq!(payload["stopped"], Json::Bool(false));
    assert!(
        payload.get("pid").is_none(),
        "the not-running payload carries no pid key: {payload}"
    );
}

#[test]
fn the_daemon_shim_stays_hidden_from_help() {
    let fixture = VaultFixture::new();
    let out = fixture.cmd().arg("--help").output().expect("run help");
    assert!(
        !stdout_of(&out).contains("\n  daemon"),
        "{}",
        stdout_of(&out)
    );
    // It is still reachable.
    fixture.cmd().args(["daemon", "status"]).assert().success();
}
