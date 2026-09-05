//! `mesh task` lifecycle verbs, driven through the real binary.

mod common;

use common::VaultFixture;
use predicates::prelude::*;
use serde_json::Value as Json;

// ---------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------

/// Run a command and return `(stdout, stderr, exit code)`.
fn run(f: &VaultFixture, args: &[&str]) -> (String, String, i32) {
    let out = f.cmd().args(args).output().expect("run mesh");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Run and assert exit 0, returning trimmed stdout.
fn ok(f: &VaultFixture, args: &[&str]) -> String {
    let (stdout, stderr, code) = run(f, args);
    assert_eq!(code, 0, "args={args:?} stderr={stderr}");
    stdout.trim_end().to_string()
}

/// The id `task new` reports under `--quiet`.
fn new_task(f: &VaultFixture, args: &[&str]) -> String {
    let mut argv = vec!["--quiet", "task", "new"];
    argv.extend_from_slice(args);
    ok(f, &argv)
}

fn json(text: &str) -> Json {
    serde_json::from_str(text.trim()).expect("valid json")
}

/// The vault-relative path of a task file, whichever folder it is in.
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
// task new
// ---------------------------------------------------------------------------------------

#[test]
fn new_creates_an_open_task_in_the_open_folder() {
    let f = VaultFixture::new();
    let out = ok(&f, &["task", "new", "Ship it", "--body", "Original body."]);
    assert!(out.starts_with("created t-"), "{out}");
    let id = out.trim_start_matches("created ").to_string();
    assert_eq!(task_path(&f, &id), format!("tasks/open/{id}.md"));
    let text = bytes(&f, &id);
    assert!(text.contains("status: open"));
    assert!(text.contains("type: task"));
    assert!(text.ends_with("Original body.\n"), "{text}");
}

#[test]
fn new_writes_the_fourteen_keys_in_declaration_order() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["Ship it"]);
    let text = bytes(&f, &id);
    let keys: Vec<&str> = text
        .lines()
        .skip(1)
        .take_while(|l| *l != "---")
        .filter_map(|l| l.split(':').next())
        .filter(|k| !k.starts_with("  ") && !k.starts_with('-'))
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "type",
            "title",
            "tags",
            "owner",
            "created",
            "updated",
            "related",
            "status",
            "priority",
            "claimed_by",
            "project",
            "blocks",
            "blocked_by"
        ]
    );
    // Written as null, never omitted.
    assert!(text.contains("priority: null"));
    assert!(text.contains("claimed_by: null"));
    assert!(text.contains("project: null"));
}

#[test]
fn new_json_is_id_status_updated_in_that_order() {
    let f = VaultFixture::new();
    let (stdout, _, code) = run(&f, &["--json", "task", "new", "T"]);
    assert_eq!(code, 0);
    let value = json(&stdout);
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "status", "updated"]);
    assert_eq!(value["status"], Json::String("open".into()));
    assert!(value["updated"].as_str().expect("updated").ends_with('Z'));
}

#[test]
fn new_quiet_prints_the_bare_id_and_beats_json() {
    let f = VaultFixture::new();
    let out = ok(&f, &["--json", "--quiet", "task", "new", "T"]);
    assert!(out.starts_with("t-"), "{out}");
    assert!(!out.contains('{'), "{out}");
}

#[test]
fn new_rejects_a_bad_priority_before_writing_anything() {
    let f = VaultFixture::new();
    let (_, stderr, code) = run(&f, &["task", "new", "T", "--priority", "urgent"]);
    assert_eq!(code, 2);
    assert_eq!(
        stderr.trim_end(),
        "invalid priority: 'urgent' (use high, normal, low)"
    );
    assert!(f.files().is_empty(), "{:?}", f.files());
}

#[test]
fn new_rejects_an_owner_outside_a_configured_roster() {
    let f = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"alice\"\n\n[tasks]\ncollections = [\"alice\"]\n",
    );
    let (_, stderr, code) = run(&f, &["task", "new", "T", "--owner", "ghost"]);
    assert_eq!(code, 2);
    assert_eq!(stderr.trim_end(), "unknown owner: 'ghost'");
    assert!(f.files().is_empty());
}

#[test]
fn new_warns_about_a_duplicate_title_but_still_creates() {
    let f = VaultFixture::new();
    let first = new_task(&f, &["Japan Visa"]);
    let (stdout, stderr, code) = run(&f, &["task", "new", "japan  visa"]);
    assert_eq!(code, 0);
    assert_eq!(
        stderr.trim_end(),
        format!("task new: duplicate title, also used by {first}")
    );
    assert!(stdout.starts_with("created t-"));
    // The advisory never reaches the payload and is suppressed by --quiet.
    let (stdout, stderr, _) = run(&f, &["--json", "task", "new", "Japan Visa"]);
    assert!(!stdout.contains("duplicate"));
    assert!(stderr.contains("duplicate"));
    let (_, stderr, _) = run(&f, &["--quiet", "task", "new", "Japan Visa"]);
    assert_eq!(stderr, "");
}

