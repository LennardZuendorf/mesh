//! Real multi-process races against the `mesh` binary: claims, appends and the lock protocol.
//!
//! Nothing here is simulated in-process — every contender is a separate OS process, so the
//! `O_EXCL` test-and-set, the bounded wait and the stale-lock reclaim are exercised for real.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::VaultFixture;

/// The `mesh` binary Cargo built for this integration test.
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mesh"))
}

/// A raw `std::process::Command` with the same clean environment `VaultFixture::cmd` uses.
fn cmd(f: &VaultFixture, args: &[&str]) -> Command {
    let mut command = Command::new(bin());
    command
        .env_remove("MESH_CONFIG_PATH")
        .env_remove("MESH_AGENT")
        .env_remove("MESH_VAULT")
        .env_remove("MESH_INDEXED_BIN")
        .arg("--config")
        .arg(&f.config)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run(f: &VaultFixture, args: &[&str]) -> (String, String, i32) {
    let out = cmd(f, args).output().expect("run mesh");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn ok(f: &VaultFixture, args: &[&str]) -> String {
    let (stdout, stderr, code) = run(f, args);
    assert_eq!(code, 0, "args={args:?} stderr={stderr}");
    stdout.trim_end().to_string()
}

fn new_task(f: &VaultFixture, title: &str) -> String {
    ok(f, &["--quiet", "task", "new", title])
}

fn task_path(f: &VaultFixture, id: &str) -> String {
    f.files()
        .into_iter()
        .find(|p| p.ends_with(&format!("{id}.md")))
        .unwrap_or_else(|| panic!("no file for {id}"))
}

/// Spawn every argv at once, then collect the exit codes in spawn order.
fn race(f: &VaultFixture, argvs: &[Vec<String>]) -> Vec<i32> {
    let children: Vec<_> = argvs
        .iter()
        .map(|argv| {
            let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
            cmd(f, &borrowed).spawn().expect("spawn mesh")
        })
        .collect();
    children
        .into_iter()
        .map(|child| {
            child
                .wait_with_output()
                .expect("wait")
                .status
                .code()
                .unwrap_or(-1)
        })
        .collect()
}

/// The lock path for one task id inside the fixture's vault.
fn lock_path(f: &VaultFixture, id: &str) -> PathBuf {
    f.vault.join("tasks/.locks").join(format!("{id}.lock"))
}

fn write_lock(path: &Path, pid: u32) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create .locks");
    }
    std::fs::write(path, format!("{pid}\n")).expect("write lock");
}

// ---------------------------------------------------------------------------------------
// claim races
// ---------------------------------------------------------------------------------------

#[test]
fn eight_concurrent_claims_yield_exactly_one_winner() {
    let f = VaultFixture::new();
    let id = new_task(&f, "Contended");
    let argvs: Vec<Vec<String>> = (0..8)
        .map(|n| {
            vec![
                "--owner".to_string(),
                format!("agent-{n}"),
                "task".to_string(),
                "claim".to_string(),
                id.clone(),
            ]
        })
        .collect();
    let codes = race(&f, &argvs);
    assert_eq!(codes.len(), 8);
    assert_eq!(
        codes.iter().filter(|c| **c == 0).count(),
        1,
        "codes = {codes:?}"
    );
    assert_eq!(
        codes.iter().filter(|c| **c == 4).count(),
        7,
        "codes = {codes:?}"
    );
    // Exactly one owner is recorded, and it is one of the contenders.
    let text = f.read(&task_path(&f, &id));
    assert_eq!(text.matches("claimed_by: agent-").count(), 1, "{text}");
    assert!(text.contains("status: claimed"));
}

#[test]
fn eight_concurrent_same_agent_claims_are_all_idempotent() {
    let f = VaultFixture::new();
    let id = new_task(&f, "Reclaimed");
    let argvs: Vec<Vec<String>> = (0..8)
        .map(|_| {
            vec![
                "--owner".to_string(),
                "solo".to_string(),
                "task".to_string(),
                "claim".to_string(),
                id.clone(),
            ]
        })
        .collect();
    let codes = race(&f, &argvs);
    assert!(codes.iter().all(|c| *c == 0), "codes = {codes:?}");
    let text = f.read(&task_path(&f, &id));
    assert_eq!(text.matches("claimed_by: solo").count(), 1);
}

