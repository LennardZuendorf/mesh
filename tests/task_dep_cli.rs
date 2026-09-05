//! `mesh task block | unblock | next` and the derived readiness surface.

mod common;

use common::VaultFixture;
use predicates::prelude::*;
use serde_json::Value as Json;

fn run(f: &VaultFixture, args: &[&str]) -> (String, String, i32) {
    let out = f.cmd().args(args).output().expect("run mesh");
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

fn new_task(f: &VaultFixture, args: &[&str]) -> String {
    let mut argv = vec!["--quiet", "task", "new"];
    argv.extend_from_slice(args);
    ok(f, &argv)
}

fn json(text: &str) -> Json {
    serde_json::from_str(text.trim()).expect("valid json")
}

fn task_path(f: &VaultFixture, id: &str) -> String {
    f.files()
        .into_iter()
        .find(|p| p.ends_with(&format!("{id}.md")))
        .unwrap_or_else(|| panic!("no file for {id}"))
}

fn bytes(f: &VaultFixture, id: &str) -> String {
    f.read(&task_path(f, id))
}

// ---------------------------------------------------------------------------------------
// task block
// ---------------------------------------------------------------------------------------

#[test]
fn block_adds_an_edge_and_mirrors_it_onto_the_blocker() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    let out = ok(&f, &["task", "block", &b, "--on", &a]);
    assert_eq!(out, format!("blocked {b} by {a}"));
    assert!(bytes(&f, &b).contains(&format!("blocked_by:\n  - {a}")));
    // The mirror is best effort but present when the blocker exists.
    assert!(bytes(&f, &a).contains(&format!("blocks:\n  - {b}")));
    assert!(ok(&f, &["task", "get", &b]).contains("ready: false"));
}

#[test]
fn block_accepts_a_csv_of_blockers() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    let c = new_task(&f, &["C"]);
    let out = ok(
        &f,
        &["task", "block", &c, "--on", &format!(" {a} , {b} ,, {a} ")],
    );
    assert_eq!(out, format!("blocked {c} by {a}, {b}"));
    let text = bytes(&f, &c);
    assert!(text.contains(&format!("  - {a}")));
    assert!(text.contains(&format!("  - {b}")));
}

#[test]
fn block_is_additive_and_never_replaces() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    let c = new_task(&f, &["C"]);
    ok(&f, &["task", "block", &c, "--on", &a]);
    ok(&f, &["task", "block", &c, "--on", &b]);
    let text = bytes(&f, &c);
    assert!(text.contains(&format!("  - {a}")), "{text}");
    assert!(text.contains(&format!("  - {b}")), "{text}");
}

#[test]
fn adding_an_existing_edge_writes_nothing_and_exits_zero() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    ok(&f, &["task", "block", &b, "--on", &a]);
    let before_b = bytes(&f, &b);
    let before_a = bytes(&f, &a);
    ok(&f, &["task", "block", &b, "--on", &a]);
    assert_eq!(bytes(&f, &b), before_b);
    assert_eq!(bytes(&f, &a), before_a);
}

#[test]
fn block_json_is_id_blocked_by_ready_updated() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    let value = json(&ok(&f, &["--json", "task", "block", &b, "--on", &a]));
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "blocked_by", "ready", "updated"]);
    assert_eq!(value["id"], Json::String(b));
    assert_eq!(value["blocked_by"], serde_json::json!([a]));
    assert_eq!(value["ready"], Json::Bool(false));
}

#[test]
fn block_quiet_prints_the_bare_id() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    assert_eq!(ok(&f, &["--quiet", "task", "block", &b, "--on", &a]), b);
}

#[test]
fn a_self_edge_is_exit_two_and_writes_nothing() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let before = bytes(&f, &a);
    let (_, stderr, code) = run(&f, &["task", "block", &a, "--on", &a]);
    assert_eq!(code, 2);
    assert_eq!(
        stderr.trim_end(),
        format!("a task cannot block itself: {a}")
    );
    assert_eq!(bytes(&f, &a), before);
}

#[test]
fn a_cycle_is_refused_before_any_write() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    let c = new_task(&f, &["C"]);
    ok(&f, &["task", "block", &b, "--on", &a]);
    ok(&f, &["task", "block", &c, "--on", &b]);
    let before = bytes(&f, &a);
    let (_, stderr, code) = run(&f, &["task", "block", &a, "--on", &c]);
    assert_eq!(code, 2);
    assert_eq!(
        stderr.trim_end(),
        format!("dependency cycle: {a} -> {c} -> {b} -> {a}")
    );
    assert_eq!(bytes(&f, &a), before);
}