#[test]
fn new_parses_csv_flags_and_records_the_project_link() {
    let f = VaultFixture::new();
    let id = new_task(
        &f,
        &[
            "T",
            "--tags",
            " a , b ,, a ",
            "--project",
            "n-P",
            "--priority",
            "high",
        ],
    );
    let text = bytes(&f, &id);
    assert!(text.contains("tags:\n  - a\n  - b\n"), "{text}");
    assert!(text.contains("project: n-P"));
    assert!(text.contains("priority: high"));
}

// ---------------------------------------------------------------------------------------
// task update
// ---------------------------------------------------------------------------------------

#[test]
fn update_changes_only_the_supplied_fields() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T", "--tags", "a"]);
    ok(&f, &["task", "claim", &id]);
    let out = ok(&f, &["task", "update", &id, "--title", "Renamed"]);
    assert_eq!(out, format!("updated {id}"));
    let text = bytes(&f, &id);
    assert!(text.contains("title: Renamed"));
    assert!(text.contains("tags:\n  - a\n"));
    // Reassignment never touches the claim.
    assert!(text.contains("claimed_by: test-agent"));
    assert!(text.contains("status: claimed"));
}

#[test]
fn update_owner_reassigns_without_acting_as_that_owner() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    ok(&f, &["task", "claim", &id]);
    ok(&f, &["task", "update", &id, "--owner", "bob"]);
    let text = bytes(&f, &id);
    assert!(text.contains("owner: bob"));
    assert!(text.contains("claimed_by: test-agent"));
}

#[test]
fn update_tags_use_the_shared_spec_grammar() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T", "--tags", "a,b"]);
    ok(&f, &["task", "update", &id, "--tags", "+c,-a"]);
    assert!(bytes(&f, &id).contains("tags:\n  - b\n  - c\n"));
    let (_, stderr, code) = run(&f, &["task", "update", &id, "--tags", "+c,d"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("ambiguous tag spec"), "{stderr}");
}

#[test]
fn update_replaces_the_edge_lists() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    let c = new_task(&f, &["C", "--blocked-by", &a]);
    assert!(bytes(&f, &c).contains(&format!("blocked_by:\n  - {a}")));
    ok(&f, &["task", "update", &c, "--blocked-by", &b]);
    let text = bytes(&f, &c);
    assert!(text.contains(&format!("blocked_by:\n  - {b}")));
    assert!(!text.contains(&format!("  - {a}")));
    // The dropped mirror is retracted, so the union rule cannot resurrect the edge.
    assert!(bytes(&f, &a).contains("blocks: []"));
}

#[test]
fn update_on_a_missing_task_is_exit_three() {
    let f = VaultFixture::new();
    let (_, stderr, code) = run(&f, &["task", "update", "t-NOPE", "--title", "x"]);
    assert_eq!(code, 3);
    assert_eq!(stderr.trim_end(), "task not found: t-NOPE");
}

// ---------------------------------------------------------------------------------------
// task append
// ---------------------------------------------------------------------------------------

#[test]
fn append_adds_a_block_and_never_transitions() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T", "--body", "start"]);
    ok(&f, &["task", "finish", &id]);
    let out = ok(&f, &["task", "append", &id, "more"]);
    assert_eq!(out, format!("appended {id}"));
    let text = bytes(&f, &id);
    assert_eq!(text.matches("## Outcome").count(), 1);
    assert!(text.contains("status: done"));
    assert!(text.ends_with("more\n"), "{text}");
    assert_eq!(task_path(&f, &id), format!("tasks/done/{id}.md"));
}

#[test]
fn append_under_a_section_and_with_a_timestamp() {
    let f = VaultFixture::new();
    let id = new_task(
        &f,
        &["T", "--body", "Intro.\n\n## A\n\nitem1\n\n## B\n\nitem2"],
    );
    ok(&f, &["task", "append", &id, "NEW", "--section", "A"]);
    let text = bytes(&f, &id);
    assert!(
        text.contains("## A\n\nitem1\n\nNEW\n\n## B\n\nitem2"),
        "{text}"
    );
    ok(
        &f,
        &[
            "--owner",
            "carol",
            "task",
            "append",
            &id,
            "stamped",
            "--timestamp",
        ],
    );
    assert!(bytes(&f, &id).contains(" — carol\nstamped"));
}

#[test]
fn append_json_reports_the_status() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    let value = json(&ok(&f, &["--json", "task", "append", &id, "x"]));
    assert_eq!(value["id"], Json::String(id));
    assert_eq!(value["status"], Json::String("open".into()));
}

// ---------------------------------------------------------------------------------------
// task claim / release
// ---------------------------------------------------------------------------------------

#[test]
fn claim_is_an_atomic_test_and_set() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    assert_eq!(ok(&f, &["task", "claim", &id]), format!("claimed {id}"));
    let text = bytes(&f, &id);
    assert!(text.contains("status: claimed"));
    assert!(text.contains("claimed_by: test-agent"));
    // A claim never moves the file.
    assert_eq!(task_path(&f, &id), format!("tasks/open/{id}.md"));
}

#[test]
fn a_same_agent_reclaim_leaves_the_bytes_untouched() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    ok(&f, &["task", "claim", &id]);
    let before = bytes(&f, &id);
    ok(&f, &["task", "claim", &id]);
    assert_eq!(bytes(&f, &id), before);
}

