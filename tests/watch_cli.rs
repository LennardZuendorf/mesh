//! `mesh watch`: the singleton lock, the `--once` sweep, and every reconcile guard.

mod common;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::VaultFixture;
use serde_json::Value as Json;

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

const TASK_DONE: &str = "---\nid: t-AAAA\ntype: task\ntitle: Ship it\nstatus: done\n\
                         reviewer: alice\n---\n\nThe body.\n";
const NOTE_DECISION: &str =
    "---\nid: n-BBBB\ntype: decision\ntitle: Pick one\nextra: keep\n---\n\nThe body.\n";

/// The watch lock for this fixture's vault, straight from the `daemon` shim.
fn watch_lock(fixture: &VaultFixture) -> PathBuf {
    let out = fixture
        .cmd()
        .args(["--json", "daemon", "status"])
        .output()
        .expect("run daemon status");
    let payload: Json = serde_json::from_str(stdout_of(&out).trim()).expect("json");
    PathBuf::from(payload["socket"].as_str().expect("socket path"))
}

/// Claim the watch lock with a pid that is definitely alive: this test process.
fn hold_lock(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("lock dir");
    }
    std::fs::write(path, format!("{}\n", std::process::id())).expect("write lock");
}

// --------------------------------------------------------------------------------------------
// --once
// --------------------------------------------------------------------------------------------

#[test]
fn once_moves_a_misfiled_task_byte_for_byte() {
    let fixture = VaultFixture::new();
    fixture.write("tasks/open/t-AAAA.md", TASK_DONE);
    let out = fixture
        .cmd()
        .args(["watch", "--once", "--no-index"])
        .output()
        .expect("run watch");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert_eq!(
        fixture.files(),
        vec!["tasks/done/t-AAAA.md".to_string()],
        "the task moved into tasks/done"
    );
    assert_eq!(
        fixture.read("tasks/done/t-AAAA.md"),
        TASK_DONE,
        "a reconcile move never reserialises"
    );
    assert!(
        fixture
            .read("tasks/done/t-AAAA.md")
            .contains("reviewer: alice"),
        "unknown keys survive"
    );
}

#[test]
fn once_moves_a_misfiled_note_into_its_type_folder() {
    let fixture = VaultFixture::new();
    fixture.write("notes/n-BBBB.md", NOTE_DECISION);
    fixture
        .cmd()
        .args(["watch", "--once", "--no-index"])
        .assert()
        .success();
    assert_eq!(
        fixture.files(),
        vec!["notes/decisions/n-BBBB.md".to_string()]
    );
    assert_eq!(fixture.read("notes/decisions/n-BBBB.md"), NOTE_DECISION);
}

#[test]
fn once_is_idempotent() {
    let fixture = VaultFixture::new();
    fixture.write("tasks/open/t-AAAA.md", TASK_DONE);
    for _ in 0..3 {
        fixture
            .cmd()
            .args(["watch", "--once", "--no-index"])
            .assert()
            .success();
    }
    assert_eq!(fixture.files(), vec!["tasks/done/t-AAAA.md".to_string()]);
}

#[test]
fn once_leaves_every_hostile_file_exactly_where_it_is() {
    let fixture = VaultFixture::new();
    let hostile: Vec<(&str, &str)> = vec![
        // A sidecar carrying mesh-shaped frontmatter is not Markdown.
        ("notes/sidecar.txt", TASK_DONE),
        // Malformed YAML.
        (
            "notes/malformed.md",
            "---\nid: n-CCCC\n  bad: [\n---\n\nx\n",
        ),
        // A foreign file with no mesh id.
        (
            "notes/foreign.md",
            "---\ntitle: Foreign\ntype: decision\n---\n\nx\n",
        ),
        // An unknown type and an unknown status.
        (
            "notes/bogus-type.md",
            "---\nid: n-DDDD\ntype: wat\ntitle: X\n---\n\nx\n",
        ),
        (
            "tasks/open/bogus-status.md",
            "---\nid: t-EEEE\ntype: task\nstatus: wat\n---\n\nx\n",
        ),
        // Nothing to read at all.
        ("notes/empty.md", ""),
        ("notes/no-frontmatter.md", "# Heading only\n"),
        // A memory is not a note, whatever its type says.
        (
            "memories/m-AAAA.md",
            "---\nid: m-AAAA\ntype: note\ntitle: X\n---\n\nx\n",
        ),
    ];
    for (rel, body) in &hostile {
        fixture.write(rel, body);
    }
    let before = fixture.files();
    let out = fixture
        .cmd()
        .args(["watch", "--once", "--no-index"])
        .output()
        .expect("run watch");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert_eq!(fixture.files(), before, "no hostile file moved");
    for (rel, body) in &hostile {
        assert_eq!(&fixture.read(rel), body, "{rel} changed");
    }
}