#[test]
fn block_warns_when_the_mirror_target_is_missing() {
    let f = VaultFixture::new();
    let b = new_task(&f, &["B"]);
    let (stdout, stderr, code) = run(&f, &["task", "block", &b, "--on", "t-GONE"]);
    assert_eq!(code, 0);
    assert_eq!(stdout.trim_end(), format!("blocked {b} by t-GONE"));
    assert_eq!(
        stderr.trim_end(),
        "task block: could not mirror onto t-GONE (missing)"
    );
    // A dangling blocker fails open, so the task is still ready.
    assert!(ok(&f, &["task", "get", &b]).contains("ready: true"));
    // The warning is a notice: suppressed by --quiet, never in the payload.
    let (stdout, stderr, _) = run(&f, &["--json", "task", "block", &b, "--on", "t-ALSO-GONE"]);
    assert!(!stdout.contains("mirror"));
    assert!(stderr.contains("mirror"));
    let (_, stderr, _) = run(&f, &["--quiet", "task", "block", &b, "--on", "t-THIRD"]);
    assert_eq!(stderr, "");
}

#[test]
fn block_rejects_a_malformed_blocker_id_and_a_missing_task() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let (_, stderr, code) = run(&f, &["task", "block", &a, "--on", "nope"]);
    assert_eq!(code, 2);
    assert_eq!(stderr.trim_end(), "invalid task id: 'nope'");
    let (_, stderr, code) = run(&f, &["task", "block", "t-NOPE", "--on", &a]);
    assert_eq!(code, 3);
    assert_eq!(stderr.trim_end(), "task not found: t-NOPE");
}

// ---------------------------------------------------------------------------------------
// task unblock
// ---------------------------------------------------------------------------------------

#[test]
fn unblock_removes_an_edge_and_its_mirror() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    ok(&f, &["task", "block", &b, "--on", &a]);
    let out = ok(&f, &["task", "unblock", &b, "--on", &a]);
    assert_eq!(out, format!("unblocked {b} from {a}"));
    assert!(bytes(&f, &b).contains("blocked_by: []"));
    assert!(bytes(&f, &a).contains("blocks: []"));
    assert!(ok(&f, &["task", "get", &b]).contains("ready: true"));
}

#[test]
fn removing_an_absent_edge_writes_nothing_and_exits_zero() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    ok(&f, &["task", "block", &b, "--on", &a]);
    let before = bytes(&f, &b);
    ok(&f, &["task", "unblock", &b, "--on", "t-OTHER"]);
    assert_eq!(bytes(&f, &b), before);
}

#[test]
fn unblock_all_clears_every_edge_it_can_reach() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    let c = new_task(&f, &["C"]);
    ok(&f, &["task", "block", &c, "--on", &format!("{a},{b}")]);
    let out = ok(&f, &["task", "unblock", &c, "--all"]);
    // `--all` reports every blocker it reached, in ascending id order.
    let mut want = [a.as_str(), b.as_str()];
    want.sort();
    assert_eq!(out, format!("unblocked {c} from {}", want.join(", ")));
    assert!(bytes(&f, &c).contains("blocked_by: []"));
    assert!(bytes(&f, &a).contains("blocks: []"));
    assert!(bytes(&f, &b).contains("blocks: []"));
}

#[test]
fn unblock_all_reaches_a_one_sided_mirror_edge() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    // Hand-write only the forward direction, as an Obsidian edit would.
    let text = bytes(&f, &a).replace("blocks: []", &format!("blocks:\n  - {b}"));
    f.write(&task_path(&f, &a), &text);
    assert!(ok(&f, &["task", "get", &b]).contains("ready: false"));
    ok(&f, &["task", "unblock", &b, "--all"]);
    assert!(bytes(&f, &a).contains("blocks: []"));
    assert!(ok(&f, &["task", "get", &b]).contains("ready: true"));
}

#[test]
fn unblock_breaks_a_hand_made_cycle_without_being_refused() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    for (one, two) in [(&a, &b), (&b, &a)] {
        let text = bytes(&f, one).replace("blocked_by: []", &format!("blocked_by:\n  - {two}"));
        f.write(&task_path(&f, one), &text);
    }
    assert!(ok(&f, &["task", "get", &a]).contains("ready: false"));
    ok(&f, &["task", "unblock", &a, "--on", &b]);
    assert!(ok(&f, &["task", "get", &a]).contains("ready: true"));
}