#[test]
fn a_foreign_claim_is_exit_four_and_writes_nothing() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    ok(&f, &["--owner", "alice", "task", "claim", &id]);
    let before = bytes(&f, &id);
    let (_, stderr, code) = run(&f, &["--owner", "bob", "task", "claim", &id]);
    assert_eq!(code, 4);
    assert_eq!(
        stderr.trim_end(),
        format!("task {id} already claimed by alice")
    );
    assert_eq!(bytes(&f, &id), before);
}

#[test]
fn a_claim_conflict_json_envelope_carries_the_holder() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    ok(&f, &["--owner", "alice", "task", "claim", &id]);
    let (_, stderr, code) = run(&f, &["--json", "--owner", "bob", "task", "claim", &id]);
    assert_eq!(code, 4);
    let value = json(&stderr);
    assert_eq!(value["kind"], Json::String("claim_conflict".into()));
    assert_eq!(value["task_id"], Json::String(id));
    assert_eq!(value["existing_owner"], Json::String("alice".into()));
    // `retry_after_ms` is advice for a contended lock, not for a durable claim.
    assert!(value.get("retry_after_ms").is_none());
}

#[test]
fn claiming_a_terminal_task_is_a_no_op_for_anyone() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    ok(&f, &["--owner", "alice", "task", "claim", &id]);
    ok(&f, &["--owner", "alice", "task", "finish", &id]);
    let before = bytes(&f, &id);
    let out = ok(&f, &["--owner", "bob", "task", "claim", &id]);
    assert_eq!(out, format!("claimed {id}"));
    assert_eq!(bytes(&f, &id), before);
    // finish never clears claimed_by.
    assert!(before.contains("claimed_by: alice"));
}

#[test]
fn claim_without_an_identity_is_exit_two() {
    let f = VaultFixture::with("[core]\nvault_path = \"{VAULT}\"\n\n[tasks]\ncollections = []\n");
    let id = new_task(&f, &["T"]);
    let (_, stderr, code) = run(&f, &["task", "claim", &id]);
    assert_eq!(code, 2);
    assert_eq!(
        stderr.trim_end(),
        "no agent identity: set [core].agent or pass --owner"
    );
}

#[test]
fn releasing_an_unclaimed_task_leaves_the_bytes_untouched() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    let before = bytes(&f, &id);
    assert_eq!(ok(&f, &["task", "release", &id]), format!("released {id}"));
    assert_eq!(bytes(&f, &id), before);
}

#[test]
fn release_needs_force_to_break_a_foreign_claim() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    ok(&f, &["--owner", "alice", "task", "claim", &id]);
    let (_, _, code) = run(&f, &["--owner", "bob", "task", "release", &id]);
    assert_eq!(code, 4);
    ok(&f, &["--owner", "bob", "task", "release", &id, "--force"]);
    let text = bytes(&f, &id);
    assert!(text.contains("status: open"));
    assert!(text.contains("claimed_by: null"));
}

#[test]
fn release_note_appends_one_stamped_block_attributed_to_the_releaser() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T", "--body", "start"]);
    ok(&f, &["--owner", "alice", "task", "claim", &id]);
    ok(
        &f,
        &[
            "--owner",
            "alice",
            "task",
            "release",
            &id,
            "--note",
            "handing off",
        ],
    );
    let text = bytes(&f, &id);
    assert_eq!(text.matches("handing off").count(), 1);
    assert!(text.contains(" — alice\nhanding off"), "{text}");
    assert!(text.contains("status: open"));
}

// ---------------------------------------------------------------------------------------
// task finish / cancel
// ---------------------------------------------------------------------------------------

#[test]
fn finish_writes_an_outcome_section_and_moves_the_file() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T", "--body", "Original body."]);
    let out = ok(
        &f,
        &[
            "--owner",
            "alice",
            "task",
            "finish",
            &id,
            "--outcome",
            "Shipped.",
        ],
    );
    assert_eq!(out, format!("finished {id}"));
    assert_eq!(task_path(&f, &id), format!("tasks/done/{id}.md"));
    let text = bytes(&f, &id);
    assert!(text.contains("status: done"));
    assert!(text.contains("Original body.\n\n## Outcome\n\n"), "{text}");
    assert!(text.trim_end().ends_with(" — alice\nShipped."), "{text}");
}

#[test]
fn cancel_writes_a_cancelled_section() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    let out = ok(&f, &["task", "cancel", &id, "--reason", "not needed"]);
    assert_eq!(out, format!("cancelled {id}"));
    let text = bytes(&f, &id);
    assert!(text.contains("status: cancelled"));
    assert!(text.contains("## Cancelled\n\n"));
    assert!(text.trim_end().ends_with(" — test-agent\nnot needed"));
}

#[test]
fn a_second_terminal_call_is_a_pure_no_op() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    ok(&f, &["task", "finish", &id, "--outcome", "one"]);
    let before = bytes(&f, &id);
    ok(&f, &["task", "finish", &id, "--outcome", "two"]);
    assert_eq!(bytes(&f, &id), before);
    // A cross-transition is refused as a no-op too.
    ok(&f, &["task", "cancel", &id]);
    assert_eq!(bytes(&f, &id), before);
    assert_eq!(before.matches("## Outcome").count(), 1);
    assert!(!before.contains("## Cancelled"));
}