#[test]
fn once_with_no_reconcile_moves_nothing() {
    let fixture = VaultFixture::new();
    fixture.write("tasks/open/t-AAAA.md", TASK_DONE);
    fixture
        .cmd()
        .args(["watch", "--once", "--no-index", "--no-reconcile"])
        .assert()
        .success();
    assert_eq!(fixture.files(), vec!["tasks/open/t-AAAA.md".to_string()]);
}

#[test]
fn once_emits_ndjson_events_under_json() {
    let fixture = VaultFixture::new();
    fixture.write("tasks/open/t-AAAA.md", TASK_DONE);
    let out = fixture
        .cmd()
        .args(["--json", "watch", "--once", "--no-index"])
        .output()
        .expect("run watch");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    let lines: Vec<Json> = stdout_of(&out)
        .lines()
        .map(|line| serde_json::from_str(line).expect("one json object per line"))
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0]["event"], "reconcile");
    assert!(lines[0]["path"]
        .as_str()
        .expect("path")
        .ends_with("tasks/open/t-AAAA.md"));
    assert!(lines[0]["to"]
        .as_str()
        .expect("to")
        .ends_with("tasks/done/t-AAAA.md"));
    assert!(lines[0]["ts"].as_str().expect("ts").ends_with('Z'));
    assert_eq!(lines[1]["event"], "sweep");
    assert_eq!(lines[1]["reconciled"], Json::from(1));
    assert_eq!(lines[1]["indexed"], Json::Bool(false));
}

#[test]
fn once_on_an_empty_vault_is_a_clean_no_op() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["--json", "watch", "--once", "--no-index"])
        .output()
        .expect("run watch");
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1);
    let sweep: Json = serde_json::from_str(lines[0]).expect("json");
    assert_eq!(sweep["reconciled"], Json::from(0));
    assert!(fixture.files().is_empty(), "no folders were scattered");
}

#[test]
fn once_scoped_to_one_space_ignores_the_others() {
    let fixture = VaultFixture::new();
    fixture.write("tasks/open/t-AAAA.md", TASK_DONE);
    fixture.write("notes/n-BBBB.md", NOTE_DECISION);
    fixture
        .cmd()
        .args(["watch", "--once", "--no-index", "--space", "notes"])
        .assert()
        .success();
    assert_eq!(
        fixture.files(),
        vec![
            "notes/decisions/n-BBBB.md".to_string(),
            "tasks/open/t-AAAA.md".to_string()
        ]
    );
}

#[test]
fn an_unknown_space_is_a_validation_error() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["watch", "--once", "--space", "nope"])
        .output()
        .expect("run watch");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out),
        "unknown space: 'nope' (use notes, tasks, memories, scratch, assets)\n"
    );
}

#[test]
fn a_missing_indexed_binary_is_one_notice_not_a_failure() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["watch", "--once"])
        .output()
        .expect("run watch");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stderr_of(&out),
        "watch: indexed unavailable — watching for reconcile only\n"
    );
    // `--no-index` never mentions indexed at all, and `--quiet` suppresses the notice.
    let silent = fixture
        .cmd()
        .args(["watch", "--once", "--no-index"])
        .output()
        .expect("run watch");
    assert_eq!(stderr_of(&silent), "");
    let quiet = fixture
        .cmd()
        .args(["--quiet", "watch", "--once"])
        .output()
        .expect("run watch");
    assert_eq!(stderr_of(&quiet), "");
}

// --------------------------------------------------------------------------------------------
// the singleton lock
// --------------------------------------------------------------------------------------------

#[test]
fn a_second_watcher_exits_four() {
    let fixture = VaultFixture::new();
    let lock = watch_lock(&fixture);
    hold_lock(&lock);
    let out = fixture
        .cmd()
        .args(["watch", "--once", "--no-index"])
        .output()
        .expect("run watch");
    let _ = std::fs::remove_file(&lock);
    assert_eq!(out.status.code(), Some(4), "{}", stderr_of(&out));
    assert_eq!(
        stderr_of(&out),
        format!("watch: already running (pid {})\n", std::process::id())
    );
}

#[test]
fn the_second_watcher_envelope_is_a_lock_conflict() {
    let fixture = VaultFixture::new();
    let lock = watch_lock(&fixture);
    hold_lock(&lock);
    let out = fixture
        .cmd()
        .args(["--json", "watch", "--once", "--no-index"])
        .output()
        .expect("run watch");
    let _ = std::fs::remove_file(&lock);
    assert_eq!(out.status.code(), Some(4));
    let payload: Json = serde_json::from_str(stderr_of(&out).trim()).expect("envelope");
    assert_eq!(payload["kind"], "lock_conflict");
    assert!(payload["message"]
        .as_str()
        .expect("message")
        .starts_with("watch: already running (pid "));
    assert!(payload["retry_after_ms"].is_number());
}