#[test]
fn unblock_needs_on_or_all() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let (_, stderr, code) = run(&f, &["task", "unblock", &a]);
    assert_eq!(code, 2);
    assert_eq!(stderr.trim_end(), "pass --on with task ids, or --all");
}

#[test]
fn unblock_json_matches_the_block_shape() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    ok(&f, &["task", "block", &b, "--on", &a]);
    let value = json(&ok(&f, &["--json", "task", "unblock", &b, "--on", &a]));
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "blocked_by", "ready", "updated"]);
    assert_eq!(value["blocked_by"], serde_json::json!([]));
    assert_eq!(value["ready"], Json::Bool(true));
}

// ---------------------------------------------------------------------------------------
// derived readiness
// ---------------------------------------------------------------------------------------

#[test]
fn a_hand_written_one_sided_edge_still_answers_correctly() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    // Only `blocks` on A, as an Obsidian edit would leave it.
    let text = bytes(&f, &a).replace("blocks: []", &format!("blocks:\n  - {b}"));
    f.write(&task_path(&f, &a), &text);
    assert!(ok(&f, &["task", "get", &b]).contains("ready: false"));
    assert_eq!(ok(&f, &["--quiet", "task", "list", "--blocked"]), b);
    ok(&f, &["task", "finish", &a]);
    assert!(ok(&f, &["task", "get", &b]).contains("ready: true"));
}

#[test]
fn a_cancelled_blocker_unblocks_for_free() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B", "--blocked-by", &a]);
    let (_, stderr, _) = run(&f, &["task", "cancel", &a]);
    assert_eq!(stderr.trim_end(), format!("unblocked: {b}"));
    assert!(ok(&f, &["task", "get", &b]).contains("ready: true"));
}

#[test]
fn ready_implies_available_and_defaults_the_sort_to_priority() {
    let f = VaultFixture::new();
    let low = new_task(&f, &["low", "--priority", "low"]);
    let high = new_task(&f, &["high", "--priority", "high"]);
    let blocked = new_task(&f, &["blocked", "--priority", "high", "--blocked-by", &low]);
    let claimed = new_task(&f, &["claimed", "--priority", "high"]);
    ok(&f, &["task", "claim", &claimed]);
    let out = ok(&f, &["--quiet", "task", "list", "--ready"]);
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [high.as_str(), low.as_str()]
    );
    assert!(!out.contains(&blocked));
    assert!(!out.contains(&claimed));
    // --available keeps its dependency-blind Python meaning.
    assert!(ok(&f, &["--quiet", "task", "list", "--available"]).contains(&blocked));
}

#[test]
fn blocked_covers_open_and_claimed_but_not_terminal() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let open = new_task(&f, &["open", "--blocked-by", &a]);
    let claimed = new_task(&f, &["claimed", "--blocked-by", &a]);
    ok(&f, &["task", "claim", &claimed]);
    let done = new_task(&f, &["done", "--blocked-by", &a]);
    ok(&f, &["task", "finish", &done]);
    let out = ok(
        &f,
        &["--quiet", "task", "list", "--blocked", "--sort", "title"],
    );
    let mut ids: Vec<&str> = out.lines().collect();
    ids.sort();
    let mut want = vec![claimed.as_str(), open.as_str()];
    want.sort();
    assert_eq!(ids, want);
}

#[test]
fn strict_claim_is_exit_five_and_no_strict_overrides_the_config_default() {
    let f = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n\
         [tasks]\ncollections = []\nstrict = true\n",
    );
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B", "--blocked-by", &a]);
    let before = bytes(&f, &b);
    // `[tasks].strict = true` flips the default.
    let (_, stderr, code) = run(&f, &["task", "claim", &b]);
    assert_eq!(code, 5);
    assert_eq!(stderr.trim_end(), format!("task {b} is blocked by {a}"));
    assert_eq!(bytes(&f, &b), before);
    // `--no-strict` overrides it per call.
    let (stdout, stderr, code) = run(&f, &["task", "claim", &b, "--no-strict"]);
    assert_eq!(code, 0);
    assert_eq!(stdout.trim_end(), format!("claimed {b}"));
    assert_eq!(stderr.trim_end(), format!("task {b} is blocked by {a}"));
}