#[test]
fn a_crash_stranded_terminal_file_is_reconciled_into_done() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    // Simulate a crash between the in-place write and the rename.
    let stranded = f
        .read(&format!("tasks/open/{id}.md"))
        .replace("status: open", "status: done");
    f.write(&format!("tasks/open/{id}.md"), &stranded);
    ok(&f, &["task", "finish", &id]);
    assert_eq!(task_path(&f, &id), format!("tasks/done/{id}.md"));
    assert!(!f.files().contains(&format!("tasks/open/{id}.md")));
    // No second section was appended by the reconcile.
    assert!(!bytes(&f, &id).contains("## Outcome"));
}

#[test]
fn finish_with_no_outcome_is_heading_plus_stamp_only() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    ok(&f, &["--owner", "alice", "task", "finish", &id]);
    let text = bytes(&f, &id);
    let body = text.rsplit("---\n").next().unwrap_or_default();
    let lines: Vec<&str> = body.trim().lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines.first().copied(), Some("## Outcome"));
    assert_eq!(lines.get(1).copied(), Some(""));
    assert!(lines
        .get(2)
        .copied()
        .unwrap_or_default()
        .ends_with(" — alice"));
}

#[test]
fn finish_reports_the_newly_unblocked_dependents() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B", "--blocked-by", &a]);
    let (stdout, stderr, code) = run(&f, &["task", "finish", &a]);
    assert_eq!(code, 0);
    assert_eq!(stdout.trim_end(), format!("finished {a}"));
    assert_eq!(stderr.trim_end(), format!("unblocked: {b}"));

    let a2 = new_task(&f, &["A2"]);
    let b2 = new_task(&f, &["B2", "--blocked-by", &a2]);
    let value = json(&ok(&f, &["--json", "task", "cancel", &a2]));
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "status", "unblocked", "updated"]);
    assert_eq!(value["unblocked"], serde_json::json!([b2]));
}

#[test]
fn the_unblocked_notice_is_suppressed_by_quiet() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    new_task(&f, &["B", "--blocked-by", &a]);
    let (stdout, stderr, code) = run(&f, &["--quiet", "task", "finish", &a]);
    assert_eq!(code, 0);
    assert_eq!(stdout.trim_end(), a);
    assert_eq!(stderr, "");
}

// ---------------------------------------------------------------------------------------
// task get
// ---------------------------------------------------------------------------------------

#[test]
fn get_prints_fourteen_meta_lines_then_ready_then_the_preview() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T", "--body", "the body", "--priority", "high"]);
    let out = ok(&f, &["task", "get", &id]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 17, "{out}");
    assert_eq!(
        lines
            .iter()
            .take(14)
            .map(|l| l.split(':').next().unwrap_or(""))
            .collect::<Vec<_>>(),
        [
            "id",
            "type",
            "title",
            "status",
            "priority",
            "owner",
            "claimed_by",
            "project",
            "tags",
            "blocks",
            "blocked_by",
            "created",
            "updated",
            "related"
        ]
    );
    assert_eq!(lines.get(14).copied(), Some("ready: true"));
    assert_eq!(lines.get(15).copied(), Some(""));
    assert_eq!(lines.get(16).copied(), Some("the body"));
}

#[test]
fn get_meta_only_drops_the_preview_and_the_body_key() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T", "--body", "the body"]);
    let out = ok(&f, &["task", "get", &id, "--meta-only"]);
    assert_eq!(out.lines().count(), 15);
    assert!(!out.contains("the body"));
    let value = json(&ok(&f, &["--json", "task", "get", &id, "--meta-only"]));
    assert!(value.get("body").is_none());
    assert_eq!(value["ready"], Json::Bool(true));
}

#[test]
fn get_json_is_the_frontmatter_plus_body_then_ready() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T", "--body", "the body"]);
    let value = json(&ok(&f, &["--json", "task", "get", &id]));
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "type",
            "title",
            "tags",
            "owner",
            "created",
            "updated",
            "related",
            "status",
            "priority",
            "claimed_by",
            "project",
            "blocks",
            "blocked_by",
            "body",
            "ready"
        ]
    );
    assert_eq!(value["body"], Json::String("the body".into()));
}

#[test]
fn get_reports_readiness_from_the_derived_graph() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B", "--blocked-by", &a]);
    assert!(ok(&f, &["task", "get", &b]).contains("ready: false"));
    ok(&f, &["task", "finish", &a]);
    assert!(ok(&f, &["task", "get", &b]).contains("ready: true"));
}

#[test]
fn get_quiet_prints_the_id_but_json_wins() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    assert_eq!(ok(&f, &["--quiet", "task", "get", &id]), id);
    assert!(ok(&f, &["--json", "--quiet", "task", "get", &id]).starts_with('{'));
}

#[test]
fn get_truncates_the_preview_at_two_hundred_code_points() {
    let f = VaultFixture::new();
    let body = "é".repeat(300);
    let id = new_task(&f, &["T", "--body", &body]);
    let out = ok(&f, &["task", "get", &id]);
    let preview = out.lines().last().unwrap_or_default();
    assert_eq!(preview.chars().count(), 200);
    let out = ok(&f, &["task", "get", &id, "--full"]);
    assert_eq!(out.lines().last().unwrap_or_default().chars().count(), 300);
}