#[test]
fn a_held_lock_makes_the_daemon_shim_report_a_running_watcher() {
    let fixture = VaultFixture::new();
    let lock = watch_lock(&fixture);
    hold_lock(&lock);
    let me = std::process::id();

    let status = fixture
        .cmd()
        .args(["--json", "daemon", "status"])
        .output()
        .expect("run daemon status");
    let payload: Json = serde_json::from_str(stdout_of(&status).trim()).expect("json");
    assert_eq!(payload["running"], Json::Bool(true));
    assert_eq!(payload["pid"], Json::from(me));

    let human = fixture
        .cmd()
        .args(["daemon", "status"])
        .output()
        .expect("run daemon status");
    assert!(
        stdout_of(&human).starts_with(&format!("running (pid {me}) — socket ")),
        "{}",
        stdout_of(&human)
    );

    let start = fixture
        .cmd()
        .args(["daemon", "start"])
        .output()
        .expect("run daemon start");
    assert_eq!(
        stdout_of(&start),
        format!("daemon already running (pid {me})\n")
    );
    let _ = std::fs::remove_file(&lock);
}

#[test]
fn a_stale_lock_reads_as_not_running_and_is_reclaimed() {
    let fixture = VaultFixture::new();
    let lock = watch_lock(&fixture);
    if let Some(parent) = lock.parent() {
        std::fs::create_dir_all(parent).expect("lock dir");
    }
    // The maximum Linux pid is 4194304; nothing is alive there.
    std::fs::write(&lock, "4194303\n").expect("write lock");
    let status = fixture
        .cmd()
        .args(["--json", "daemon", "status"])
        .output()
        .expect("run daemon status");
    let payload: Json = serde_json::from_str(stdout_of(&status).trim()).expect("json");
    assert_eq!(payload["running"], Json::Bool(false));
    assert_eq!(payload["pid"], Json::Null);

    // And a fresh watcher takes the lock over rather than exiting 4.
    let out = fixture
        .cmd()
        .args(["watch", "--once", "--no-index"])
        .output()
        .expect("run watch");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    let _ = std::fs::remove_file(&lock);
}

#[test]
fn the_lock_is_released_when_the_sweep_ends() {
    let fixture = VaultFixture::new();
    let lock = watch_lock(&fixture);
    assert!(!lock.exists(), "no watcher yet");
    fixture
        .cmd()
        .args(["watch", "--once", "--no-index"])
        .assert()
        .success();
    assert!(!lock.exists(), "the guard unlinks the lock on the way out");
}

// --------------------------------------------------------------------------------------------
// the live loop
// --------------------------------------------------------------------------------------------

/// Spawn a real watcher, wait for its `start` event, drop a file in, and read the event it
/// produces. Bounded by a hard deadline so a stalled inotify can never hang the suite.
#[test]
fn a_live_watcher_reports_the_file_it_healed() {
    let fixture = VaultFixture::new();
    // Create the leaf directory up front: a recursive watch registers a brand-new subtree
    // asynchronously, so a file written into a folder that did not exist at spawn time is a
    // race, not a contract.
    std::fs::create_dir_all(fixture.vault.join("tasks/open")).expect("tasks/open");
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("mesh"))
        .env_remove("MESH_CONFIG_PATH")
        .env_remove("MESH_AGENT")
        .env_remove("MESH_VAULT")
        .env_remove("MESH_INDEXED_BIN")
        .arg("--config")
        .arg(&fixture.config)
        .args(["--json", "watch", "--no-index", "--debounce", "50"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn watch");

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });

    let deadline = Duration::from_secs(5);
    let start: Json = serde_json::from_str(
        &rx.recv_timeout(deadline)
            .expect("the watcher must announce itself"),
    )
    .expect("json");
    assert_eq!(start["event"], "start");
    assert!(start["pid"].is_number());

    // The watcher creates every space root before it starts listening.
    fixture.write("tasks/open/t-AAAA.md", TASK_DONE);

    let began = Instant::now();
    let mut healed = false;
    while began.elapsed() < deadline {
        let Ok(line) = rx.recv_timeout(Duration::from_millis(500)) else {
            continue;
        };
        let event: Json = serde_json::from_str(&line).expect("json");
        if event["event"] == "reconcile" {
            assert!(event["to"]
                .as_str()
                .expect("to")
                .ends_with("tasks/done/t-AAAA.md"));
            healed = true;
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    drop(rx);
    let _ = reader.join();

    assert!(healed, "the watcher never reported the misfiled task");
    assert_eq!(fixture.files(), vec!["tasks/done/t-AAAA.md".to_string()]);
}