#[test]
fn a_blocked_json_envelope_names_the_blockers() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B", "--blocked-by", &a]);
    let (_, stderr, code) = run(&f, &["--json", "task", "claim", &b, "--strict"]);
    assert_eq!(code, 5);
    let value = json(&stderr);
    assert_eq!(value["kind"], Json::String("blocked".into()));
    assert_eq!(
        value["message"],
        Json::String(format!("task {b} is blocked by {a}"))
    );
    assert_eq!(
        value["next_action"],
        Json::String("finish or cancel the blocking tasks, then retry".into())
    );
}

#[test]
fn a_non_strict_claim_payload_gains_blocked_by_unsatisfied() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B", "--blocked-by", &a]);
    let value = json(&ok(&f, &["--json", "task", "claim", &b]));
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "status", "blocked_by_unsatisfied", "updated"]);
    assert_eq!(value["blocked_by_unsatisfied"], serde_json::json!([a]));
}

#[test]
fn a_dangling_blocker_fails_open() {
    let f = VaultFixture::new();
    let b = new_task(&f, &["B", "--blocked-by", "t-TYPO"]);
    assert!(ok(&f, &["task", "get", &b]).contains("ready: true"));
    assert_eq!(ok(&f, &["--quiet", "task", "list", "--ready"]), b);
    assert_eq!(ok(&f, &["--quiet", "task", "list", "--blocked"]), "");
}

// ---------------------------------------------------------------------------------------
// task next
// ---------------------------------------------------------------------------------------

#[test]
fn next_selects_by_priority_then_fifo() {
    let f = VaultFixture::new();
    let low = new_task(&f, &["low", "--priority", "low"]);
    let first = new_task(&f, &["first", "--priority", "high"]);
    let second = new_task(&f, &["second", "--priority", "high"]);
    let out = ok(&f, &["task", "next"]);
    assert_eq!(out.split('\t').next(), Some(first.as_str()));
    ok(&f, &["task", "claim", &first]);
    assert_eq!(ok(&f, &["--quiet", "task", "next"]), second);
    ok(&f, &["task", "claim", &second]);
    assert_eq!(ok(&f, &["--quiet", "task", "next"]), low);
}

#[test]
fn next_human_output_is_the_task_list_row() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["Pick me"]);
    let out = ok(&f, &["task", "next"]);
    assert_eq!(
        out.split('\t').collect::<Vec<_>>(),
        [id.as_str(), "open", "-", "Pick me"]
    );
}

#[test]
fn next_json_is_the_task_node_plus_path() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    let value = json(&ok(&f, &["--json", "task", "next"]));
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.first().copied(), Some("id"));
    assert_eq!(keys.last().copied(), Some("path"));
    assert_eq!(value["id"], Json::String(id.clone()));
    assert!(value["path"]
        .as_str()
        .unwrap_or_default()
        .ends_with(&format!("{id}.md")));
    assert!(value.get("body").is_none());
}

#[test]
fn next_with_nothing_ready_is_exit_three() {
    let f = VaultFixture::new();
    let (stdout, stderr, code) = run(&f, &["task", "next"]);
    assert_eq!(code, 3);
    assert_eq!(stdout, "");
    assert_eq!(stderr.trim_end(), "no ready task");

    let (_, stderr, code) = run(&f, &["--json", "task", "next"]);
    assert_eq!(code, 3);
    let value = json(&stderr);
    assert_eq!(value["kind"], Json::String("not_found".into()));
    assert_eq!(value["message"], Json::String("no ready task".into()));
    assert_eq!(
        value["next_action"],
        Json::String("check the id and retry, or list to find the right one".into())
    );
    // --quiet never suppresses an error.
    let (_, stderr, code) = run(&f, &["--quiet", "task", "next"]);
    assert_eq!(code, 3);
    assert_eq!(stderr.trim_end(), "no ready task");
}

#[test]
fn next_skips_blocked_and_claimed_candidates() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B", "--blocked-by", &a]);
    ok(&f, &["task", "claim", &a]);
    let (_, _, code) = run(&f, &["task", "next"]);
    assert_eq!(code, 3);
    ok(&f, &["task", "finish", &a]);
    assert_eq!(ok(&f, &["--quiet", "task", "next"]), b);
}

#[test]
fn next_claim_takes_the_task_in_the_same_invocation() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    assert_eq!(ok(&f, &["--quiet", "task", "next", "--claim"]), id);
    let text = bytes(&f, &id);
    assert!(text.contains("status: claimed"));
    assert!(text.contains("claimed_by: test-agent"));
    // The queue is now empty.
    assert_eq!(run(&f, &["task", "next", "--claim"]).2, 3);
}