#[test]
fn get_never_resolves_a_title_slug() {
    let f = VaultFixture::new();
    new_task(&f, &["Ship it"]);
    let (_, stderr, code) = run(&f, &["task", "get", "ship-it"]);
    assert_eq!(code, 3);
    assert_eq!(stderr.trim_end(), "task not found: ship-it");
}

#[test]
fn a_corrupt_task_is_not_found_on_read_and_amend_but_still_deletable() {
    let f = VaultFixture::new();
    f.write("tasks/open/t-BAD.md", "---\n: : :\n---\n\nbroken\n");
    for args in [
        vec!["task", "get", "t-BAD"],
        vec!["task", "append", "t-BAD", "x"],
        vec!["task", "update", "t-BAD", "--title", "x"],
        vec!["task", "claim", "t-BAD"],
        vec!["task", "finish", "t-BAD"],
    ] {
        let (_, _, code) = run(&f, &args);
        assert_eq!(code, 3, "{args:?}");
    }
    // Listings skip it silently.
    assert_eq!(ok(&f, &["task", "list"]), "");
    ok(&f, &["task", "delete", "t-BAD", "--force"]);
    assert!(!f.files().contains(&"tasks/open/t-BAD.md".to_string()));
}

#[test]
fn a_not_found_json_envelope_carries_candidates() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    let (_, stderr, code) = run(&f, &["--json", "task", "get", "t-ZZZZ"]);
    assert_eq!(code, 3);
    let value = json(&stderr);
    assert_eq!(value["kind"], Json::String("not_found".into()));
    assert_eq!(
        value["message"],
        Json::String("task not found: t-ZZZZ".into())
    );
    let candidates = value["candidates"].as_array().expect("candidates");
    assert!(candidates.contains(&Json::String(id)), "{candidates:?}");
}

// ---------------------------------------------------------------------------------------
// task list
// ---------------------------------------------------------------------------------------

#[test]
fn list_rows_are_tab_separated_with_a_dash_for_an_unclaimed_holder() {
    let f = VaultFixture::new();
    let open = new_task(&f, &["Open Task"]);
    let held = new_task(&f, &["Held Task"]);
    ok(&f, &["--owner", "agent-a", "task", "claim", &held]);
    let out = ok(&f, &["task", "list", "--sort", "title"]);
    let rows: Vec<Vec<&str>> = out.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(
        rows,
        [
            vec![held.as_str(), "claimed", "agent-a", "Held Task"],
            vec![open.as_str(), "open", "-", "Open Task"],
        ]
    );
}

#[test]
fn list_json_is_an_array_of_full_model_dumps() {
    let f = VaultFixture::new();
    new_task(&f, &["T"]);
    let value = json(&ok(&f, &["--json", "task", "list"]));
    let rows = value.as_array().expect("array");
    assert_eq!(rows.len(), 1);
    let keys: Vec<&str> = rows[0]
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.first().copied(), Some("id"));
    assert_eq!(keys.len(), 14);
    assert!(!keys.contains(&"body"));
    assert!(!keys.contains(&"path"));
}

#[test]
fn list_quiet_prints_one_id_per_line() {
    let f = VaultFixture::new();
    new_task(&f, &["A"]);
    new_task(&f, &["B"]);
    let out = ok(&f, &["--quiet", "task", "list"]);
    assert_eq!(out.lines().count(), 2);
    assert!(out.lines().all(|l| l.starts_with("t-")));
}

#[test]
fn list_status_filter_is_a_union_and_rejects_unknowns() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    ok(&f, &["task", "claim", &b]);
    let c = new_task(&f, &["C"]);
    ok(&f, &["task", "finish", &c]);
    assert_eq!(
        ok(&f, &["--quiet", "task", "list", "--status", "open"])
            .lines()
            .count(),
        1
    );
    assert_eq!(
        ok(&f, &["--quiet", "task", "list", "--status", "open,claimed"])
            .lines()
            .count(),
        2
    );
    assert_eq!(
        ok(&f, &["--quiet", "task", "list", "--status", " , "])
            .lines()
            .count(),
        3
    );
    let (_, stderr, code) = run(&f, &["task", "list", "--status", "open,wat,nope"]);
    assert_eq!(code, 2);
    assert_eq!(
        stderr.trim_end(),
        "unknown status: wat, nope (use open, claimed, done, cancelled)"
    );
    let _ = (a, b);
}

#[test]
fn available_defaults_the_sort_to_priority_but_an_explicit_sort_wins() {
    let f = VaultFixture::new();
    let low = new_task(&f, &["low", "--priority", "low"]);
    let high = new_task(&f, &["high", "--priority", "high"]);
    let out = ok(&f, &["--quiet", "task", "list", "--available"]);
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [high.as_str(), low.as_str()]
    );
    let out = ok(
        &f,
        &[
            "--quiet",
            "task",
            "list",
            "--available",
            "--sort",
            "created",
        ],
    );
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [high.as_str(), low.as_str()]
    );
    // Without --available the default is `updated`, newest first.
    let out = ok(&f, &["--quiet", "task", "list"]);
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [high.as_str(), low.as_str()]
    );
}