#[test]
fn concurrent_claims_across_distinct_tasks_all_succeed() {
    let f = VaultFixture::new();
    let ids: Vec<String> = (0..6).map(|n| new_task(&f, &format!("T{n}"))).collect();
    let argvs: Vec<Vec<String>> = ids
        .iter()
        .map(|id| {
            vec![
                "--owner".to_string(),
                "alice".to_string(),
                "task".to_string(),
                "claim".to_string(),
                id.clone(),
            ]
        })
        .collect();
    let codes = race(&f, &argvs);
    assert!(codes.iter().all(|c| *c == 0), "codes = {codes:?}");
    for id in &ids {
        assert!(f.read(&task_path(&f, id)).contains("claimed_by: alice"));
    }
}

// ---------------------------------------------------------------------------------------
// append and terminal races
// ---------------------------------------------------------------------------------------

#[test]
fn eight_concurrent_appends_all_land() {
    let f = VaultFixture::new();
    let id = new_task(&f, "Journal");
    let argvs: Vec<Vec<String>> = (0..8)
        .map(|n| {
            vec![
                "task".to_string(),
                "append".to_string(),
                id.clone(),
                format!("line-{n}"),
            ]
        })
        .collect();
    let codes = race(&f, &argvs);
    assert!(codes.iter().all(|c| *c == 0), "codes = {codes:?}");
    let text = f.read(&task_path(&f, &id));
    for n in 0..8 {
        assert_eq!(
            text.matches(&format!("line-{n}")).count(),
            1,
            "line-{n} missing or duplicated in:\n{text}"
        );
    }
}

#[test]
fn concurrent_finishes_write_exactly_one_outcome_section() {
    let f = VaultFixture::new();
    let id = new_task(&f, "Finished once");
    let argvs: Vec<Vec<String>> = (0..6)
        .map(|n| {
            vec![
                "task".to_string(),
                "finish".to_string(),
                id.clone(),
                "--outcome".to_string(),
                format!("outcome-{n}"),
            ]
        })
        .collect();
    let codes = race(&f, &argvs);
    assert!(codes.iter().all(|c| *c == 0), "codes = {codes:?}");
    let path = task_path(&f, &id);
    assert_eq!(path, format!("tasks/done/{id}.md"));
    let text = f.read(&path);
    assert_eq!(text.matches("## Outcome").count(), 1, "{text}");
    assert!(text.contains("status: done"));
}

#[test]
fn a_concurrent_append_and_finish_both_survive() {
    let f = VaultFixture::new();
    let id = new_task(&f, "Both");
    let argvs = vec![
        vec![
            "task".to_string(),
            "append".to_string(),
            id.clone(),
            "note-from-append".to_string(),
        ],
        vec!["task".to_string(), "finish".to_string(), id.clone()],
    ];
    let codes = race(&f, &argvs);
    assert!(codes.iter().all(|c| *c == 0), "codes = {codes:?}");
    let text = f.read(&task_path(&f, &id));
    assert!(text.contains("note-from-append"), "{text}");
    assert!(text.contains("## Outcome"), "{text}");
    assert!(text.contains("status: done"));
}

#[test]
fn concurrent_creates_never_collide_on_an_id() {
    let f = VaultFixture::new();
    let argvs: Vec<Vec<String>> = (0..8)
        .map(|_| {
            vec![
                "--quiet".to_string(),
                "task".to_string(),
                "new".to_string(),
                // Identical titles, so the id digest collides unless the allocator lock works.
                "Same Title".to_string(),
            ]
        })
        .collect();
    let codes = race(&f, &argvs);
    assert!(codes.iter().all(|c| *c == 0), "codes = {codes:?}");
    let files: Vec<String> = f
        .files()
        .into_iter()
        .filter(|p| p.starts_with("tasks/open/"))
        .collect();
    assert_eq!(files.len(), 8, "{files:?}");
    assert_eq!(
        ok(&f, &["--quiet", "task", "list", "--limit=-1"])
            .lines()
            .count(),
        8
    );
}