#[test]
fn next_claim_re_selects_past_a_task_another_agent_holds() {
    let f = VaultFixture::new();
    let taken = new_task(&f, &["taken", "--priority", "high"]);
    let free = new_task(&f, &["free", "--priority", "normal"]);
    // A hand-edited stale claim leaves `taken` visible to --available but not to --ready.
    let text = bytes(&f, &taken).replace("claimed_by: null", "claimed_by: someone-else");
    f.write(&task_path(&f, &taken), &text);
    assert_eq!(ok(&f, &["--quiet", "task", "next", "--claim"]), free);
}

#[test]
fn next_claim_respects_the_owner_flag_as_the_claimer() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    ok(&f, &["--owner", "alice", "task", "next", "--claim"]);
    assert!(bytes(&f, &id).contains("claimed_by: alice"));
}

#[test]
fn next_filters_by_mine_project_and_tags() {
    let f = VaultFixture::new();
    let mine = new_task(&f, &["mine", "--tags", "x", "--project", "n-P"]);
    let theirs = new_task(&f, &["theirs", "--owner", "bob"]);
    assert_eq!(ok(&f, &["--quiet", "task", "next", "--mine"]), mine);
    assert_eq!(
        ok(&f, &["--quiet", "task", "next", "--project", "n-P"]),
        mine
    );
    assert_eq!(ok(&f, &["--quiet", "task", "next", "--tags", "x"]), mine);
    assert_eq!(run(&f, &["task", "next", "--tags", "nope"]).2, 3);
    let _ = theirs;
}

#[test]
fn next_strict_with_claim_skips_a_blocked_candidate_rather_than_offering_it() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    // Make B blocked only through the one-sided mirror, so --ready still admits it under
    // a stale scan but the claim's own readiness check catches it.
    let text = bytes(&f, &b).replace("blocked_by: []", &format!("blocked_by:\n  - {a}"));
    f.write(&task_path(&f, &b), &text);
    // A is ready; claiming it strictly is fine.
    assert_eq!(
        ok(&f, &["--quiet", "task", "next", "--claim", "--strict"]),
        a
    );
    // Now nothing is ready: B is blocked by the unfinished A.
    let (_, stderr, code) = run(&f, &["task", "next", "--claim", "--strict"]);
    assert_eq!(code, 3);
    assert_eq!(stderr.trim_end(), "no ready task");
}

#[test]
fn next_flag_placement_parity() {
    let f = VaultFixture::new();
    new_task(&f, &["T"]);
    assert_eq!(
        run(&f, &["--json", "task", "next"]).0,
        run(&f, &["task", "next", "--json"]).0
    );
    assert_eq!(
        run(&f, &["--quiet", "task", "next"]).0,
        run(&f, &["task", "next", "--quiet"]).0
    );
}

#[test]
fn the_dependency_verbs_document_themselves_in_help() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["task", "block", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Comma-separated blocker task ids"));
    f.cmd()
        .args(["task", "unblock", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Drop every blocker"));
    f.cmd()
        .args(["task", "next", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Claim the selected task"));
}

// ---------------------------------------------------------------------------------------
// the Python corpus
// ---------------------------------------------------------------------------------------

#[test]
fn the_corpus_blocked_edge_is_honoured_end_to_end() {
    let f = VaultFixture::from_corpus();
    assert!(ok(&f, &["task", "get", "t-1FN1"]).contains("ready: false"));
    // t-TCY1 is the only ready, unblocked, unclaimed corpus task with a real priority.
    assert_eq!(ok(&f, &["--quiet", "task", "next"]), "t-TCY1");
    ok(&f, &["--owner", "demo-agent", "task", "finish", "t-TCY1"]);
    assert!(ok(&f, &["task", "get", "t-1FN1"]).contains("ready: true"));
    assert_eq!(ok(&f, &["--quiet", "task", "next"]), "t-1FN1");
}

#[test]
fn blocking_a_corpus_task_keeps_its_python_key_order() {
    let f = VaultFixture::from_corpus();
    ok(&f, &["task", "block", "t-LEGP", "--on", "t-TCY1"]);
    let text = f.read("tasks/open/t-LEGP.md");
    assert!(
        text.starts_with("---\nblocked_by:\n  - t-TCY1\nblocks: []\n"),
        "{text}"
    );
    assert!(text.contains("priority: urgent"));
    assert!(f
        .read("tasks/open/t-TCY1.md")
        .contains("blocks:\n  - t-LEGP"));
}