#[test]
fn available_excludes_a_hand_edited_open_task_carrying_a_claim() {
    let f = VaultFixture::new();
    f.write(
        "tasks/open/t-STALE.md",
        "---\nid: t-STALE\ntype: task\ntitle: Stale\ncreated: 2026-01-01T00:00:00Z\n\
         updated: 2026-01-01T00:00:00Z\nstatus: open\nclaimed_by: ghost\n---\n\nx\n",
    );
    assert_eq!(ok(&f, &["--quiet", "task", "list"]), "t-STALE");
    assert_eq!(ok(&f, &["--quiet", "task", "list", "--available"]), "");
    // …but --status open still shows it. That is the difference between the two.
    assert_eq!(
        ok(&f, &["--quiet", "task", "list", "--status", "open"]),
        "t-STALE"
    );
}

#[test]
fn mine_with_no_identity_matches_nothing() {
    let f = VaultFixture::with("[core]\nvault_path = \"{VAULT}\"\n\n[tasks]\ncollections = []\n");
    f.write(
        "tasks/open/t-ANON.md",
        "---\nid: t-ANON\ntype: task\ntitle: Anon\ncreated: 2026-01-01T00:00:00Z\n\
         updated: 2026-01-01T00:00:00Z\nstatus: open\nowner: null\n---\n\nx\n",
    );
    assert_eq!(ok(&f, &["--quiet", "task", "list"]), "t-ANON");
    assert_eq!(ok(&f, &["--quiet", "task", "list", "--mine"]), "");
}

#[test]
fn mine_matches_what_i_own_or_have_claimed() {
    let f = VaultFixture::new();
    let mine = new_task(&f, &["Mine"]);
    let theirs = new_task(&f, &["Theirs", "--owner", "bob"]);
    ok(&f, &["task", "claim", &theirs]);
    let out = ok(
        &f,
        &["--quiet", "task", "list", "--mine", "--sort", "title"],
    );
    assert_eq!(out.lines().count(), 2);
    let hidden = new_task(&f, &["Hidden", "--owner", "carol"]);
    let out = ok(&f, &["--quiet", "task", "list", "--mine"]);
    assert!(!out.contains(&hidden));
    assert!(out.contains(&mine));
}

#[test]
fn list_filters_by_tags_project_and_limit() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A", "--tags", "x,y", "--project", "n-P"]);
    let b = new_task(&f, &["B", "--tags", "x"]);
    assert_eq!(ok(&f, &["--quiet", "task", "list", "--tags", "x,y"]), a);
    assert_eq!(
        ok(
            &f,
            &["--quiet", "task", "list", "--tags", "x,y", "--any-tag"]
        )
        .lines()
        .count(),
        2
    );
    assert_eq!(ok(&f, &["--quiet", "task", "list", "--project", "n-P"]), a);
    assert_eq!(
        ok(&f, &["--quiet", "task", "list", "--limit", "1"])
            .lines()
            .count(),
        1
    );
    assert_eq!(ok(&f, &["--quiet", "task", "list", "--limit", "0"]), "");
    assert_eq!(
        ok(&f, &["--quiet", "task", "list", "--limit=-1"])
            .lines()
            .count(),
        2
    );
    let _ = b;
}

#[test]
fn since_is_a_floor_and_stale_a_ceiling() {
    let f = VaultFixture::new();
    f.write(
        "tasks/open/t-OLD.md",
        "---\nid: t-OLD\ntype: task\ntitle: Old\ncreated: 2020-01-01T00:00:00Z\n\
         updated: 2020-01-01T00:00:00Z\nstatus: open\n---\n\nx\n",
    );
    let fresh = new_task(&f, &["Fresh"]);
    assert_eq!(ok(&f, &["--quiet", "task", "list", "--since", "7d"]), fresh);
    assert_eq!(
        ok(&f, &["--quiet", "task", "list", "--stale", "7d"]),
        "t-OLD"
    );
}

#[test]
fn an_invalid_sort_names_the_four_task_keys() {
    let f = VaultFixture::new();
    let (_, stderr, code) = run(&f, &["task", "list", "--sort", "bogus"]);
    assert_eq!(code, 2);
    assert_eq!(
        stderr.trim_end(),
        "invalid sort field: 'bogus' (use updated, created, title, priority)"
    );
}

// ---------------------------------------------------------------------------------------
// task delete
// ---------------------------------------------------------------------------------------

#[test]
fn delete_refuses_a_machine_path_without_force() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    let (_, stderr, code) = run(&f, &["--json", "task", "delete", &id]);
    assert_eq!(code, 2);
    assert!(stderr.contains("refusing to delete on a non-interactive path"));
    assert!(f.files().iter().any(|p| p.ends_with(&format!("{id}.md"))));
}

#[test]
fn delete_with_force_removes_the_file_in_any_state() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T"]);
    ok(&f, &["task", "finish", &id]);
    assert_eq!(
        ok(&f, &["task", "delete", &id, "--force"]),
        format!("deleted {id}")
    );
    assert!(!f.files().iter().any(|p| p.ends_with(&format!("{id}.md"))));
    let (_, _, code) = run(&f, &["task", "delete", &id, "--force"]);
    assert_eq!(code, 3);
}

#[test]
fn delete_json_and_quiet_shapes() {
    let f = VaultFixture::new();
    let a = new_task(&f, &["A"]);
    let b = new_task(&f, &["B"]);
    let value = json(&ok(&f, &["--json", "task", "delete", &a, "--force"]));
    assert_eq!(value, serde_json::json!({"id": a, "deleted": true}));
    assert_eq!(ok(&f, &["--quiet", "task", "delete", &b, "--force"]), b);
}