#[test]
fn concurrent_block_calls_never_deadlock() {
    let f = VaultFixture::new();
    let blocker = new_task(&f, "Blocker");
    let ids: Vec<String> = (0..6).map(|n| new_task(&f, &format!("Dep{n}"))).collect();
    // Every one of these takes its own lock and then the blocker's mirror lock, one at a
    // time in ascending id order — so they serialise rather than deadlocking.
    let argvs: Vec<Vec<String>> = ids
        .iter()
        .map(|id| {
            vec![
                "task".to_string(),
                "block".to_string(),
                id.clone(),
                "--on".to_string(),
                blocker.clone(),
            ]
        })
        .collect();
    let started = Instant::now();
    let codes = race(&f, &argvs);
    assert!(codes.iter().all(|c| *c == 0), "codes = {codes:?}");
    assert!(started.elapsed() < Duration::from_secs(30), "took too long");
    for id in &ids {
        assert!(f.read(&task_path(&f, id)).contains(&blocker));
    }
}

// ---------------------------------------------------------------------------------------
// the lock protocol
// ---------------------------------------------------------------------------------------

#[test]
fn a_live_lock_exits_four_within_the_wait_budget_rather_than_hanging() {
    let f = VaultFixture::new();
    let id = new_task(&f, "Locked");
    // A fresh lock file naming this very process: alive, and inside the TTL.
    let lock = lock_path(&f, &id);
    write_lock(&lock, std::process::id());
    let before = f.read(&task_path(&f, &id));

    let started = Instant::now();
    let (_, stderr, code) = run(&f, &["task", "claim", &id]);
    let elapsed = started.elapsed();

    assert_eq!(code, 4, "stderr={stderr}");
    assert_eq!(
        stderr.trim_end(),
        format!("lock is held: {}", lock.display())
    );
    // The bounded wait is 15 s; anything near it is fine, a hang is not.
    assert!(elapsed < Duration::from_secs(60), "waited {elapsed:?}");
    assert!(
        elapsed >= Duration::from_secs(10),
        "did not wait: {elapsed:?}"
    );
    assert_eq!(f.read(&task_path(&f, &id)), before, "the file was written");
    assert!(lock.exists(), "the live lock was stolen");
}

#[test]
fn a_live_lock_json_envelope_advises_a_retry_delay() {
    let f = VaultFixture::new();
    let id = new_task(&f, "Locked");
    write_lock(&lock_path(&f, &id), std::process::id());
    let (_, stderr, code) = run(&f, &["--json", "task", "append", &id, "x"]);
    assert_eq!(code, 4);
    let value: serde_json::Value = serde_json::from_str(stderr.trim()).expect("json envelope");
    assert_eq!(value["kind"], serde_json::json!("lock_conflict"));
    assert_eq!(value["retry_after_ms"], serde_json::json!(250));
    assert!(value["message"]
        .as_str()
        .unwrap_or_default()
        .starts_with("lock is held: "));
}

#[test]
fn a_dead_pid_lock_is_reclaimed_immediately() {
    let f = VaultFixture::new();
    let id = new_task(&f, "Stale");
    let lock = lock_path(&f, &id);
    // 999999 is above the usual pid_max and is not running.
    write_lock(&lock, 999_999);

    let started = Instant::now();
    let out = ok(&f, &["task", "claim", &id]);
    assert_eq!(out, format!("claimed {id}"));
    // A reclaim happens on the first attempt, so this must not spend the wait budget.
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(f
        .read(&task_path(&f, &id))
        .contains("claimed_by: test-agent"));
    assert!(!lock.exists(), "the reclaimed lock was not released");
}

#[test]
fn an_aged_out_lock_is_reclaimed_regardless_of_its_contents() {
    let f = VaultFixture::new();
    let id = new_task(&f, "Aged");
    let lock = lock_path(&f, &id);
    // A live pid, but a modification time well past the 300 s TTL.
    write_lock(&lock, std::process::id());
    let aged = Command::new("touch")
        .arg("-d")
        .arg("2 hours ago")
        .arg(&lock)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !aged {
        // No GNU `touch -d` here; the dead-pid path already covers reclaim.
        return;
    }
    let started = Instant::now();
    ok(&f, &["task", "claim", &id]);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(f
        .read(&task_path(&f, &id))
        .contains("claimed_by: test-agent"));
}