// ---------------------------------------------------------------------------------------
// flag placement, help and no-args
// ---------------------------------------------------------------------------------------

#[test]
fn both_flag_positions_are_byte_identical() {
    let f = VaultFixture::new();
    let id = new_task(&f, &["T", "--body", "x"]);
    for args in [
        vec!["task", "get", id.as_str()],
        vec!["task", "list"],
        vec!["task", "list", "--available"],
    ] {
        let mut left = vec!["--json"];
        left.extend_from_slice(&args);
        let mut right = args.clone();
        right.push("--json");
        assert_eq!(run(&f, &left).0, run(&f, &right).0, "{args:?}");

        let mut left = vec!["--quiet"];
        left.extend_from_slice(&args);
        let mut right = args.clone();
        right.push("--quiet");
        assert_eq!(run(&f, &left).0, run(&f, &right).0, "{args:?}");
    }
    // The idempotent no-op branches too.
    ok(&f, &["task", "claim", &id]);
    assert_eq!(
        run(&f, &["--json", "task", "claim", &id]).0,
        run(&f, &["task", "claim", &id, "--json"]).0
    );
    ok(&f, &["task", "finish", &id]);
    assert_eq!(
        run(&f, &["--json", "task", "finish", &id]).0,
        run(&f, &["task", "finish", &id, "--json"]).0
    );
}

#[test]
fn task_with_no_subcommand_prints_long_help_to_stdout_and_exits_two() {
    let f = VaultFixture::new();
    let (stdout, stderr, code) = run(&f, &["task"]);
    assert_eq!(code, 2);
    assert_eq!(stderr, "");
    for verb in [
        "new", "update", "append", "claim", "release", "finish", "cancel", "get", "list", "delete",
        "block", "unblock", "next",
    ] {
        assert!(stdout.contains(verb), "missing {verb}");
    }
}

#[test]
fn the_task_help_lists_the_subcommands_in_registration_order() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["task", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Create a task"))
        .stdout(predicate::str::contains("Pick the next ready task"));
}

#[test]
fn no_failure_path_ever_prints_a_backtrace() {
    let f = VaultFixture::new();
    for args in [
        vec!["task", "get", "t-NOPE"],
        vec!["task", "new", "T", "--priority", "urgent"],
        vec!["task", "list", "--sort", "bogus"],
        vec!["task", "delete", "t-NOPE", "--force"],
    ] {
        let (_, stderr, code) = run(&f, &args);
        assert_ne!(code, 0, "{args:?}");
        assert!(!stderr.contains("panicked"), "{args:?} {stderr}");
        assert!(!stderr.contains("RUST_BACKTRACE"), "{args:?} {stderr}");
        assert_eq!(stderr.lines().count(), 1, "{args:?} {stderr}");
    }
}

#[test]
fn a_missing_config_is_the_three_line_message_at_exit_two() {
    let f = VaultFixture::new();
    let out = f.bare_cmd().args(["task", "list"]).output().expect("run");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.lines().count(), 3, "{stderr}");
    assert!(stderr.starts_with("mesh: no config found at"));
}

#[test]
fn a_disabled_tasks_space_is_exit_two() {
    let f = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"a\"\n\n[tasks]\ncollections = []\n\n\
         [spaces]\ntasks = false\n",
    );
    let (_, stderr, code) = run(&f, &["task", "new", "T"]);
    assert_eq!(code, 2);
    assert_eq!(stderr.trim_end(), "space 'tasks' is disabled in [spaces]");
}

// ---------------------------------------------------------------------------------------
// the Python corpus
// ---------------------------------------------------------------------------------------

#[test]
fn the_corpus_lists_every_python_written_task() {
    let f = VaultFixture::from_corpus();
    let out = ok(&f, &["--quiet", "task", "list", "--sort", "title"]);
    let mut ids: Vec<&str> = out.lines().collect();
    ids.sort();
    assert_eq!(
        ids,
        ["t-1FN1", "t-1Z4Y", "t-99J7", "t-D0YQ", "t-LEGP", "t-TCY1"]
    );
}

#[test]
fn a_python_written_task_reads_back_with_its_typed_values() {
    let f = VaultFixture::from_corpus();
    let out = ok(&f, &["task", "get", "t-TCY1"]);
    assert!(out.contains("title: Ship it"));
    assert!(out.contains("status: open"));
    assert!(out.contains("priority: high"));
    assert!(out.contains("project: n-19EP"));
    assert!(out.contains("owner: demo-agent"));
    assert!(out.contains("tags: x"));
    // The space-separated Python timestamp is normalised to `Z` on the surface.
    assert!(
        out.contains("created: 2026-09-05T08:45:38.869914Z"),
        "{out}"
    );
    assert!(out.ends_with("Original body."), "{out}");
}

#[test]
fn a_read_never_rewrites_a_python_written_file() {
    let f = VaultFixture::from_corpus();
    let before = f.read("tasks/open/t-TCY1.md");
    ok(&f, &["task", "get", "t-TCY1"]);
    ok(&f, &["task", "list"]);
    ok(&f, &["--json", "task", "get", "t-TCY1"]);
    assert_eq!(f.read("tasks/open/t-TCY1.md"), before);
}