#[test]
fn an_unparseable_lock_is_never_stolen() {
    let f = VaultFixture::new();
    let id = new_task(&f, "Garbage");
    let lock = lock_path(&f, &id);
    if let Some(parent) = lock.parent() {
        std::fs::create_dir_all(parent).expect("create .locks");
    }
    std::fs::write(&lock, "not-a-pid\n").expect("write lock");
    let (_, stderr, code) = run(&f, &["task", "claim", &id]);
    assert_eq!(code, 4, "stderr={stderr}");
    assert!(lock.exists());
}

#[test]
fn a_live_lock_blocks_a_create_through_the_allocator_lock() {
    let f = VaultFixture::new();
    // Force the tasks root to exist, then take the per-space allocator lock.
    new_task(&f, "Seed");
    let lock = f.vault.join("tasks/.locks/_create.lock");
    write_lock(&lock, std::process::id());
    let (_, stderr, code) = run(&f, &["task", "new", "Blocked create"]);
    assert_eq!(code, 4, "stderr={stderr}");
    assert!(stderr.contains("_create.lock"), "{stderr}");
    assert_eq!(ok(&f, &["--quiet", "task", "list"]).lines().count(), 1);
}

#[test]
fn a_live_lock_on_one_task_never_blocks_another() {
    let f = VaultFixture::new();
    let locked = new_task(&f, "Locked");
    let free = new_task(&f, "Free");
    write_lock(&lock_path(&f, &locked), std::process::id());
    let started = Instant::now();
    ok(&f, &["task", "claim", &free]);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(f
        .read(&task_path(&f, &free))
        .contains("claimed_by: test-agent"));
}

#[test]
fn a_lock_is_released_after_a_normal_write() {
    let f = VaultFixture::new();
    let id = new_task(&f, "Clean");
    ok(&f, &["task", "claim", &id]);
    assert!(!lock_path(&f, &id).exists());
    ok(&f, &["task", "finish", &id]);
    assert!(!lock_path(&f, &id).exists());
    // The id-named lock survives the open/ -> done/ move, so a later verb still serialises.
    ok(&f, &["task", "append", &id, "x"]);
    assert!(!lock_path(&f, &id).exists());
}

#[test]
fn the_corpus_stale_lock_does_not_wedge_a_claim() {
    let f = VaultFixture::from_corpus();
    // tests/fixtures/python-vault ships tasks/.locks/t-STAL.lock holding a dead pid.
    assert!(f.vault.join("tasks/.locks/t-STAL.lock").exists());
    let started = Instant::now();
    ok(&f, &["--owner", "demo-agent", "task", "claim", "t-TCY1"]);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(f
        .read("tasks/open/t-TCY1.md")
        .contains("claimed_by: demo-agent"));
    // A lock for an unrelated id is left exactly where it was.
    assert!(f.vault.join("tasks/.locks/t-STAL.lock").exists());
}

// ---------------------------------------------------------------------------------------
// task next under contention
// ---------------------------------------------------------------------------------------

#[test]
fn concurrent_next_claim_hands_distinct_tasks_to_distinct_agents() {
    let f = VaultFixture::new();
    for n in 0..6 {
        new_task(&f, &format!("Work {n}"));
    }
    let argvs: Vec<Vec<String>> = (0..3)
        .map(|n| {
            vec![
                "--quiet".to_string(),
                "--owner".to_string(),
                format!("agent-{n}"),
                "task".to_string(),
                "next".to_string(),
                "--claim".to_string(),
            ]
        })
        .collect();
    let codes = race(&f, &argvs);
    // The re-selection loop means a conflict is retried, not returned.
    assert!(codes.iter().all(|c| *c == 0), "codes = {codes:?}");
    let claimed = ok(
        &f,
        &[
            "--quiet",
            "task",
            "list",
            "--status",
            "claimed",
            "--limit=-1",
        ],
    );
    assert_eq!(claimed.lines().count(), 3, "{claimed}");
    let mut owners: Vec<String> = claimed
        .lines()
        .map(|id| {
            let text = f.read(&task_path(&f, id));
            text.lines()
                .find_map(|l| l.strip_prefix("claimed_by: "))
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    owners.sort();
    owners.dedup();
    assert_eq!(owners.len(), 3, "{owners:?}");
}