#[test]
fn claiming_and_finishing_a_python_written_task_works() {
    let f = VaultFixture::from_corpus();
    ok(&f, &["--owner", "demo-agent", "task", "claim", "t-TCY1"]);
    assert!(f
        .read("tasks/open/t-TCY1.md")
        .contains("claimed_by: demo-agent"));
    ok(
        &f,
        &[
            "--owner",
            "demo-agent",
            "task",
            "finish",
            "t-TCY1",
            "--outcome",
            "done",
        ],
    );
    assert!(f.files().contains(&"tasks/done/t-TCY1.md".to_string()));
    let text = f.read("tasks/done/t-TCY1.md");
    assert!(text.contains("status: done"));
    assert!(text.contains("## Outcome"));
    // The Python key order and the unknown keys survived the rewrite.
    assert!(text.starts_with("---\nblocked_by: []\nblocks:"), "{text}");
}

#[test]
fn a_python_written_claim_conflicts_with_a_different_agent() {
    let f = VaultFixture::from_corpus();
    let (_, stderr, code) = run(&f, &["--owner", "other", "task", "claim", "t-D0YQ"]);
    assert_eq!(code, 4);
    assert_eq!(
        stderr.trim_end(),
        "task t-D0YQ already claimed by demo-agent"
    );
}

#[test]
fn a_python_written_terminal_task_is_an_idempotent_no_op() {
    let f = VaultFixture::from_corpus();
    let done = f.read("tasks/done/t-99J7.md");
    let cancelled = f.read("tasks/done/t-1Z4Y.md");
    ok(&f, &["task", "finish", "t-99J7"]);
    ok(&f, &["task", "cancel", "t-1Z4Y"]);
    ok(&f, &["task", "claim", "t-99J7"]);
    assert_eq!(f.read("tasks/done/t-99J7.md"), done);
    assert_eq!(f.read("tasks/done/t-1Z4Y.md"), cancelled);
}

#[test]
fn the_corpus_readiness_flips_when_the_blocker_finishes() {
    let f = VaultFixture::from_corpus();
    assert!(ok(&f, &["task", "get", "t-1FN1"]).contains("ready: false"));
    assert_eq!(ok(&f, &["--quiet", "task", "list", "--blocked"]), "t-1FN1");
    assert!(!ok(&f, &["--quiet", "task", "list", "--ready"]).contains("t-1FN1"));

    let (_, stderr, _) = run(&f, &["task", "finish", "t-TCY1"]);
    assert_eq!(stderr.trim_end(), "unblocked: t-1FN1");
    assert!(ok(&f, &["task", "get", "t-1FN1"]).contains("ready: true"));
    assert!(ok(&f, &["--quiet", "task", "list", "--ready"]).contains("t-1FN1"));
    assert_eq!(ok(&f, &["--quiet", "task", "list", "--blocked"]), "");
}

#[test]
fn a_legacy_free_form_priority_sorts_last_and_is_never_dropped() {
    let f = VaultFixture::from_corpus();
    assert!(ok(&f, &["task", "get", "t-LEGP"]).contains("priority: urgent"));
    let out = ok(
        &f,
        &[
            "--quiet",
            "task",
            "list",
            "--available",
            "--sort",
            "priority",
        ],
    );
    let ids: Vec<&str> = out.lines().collect();
    assert!(ids.contains(&"t-LEGP"), "{ids:?}");
    assert_eq!(ids.last().copied(), Some("t-LEGP"), "{ids:?}");
    assert_eq!(ids.first().copied(), Some("t-TCY1"), "{ids:?}");
}

#[test]
fn a_corpus_update_keeps_the_python_key_order_and_unknown_keys() {
    let f = VaultFixture::from_corpus();
    ok(&f, &["task", "update", "t-LEGP", "--priority", "low"]);
    let text = f.read("tasks/open/t-LEGP.md");
    assert!(text.contains("priority: low"));
    // Alphabetical Python order is preserved, key for key.
    assert!(text.starts_with("---\nblocked_by: []\nblocks: []\nclaimed_by: null\ncreated:"));
    assert!(text.contains("tags:\n  - legacy\n"));
}

#[test]
fn a_status_union_filters_before_the_limit_not_after() {
    let f = VaultFixture::new();
    for n in 0..4 {
        let id = new_task(&f, &[&format!("done-{n}")]);
        ok(&f, &["task", "finish", &id]);
    }
    let open = new_task(&f, &["still open"]);
    // `updated` descending puts the four done tasks first; a limit applied after the
    // status filter must still surface the single open one.
    let out = ok(
        &f,
        &[
            "--quiet", "task", "list", "--status", "open", "--limit", "2",
        ],
    );
    assert_eq!(out, open);
    let out = ok(
        &f,
        &[
            "--quiet", "task", "list", "--status", "done", "--limit", "2",
        ],
    );
    assert_eq!(out.lines().count(), 2);
    assert_eq!(
        ok(
            &f,
            &["--quiet", "task", "list", "--status", "done", "--limit", "0"]
        ),
        ""
    );
    assert_eq!(
        ok(
            &f,
            &[
                "--quiet",
                "task",
                "list",
                "--status",
                "done,open",
                "--limit=-1"
            ]
        )
        .lines()
        .count(),
        5
    );
}
