//! `mesh memory` end to end: every subcommand, every output mode, every error path.
//!
//! Each test drives the real binary through `VaultFixture`, so nothing here touches the
//! process environment and the file is safe to run at default parallelism.

mod common;

use std::process::Output;

use common::VaultFixture;
use predicates::prelude::*;
use serde_json::Value as Json;

// ---------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code_of(out: &Output) -> Option<i32> {
    out.status.code()
}

fn json_of(out: &Output) -> Json {
    serde_json::from_str(&stdout_of(out)).expect("stdout is one JSON line")
}

/// Record a memory and return its id.
fn new_memory(f: &VaultFixture, title: &str, body: &str) -> String {
    let out = f
        .cmd()
        .args(["memory", "new", title, "--body", body, "--quiet"])
        .output()
        .expect("memory new");
    assert_eq!(code_of(&out), Some(0), "{}", stderr_of(&out));
    stdout_of(&out).trim().to_string()
}

/// A hand-written memory file, so a test can pin `expires`, `updated` and `scope` exactly.
struct Seed<'a> {
    id: &'a str,
    title: &'a str,
    body: &'a str,
    kind: &'a str,
    scope: &'a str,
    owner: &'a str,
    importance: i64,
    tags: &'a str,
    updated: String,
    expires: &'a str,
    superseded_by: &'a str,
}

impl Default for Seed<'_> {
    fn default() -> Self {
        Seed {
            id: "m-SEED",
            title: "Seed",
            body: "body",
            kind: "fact",
            scope: "shared",
            owner: "test-agent",
            importance: 3,
            tags: "[]",
            updated: "2026-01-02T00:00:00Z".to_string(),
            expires: "null",
            superseded_by: "null",
        }
    }
}

fn seed(f: &VaultFixture, s: Seed<'_>) {
    let text = format!(
        "---\nid: {id}\ntype: memory\ntitle: {title}\ntags: {tags}\nowner: {owner}\n\
         created: 2026-01-01T00:00:00Z\nupdated: {updated}\nrelated: []\nkind: {kind}\n\
         scope: {scope}\nimportance: {importance}\nsource: null\nexpires: {expires}\n\
         superseded_by: {superseded}\n---\n\n{body}\n",
        id = s.id,
        title = s.title,
        tags = s.tags,
        owner = s.owner,
        updated = s.updated,
        kind = s.kind,
        scope = s.scope,
        importance = s.importance,
        expires = s.expires,
        superseded = s.superseded_by,
        body = s.body,
    );
    f.write(&format!("memories/{}.md", s.id), &text);
}

/// An ISO stamp `days` in the past.
fn days_ago(days: i64) -> String {
    mesh::timefmt::iso_z(&(mesh::timefmt::now_utc() - chrono::Duration::days(days)))
}

fn ids_of(payload: &Json) -> Vec<String> {
    payload
        .as_array()
        .expect("an array")
        .iter()
        .filter_map(|e| e.get("id").and_then(Json::as_str))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------------------
// new
// ---------------------------------------------------------------------------------------

#[test]
fn new_writes_a_flat_file_with_the_declared_frontmatter() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "the widget is blue");
    assert!(id.starts_with("m-"), "{id}");
    let files = f.files();
    assert!(
        files.iter().any(|p| p == &format!("memories/{id}.md")),
        "{files:?}"
    );
    let text = f.read(&format!("memories/{id}.md"));
    for key in [
        "id:",
        "type: memory",
        "title: Alpha",
        "tags: []",
        "owner: test-agent",
        "created:",
        "updated:",
        "related: []",
        "kind: fact",
        "scope: shared",
        "importance: 3",
        "source: null",
        "expires: null",
        "superseded_by: null",
    ] {
        assert!(text.contains(key), "missing {key} in\n{text}");
    }
    assert!(text.ends_with("the widget is blue\n"), "{text}");
}

#[test]
fn new_human_output_is_created_plus_the_id() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["memory", "new", "Alpha", "--body", "x"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0));
    assert!(stdout_of(&out).starts_with("created m-"), "{out:?}");
}

#[test]
fn new_json_is_id_kind_updated_in_that_order() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args([
            "memory", "new", "Alpha", "--body", "x", "--kind", "insight", "--json",
        ])
        .output()
        .expect("run");
    let payload = json_of(&out);
    let keys: Vec<&str> = payload
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "kind", "updated"]);
    assert_eq!(payload["kind"], Json::String("insight".into()));
}

#[test]
fn new_quiet_prints_the_id_alone_and_beats_json() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["memory", "new", "Alpha", "--body", "x", "--json", "--quiet"])
        .output()
        .expect("run");
    let text = stdout_of(&out);
    assert!(text.starts_with("m-"), "{text}");
    assert!(!text.contains('{'), "quiet beats json on a class-M verb");
}

#[test]
fn new_accepts_every_flag() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args([
            "memory",
            "new",
            "Alpha",
            "--body",
            "x",
            "--kind",
            "preference",
            "--scope",
            "private",
            "--importance",
            "5",
            "--source",
            "the operator said so",
            "--expires",
            "7d",
            "--tags",
            "a,b",
            "--owner",
            "alice",
            "--json",
        ])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0), "{}", stderr_of(&out));
    let id = json_of(&out)["id"].as_str().expect("id").to_string();
    let text = f.read(&format!("memories/{id}.md"));
    assert!(text.contains("kind: preference"), "{text}");
    assert!(text.contains("scope: private"), "{text}");
    assert!(text.contains("importance: 5"), "{text}");
    assert!(text.contains("source: the operator said so"), "{text}");
    assert!(text.contains("owner: alice"), "{text}");
    assert!(text.contains("- a\n  - b"), "{text}");
    assert!(!text.contains("expires: null"), "{text}");
}

#[test]
fn new_rejects_a_bad_kind() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["memory", "new", "A", "--body", "x", "--kind", "hunch"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "invalid kind: 'hunch' (use fact, preference, procedure, insight, episode)",
        ));
    assert!(f.files().is_empty());
}

#[test]
fn new_rejects_a_bad_scope() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["memory", "new", "A", "--body", "x", "--scope", "team"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "invalid scope: 'team' (use shared, private)",
        ));
}

#[test]
fn new_rejects_an_out_of_range_importance() {
    let f = VaultFixture::new();
    for value in ["0", "6"] {
        f.cmd()
            .args(["memory", "new", "A", "--body", "x", "--importance", value])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("invalid importance:"));
    }
    assert!(f.files().is_empty());
}

#[test]
fn new_rejects_an_unparseable_expiry() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["memory", "new", "A", "--body", "x", "--expires", "soon"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid time value: 'soon'"));
    assert!(f.files().is_empty());
}

#[test]
fn new_reads_the_body_from_a_file() {
    let f = VaultFixture::new();
    let path = f.dir.path().join("body.md");
    std::fs::write(&path, "from a file").expect("write");
    let out = f
        .cmd()
        .args(["memory", "new", "Alpha", "--file"])
        .arg(&path)
        .args(["--quiet"])
        .output()
        .expect("run");
    let id = stdout_of(&out).trim().to_string();
    assert!(f.read(&format!("memories/{id}.md")).contains("from a file"));
}

#[test]
fn new_reports_an_unreadable_file() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["memory", "new", "A", "--file", "/definitely/not/here.md"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cannot read --file /definitely/not/here.md",
        ));
}

#[test]
fn new_without_a_body_on_a_headless_path_is_exit_two() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["memory", "new", "A", "--json"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "no body: pass --body or --file on a non-interactive path",
        ));
}

#[test]
fn new_emits_the_duplicate_title_advisory_and_still_succeeds() {
    let f = VaultFixture::new();
    let first = new_memory(&f, "Japan Visa", "x");
    let out = f
        .cmd()
        .args(["memory", "new", "  japan   visa!", "--body", "y"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0));
    assert!(
        stderr_of(&out).contains(&format!(
            "memory new: duplicate title, also used by {first}"
        )),
        "{}",
        stderr_of(&out)
    );
    assert!(!stdout_of(&out).contains("duplicate"));
}

#[test]
fn the_duplicate_advisory_is_suppressed_by_quiet() {
    let f = VaultFixture::new();
    new_memory(&f, "Japan Visa", "x");
    let out = f
        .cmd()
        .args(["memory", "new", "japan visa", "--body", "y", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stderr_of(&out), "");
}

#[test]
fn a_note_with_the_same_title_never_triggers_the_advisory() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["note", "new", "Shared Name", "--body", "x", "--quiet"])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["memory", "new", "Shared Name", "--body", "y"])
        .output()
        .expect("run");
    assert_eq!(stderr_of(&out), "", "the advisory is memories-only");
}

#[test]
fn supersedes_stamps_the_old_memory() {
    let f = VaultFixture::new();
    let old = new_memory(&f, "Old belief", "x");
    let out = f
        .cmd()
        .args([
            "memory",
            "new",
            "New belief",
            "--body",
            "y",
            "--supersedes",
            &old,
            "--quiet",
        ])
        .output()
        .expect("run");
    let new = stdout_of(&out).trim().to_string();
    let text = f.read(&format!("memories/{old}.md"));
    assert!(text.contains(&format!("superseded_by: {new}")), "{text}");
}

#[test]
fn a_failed_supersession_is_a_warning_not_a_failure() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args([
            "memory",
            "new",
            "Belief",
            "--body",
            "y",
            "--supersedes",
            "m-GHOST",
        ])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0));
    assert!(
        stderr_of(&out)
            .contains("memory new: could not supersede m-GHOST (memory not found: m-GHOST)"),
        "{}",
        stderr_of(&out)
    );
    assert!(stdout_of(&out).starts_with("created m-"));
}

#[test]
fn new_rejects_an_owner_outside_the_roster() {
    let f = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"alice\"\n\n\
         [tasks]\ncollections = [\"alice\"]\n",
    );
    f.cmd()
        .args(["memory", "new", "A", "--body", "x", "--owner", "ghost"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown owner: 'ghost'"));
    assert!(f.files().is_empty());
}

#[test]
fn an_expiry_duration_is_read_forwards_from_now() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args([
            "memory",
            "new",
            "A",
            "--body",
            "x",
            "--expires",
            "7d",
            "--json",
        ])
        .output()
        .expect("run");
    let id = json_of(&out)["id"].as_str().expect("id").to_string();
    let payload = f
        .cmd()
        .args(["memory", "get", &id, "--json"])
        .output()
        .expect("get");
    let expires = json_of(&payload)["expires"]
        .as_str()
        .expect("expires")
        .to_string();
    let at = mesh::timefmt::parse_since(&expires).expect("parse");
    assert!(at > mesh::timefmt::now_utc(), "{expires} is in the future");
}

// ---------------------------------------------------------------------------------------
// append
// ---------------------------------------------------------------------------------------

#[test]
fn append_adds_a_block_and_bumps_updated() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "one");
    let before = f.read(&format!("memories/{id}.md"));
    let out = f
        .cmd()
        .args(["memory", "append", &id, "two"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("appended {id}\n"));
    let after = f.read(&format!("memories/{id}.md"));
    assert!(after.contains("one\n\ntwo"), "{after}");
    assert_ne!(before, after);
}

#[test]
fn append_under_a_section_creates_it_when_absent() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "Intro.\n\n## A\n\nitem1");
    f.cmd()
        .args(["memory", "append", &id, "NEW", "--section", "A"])
        .assert()
        .success();
    let text = f.read(&format!("memories/{id}.md"));
    assert!(text.contains("## A\n\nitem1\n\nNEW"), "{text}");
    f.cmd()
        .args(["memory", "append", &id, "Z", "--section", "Zed"])
        .assert()
        .success();
    assert!(f.read(&format!("memories/{id}.md")).contains("## Zed\n\nZ"));
}

#[test]
fn a_timestamped_append_stamps_the_body_never_the_frontmatter() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "one");
    f.cmd()
        .args(["memory", "append", &id, "two", "--timestamp"])
        .assert()
        .success();
    let text = f.read(&format!("memories/{id}.md"));
    assert!(text.contains(" — test-agent\ntwo"), "{text}");
    assert!(!text.contains("actor:"), "{text}");
}

#[test]
fn append_json_carries_id_kind_updated() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "one");
    let out = f
        .cmd()
        .args(["memory", "append", &id, "two", "--json"])
        .output()
        .expect("run");
    let payload = json_of(&out);
    let keys: Vec<&str> = payload
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "kind", "updated"]);
}

#[test]
fn append_to_a_missing_memory_is_exit_three_with_candidates() {
    let f = VaultFixture::new();
    new_memory(&f, "Japan Visa", "x");
    let out = f
        .cmd()
        .args(["--json", "memory", "append", "japan-visas", "more"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(3));
    let envelope: Json =
        serde_json::from_str(&stderr_of(&out)).expect("one JSON envelope on stderr");
    assert_eq!(envelope["kind"], Json::String("not_found".into()));
    assert_eq!(
        envelope["message"],
        Json::String("memory not found: japan-visas".into())
    );
    assert!(envelope["candidates"]
        .as_array()
        .is_some_and(|c| !c.is_empty()));
}

// ---------------------------------------------------------------------------------------
// update
// ---------------------------------------------------------------------------------------

#[test]
fn update_sets_every_field() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    f.cmd()
        .args([
            "memory",
            "update",
            &id,
            "--title",
            "Beta",
            "--kind",
            "episode",
            "--scope",
            "private",
            "--importance",
            "4",
            "--source",
            "chat",
            "--tags",
            "a,b",
            "--expires",
            "2030-01-02T03:04:05Z",
        ])
        .assert()
        .success()
        .stdout(format!("updated {id}\n"));
    let text = f.read(&format!("memories/{id}.md"));
    assert!(text.contains("title: Beta"), "{text}");
    assert!(text.contains("kind: episode"), "{text}");
    assert!(text.contains("scope: private"), "{text}");
    assert!(text.contains("importance: 4"), "{text}");
    assert!(text.contains("source: chat"), "{text}");
    assert!(text.contains("expires: 2030-01-02T03:04:05Z"), "{text}");
}

#[test]
fn update_expires_none_clears_the_soft_ttl() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    f.cmd()
        .args(["memory", "update", &id, "--expires", "30d"])
        .assert()
        .success();
    assert!(!f
        .read(&format!("memories/{id}.md"))
        .contains("expires: null"));
    f.cmd()
        .args(["memory", "update", &id, "--expires", "none"])
        .assert()
        .success();
    assert!(f
        .read(&format!("memories/{id}.md"))
        .contains("expires: null"));
}

#[test]
fn update_applies_the_shared_tag_grammar() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    f.cmd()
        .args(["memory", "update", &id, "--tags", "a,b"])
        .assert()
        .success();
    f.cmd()
        .args(["memory", "update", &id, "--tags", "+c,-a"])
        .assert()
        .success();
    let text = f.read(&format!("memories/{id}.md"));
    assert!(text.contains("- b\n  - c"), "{text}");
    f.cmd()
        .args(["memory", "update", &id, "--tags", "=z"])
        .assert()
        .success();
    let text = f.read(&format!("memories/{id}.md"));
    assert!(text.contains("- z"), "{text}");
    assert!(!text.contains("- b"), "{text}");
}

#[test]
fn a_mixed_tag_spec_is_exit_two_and_writes_nothing() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    let before = f.read(&format!("memories/{id}.md"));
    f.cmd()
        .args(["memory", "update", &id, "--tags", "+a,b"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ambiguous tag spec"));
    assert_eq!(f.read(&format!("memories/{id}.md")), before);
}

#[test]
fn update_reassigns_the_owner_without_folding_the_global_flag() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    // A global --owner is the acting identity, not a reassignment.
    f.cmd()
        .args(["--owner", "alice", "memory", "update", &id, "--tags", "a"])
        .assert()
        .success();
    assert!(f
        .read(&format!("memories/{id}.md"))
        .contains("owner: test-agent"));
    f.cmd()
        .args(["memory", "update", &id, "--owner", "alice"])
        .assert()
        .success();
    assert!(f
        .read(&format!("memories/{id}.md"))
        .contains("owner: alice"));
}

#[test]
fn update_rejects_bad_values_before_the_write() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    let before = f.read(&format!("memories/{id}.md"));
    for args in [
        vec!["--kind", "hunch"],
        vec!["--scope", "team"],
        vec!["--importance", "9"],
        vec!["--expires", "whenever"],
    ] {
        let mut cmd = f.cmd();
        cmd.args(["memory", "update", &id]);
        cmd.args(&args);
        cmd.assert().code(2);
    }
    assert_eq!(f.read(&format!("memories/{id}.md")), before);
}

#[test]
fn an_idempotent_update_leaves_the_file_bytes_unchanged() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    f.cmd()
        .args(["memory", "update", &id, "--tags", "a"])
        .assert()
        .success();
    let before = f.read(&format!("memories/{id}.md"));
    f.cmd()
        .args(["memory", "update", &id, "--tags", "a"])
        .assert()
        .success();
    assert_eq!(f.read(&format!("memories/{id}.md")), before);
    f.cmd()
        .args(["memory", "update", &id, "--kind", "fact"])
        .assert()
        .success();
    assert_eq!(f.read(&format!("memories/{id}.md")), before);
}

#[test]
fn a_memory_is_addressable_by_its_title_slug() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Japan Visa", "x");
    f.cmd()
        .args(["memory", "update", "japan-visa", "--tags", "travel"])
        .assert()
        .success()
        .stdout(format!("updated {id}\n"));
}

// ---------------------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------------------

#[test]
fn get_prints_fourteen_meta_lines_then_a_preview() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "the body");
    let out = f.cmd().args(["memory", "get", &id]).output().expect("run");
    let text = stdout_of(&out);
    let (block, body) = text.split_once("\n\n").expect("a blank line");
    assert_eq!(block.lines().count(), 14, "{block}");
    assert!(block.starts_with(&format!(
        "id: {id}\ntype: memory\ntitle: Alpha\nkind: fact\n"
    )));
    assert!(block.contains("\nscope: shared\nimportance: 3\n"));
    assert_eq!(body.trim_end(), "the body");
}

#[test]
fn get_json_is_the_frontmatter_then_body_last() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "the body");
    let out = f
        .cmd()
        .args(["memory", "get", &id, "--json"])
        .output()
        .expect("run");
    let payload = json_of(&out);
    let keys: Vec<&str> = payload
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
            "kind",
            "scope",
            "importance",
            "source",
            "expires",
            "superseded_by",
            "body"
        ]
    );
    assert_eq!(payload["body"], Json::String("the body".into()));
    assert_eq!(payload["importance"], Json::from(3));
}

#[test]
fn get_meta_only_omits_the_body_on_both_surfaces() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "the body");
    let out = f
        .cmd()
        .args(["memory", "get", &id, "--meta-only", "--json"])
        .output()
        .expect("run");
    assert!(json_of(&out).get("body").is_none());
    let out = f
        .cmd()
        .args(["memory", "get", &id, "--meta-only"])
        .output()
        .expect("run");
    assert!(!stdout_of(&out).contains("the body"));
}

#[test]
fn get_full_prints_the_whole_body_where_a_preview_would_truncate() {
    let f = VaultFixture::new();
    let long = "x".repeat(260);
    let id = new_memory(&f, "Alpha", &long);
    // Count in the preview alone: the meta block's own `expires:` line carries an `x`.
    let preview_of = |out: &Output| -> usize {
        stdout_of(out)
            .split_once("\n\n")
            .map(|(_, body)| body.matches('x').count())
            .expect("a blank line")
    };
    let out = f.cmd().args(["memory", "get", &id]).output().expect("run");
    assert_eq!(preview_of(&out), 200);
    let out = f
        .cmd()
        .args(["memory", "get", &id, "--full"])
        .output()
        .expect("run");
    assert_eq!(preview_of(&out), 260);
}

#[test]
fn get_related_prints_the_derived_ids() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["note", "new", "Target", "--body", "x", "--quiet"])
        .assert()
        .success();
    let id = new_memory(&f, "Linker", "see [[Target]]");
    let out = f
        .cmd()
        .args(["memory", "get", &id, "--related", "--json"])
        .output()
        .expect("run");
    let payload = json_of(&out);
    assert_eq!(payload.as_object().expect("object").len(), 1);
    assert_eq!(payload["related"].as_array().expect("array").len(), 1);
    let out = f
        .cmd()
        .args(["memory", "get", &id, "--related"])
        .output()
        .expect("run");
    assert!(stdout_of(&out).starts_with("n-"));
}

#[test]
fn get_quiet_returns_the_id_and_beats_related() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    let out = f
        .cmd()
        .args(["memory", "get", &id, "--quiet", "--related"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{id}\n"));
}

#[test]
fn an_ambiguous_slug_is_exit_two_with_sorted_ids() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-BBBB",
            title: "Dupe",
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-AAAA",
            title: "Dupe",
            ..Seed::default()
        },
    );
    f.cmd()
        .args(["memory", "get", "dupe"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "ambiguous slug 'dupe': m-AAAA, m-BBBB",
        ));
}

#[test]
fn a_corrupt_memory_is_not_found_on_every_read_and_amend_verb() {
    let f = VaultFixture::new();
    f.write(
        "memories/m-BAD.md",
        "---\nid: m-BAD\ntitle: [unclosed\n---\n\nbroken\n",
    );
    for args in [
        vec!["memory", "get", "m-BAD"],
        vec!["memory", "append", "m-BAD", "x"],
        vec!["memory", "update", "m-BAD", "--tags", "a"],
    ] {
        f.cmd().args(&args).assert().code(3);
    }
    assert!(f.files().iter().any(|p| p == "memories/m-BAD.md"));
}

// ---------------------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------------------

#[test]
fn list_rows_are_id_kind_title_separated_by_two_spaces() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-AAAA",
            title: "Alpha",
            kind: "insight",
            ..Seed::default()
        },
    );
    let out = f.cmd().args(["memory", "list"]).output().expect("run");
    assert_eq!(stdout_of(&out), "m-AAAA  insight  Alpha\n");
}

#[test]
fn list_json_beats_quiet_on_a_class_l_verb() {
    let f = VaultFixture::new();
    seed(&f, Seed::default());
    let out = f
        .cmd()
        .args(["memory", "list", "--json", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(ids_of(&json_of(&out)), ["m-SEED"]);
}

#[test]
fn list_quiet_prints_ids_alone() {
    let f = VaultFixture::new();
    seed(&f, Seed::default());
    let out = f
        .cmd()
        .args(["memory", "list", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), "m-SEED\n");
}

#[test]
fn list_excludes_expired_and_superseded_rows_by_default() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-LIVE",
            title: "Live",
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-GONE",
            title: "Gone",
            expires: "2000-01-01T00:00:00Z",
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-OLD",
            title: "Old",
            superseded_by: "m-LIVE",
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "list", "--json"])
        .output()
        .expect("run");
    assert_eq!(ids_of(&json_of(&out)), ["m-LIVE"]);
}

#[test]
fn list_include_expired_and_include_superseded_bring_them_back() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-LIVE",
            title: "Live",
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-GONE",
            title: "Gone",
            expires: "2000-01-01T00:00:00Z",
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-OLD",
            title: "Old",
            superseded_by: "m-LIVE",
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "list", "--include-expired", "--json"])
        .output()
        .expect("run");
    let mut ids = ids_of(&json_of(&out));
    ids.sort();
    assert_eq!(ids, ["m-GONE", "m-LIVE"]);
    let out = f
        .cmd()
        .args([
            "memory",
            "list",
            "--include-expired",
            "--include-superseded",
            "--json",
        ])
        .output()
        .expect("run");
    assert_eq!(json_of(&out).as_array().expect("array").len(), 3);
}

#[test]
fn a_private_memory_is_hidden_from_another_agent() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-PRIV",
            title: "Priv",
            scope: "private",
            owner: "alice",
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "list", "--json"])
        .output()
        .expect("run");
    assert!(ids_of(&json_of(&out)).is_empty(), "hidden from test-agent");
    let out = f
        .cmd()
        .args(["--owner", "alice", "memory", "list", "--json"])
        .output()
        .expect("run");
    assert_eq!(ids_of(&json_of(&out)), ["m-PRIV"], "visible to its owner");
}

#[test]
fn list_filters_by_min_importance() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-LOW",
            title: "Low",
            importance: 1,
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-HIGH",
            title: "High",
            importance: 5,
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "list", "--min-importance", "4", "--json"])
        .output()
        .expect("run");
    assert_eq!(ids_of(&json_of(&out)), ["m-HIGH"]);
}

#[test]
fn list_filters_by_kind_scope_owner_and_tags() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-A",
            title: "A",
            kind: "insight",
            tags: "[\"x\"]",
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-B",
            title: "B",
            owner: "alice",
            tags: "[\"x\", \"y\"]",
            ..Seed::default()
        },
    );
    let ids = |args: &[&str]| -> Vec<String> {
        let mut cmd = f.cmd();
        cmd.args(["memory", "list", "--json"]);
        cmd.args(args);
        ids_of(&json_of(&cmd.output().expect("run")))
    };
    assert_eq!(ids(&["--kind", "insight"]), ["m-A"]);
    assert_eq!(ids(&["--scope", "shared"]).len(), 2);
    assert_eq!(ids(&["--scope", "private"]).len(), 0);
    assert_eq!(ids(&["--owner", "alice"]), ["m-B"]);
    assert_eq!(ids(&["--tags", "x,y"]), ["m-B"]);
    assert_eq!(ids(&["--tags", "x,y", "--any-tag"]).len(), 2);
}

#[test]
fn list_mine_keeps_only_my_own_memories() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-MINE",
            title: "Mine",
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-THEIRS",
            title: "Theirs",
            owner: "alice",
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "list", "--mine", "--json"])
        .output()
        .expect("run");
    assert_eq!(ids_of(&json_of(&out)), ["m-MINE"]);
}

#[test]
fn list_sorts_by_importance_descending_then_created_ascending() {
    let f = VaultFixture::new();
    for (id, importance) in [("m-A", 1), ("m-B", 5), ("m-C", 3)] {
        seed(
            &f,
            Seed {
                id,
                title: id,
                importance,
                ..Seed::default()
            },
        );
    }
    let out = f
        .cmd()
        .args(["memory", "list", "--sort", "importance", "--json"])
        .output()
        .expect("run");
    assert_eq!(ids_of(&json_of(&out)), ["m-B", "m-C", "m-A"]);
}

#[test]
fn list_sorts_by_updated_created_and_title() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-OLD",
            title: "Zeta",
            updated: "2026-01-01T00:00:00Z".into(),
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-NEW",
            title: "Alpha",
            updated: "2026-06-01T00:00:00Z".into(),
            ..Seed::default()
        },
    );
    let ids = |sort: &str| -> Vec<String> {
        ids_of(&json_of(
            &f.cmd()
                .args(["memory", "list", "--sort", sort, "--json"])
                .output()
                .expect("run"),
        ))
    };
    assert_eq!(ids("updated"), ["m-NEW", "m-OLD"]);
    assert_eq!(ids("title"), ["m-NEW", "m-OLD"]);
    assert_eq!(ids("created").len(), 2);
}

#[test]
fn list_rejects_an_unknown_sort_key() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["memory", "list", "--sort", "bogus"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "invalid sort field: 'bogus' (use updated, created, title, importance)",
        ));
}

#[test]
fn list_limit_slices_and_a_negative_limit_is_unbounded() {
    let f = VaultFixture::new();
    for id in ["m-A", "m-B", "m-C"] {
        seed(
            &f,
            Seed {
                id,
                title: id,
                ..Seed::default()
            },
        );
    }
    let count = |limit: &str| -> usize {
        json_of(
            &f.cmd()
                .args(["memory", "list", "--json", limit])
                .output()
                .expect("run"),
        )
        .as_array()
        .expect("array")
        .len()
    };
    assert_eq!(count("--limit=2"), 2);
    assert_eq!(count("--limit=0"), 0);
    assert_eq!(count("--limit=-1"), 3);
}

#[test]
fn list_since_keeps_only_recently_updated_memories() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-FRESH",
            title: "Fresh",
            updated: days_ago(1),
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-STALE",
            title: "Stale",
            updated: days_ago(400),
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "list", "--since", "7d", "--json"])
        .output()
        .expect("run");
    assert_eq!(ids_of(&json_of(&out)), ["m-FRESH"]);
}

#[test]
fn an_empty_listing_is_an_empty_array() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["memory", "list", "--json"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), "[]\n");
}

// ---------------------------------------------------------------------------------------
// recall
// ---------------------------------------------------------------------------------------

#[test]
fn recall_always_emits_one_json_array_whatever_the_flags_say() {
    let f = VaultFixture::new();
    new_memory(&f, "Widget", "the widget is blue");
    for args in [
        vec!["memory", "recall", "widget"],
        vec!["memory", "recall", "widget", "--json"],
        vec!["memory", "recall", "widget", "--quiet"],
    ] {
        let out = f.cmd().args(&args).output().expect("run");
        let payload = json_of(&out);
        assert_eq!(payload.as_array().expect("array").len(), 1, "{args:?}");
    }
}

#[test]
fn a_recall_hit_carries_the_standard_keys_in_order() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-A",
            title: "Widget",
            body: "the widget is blue",
            tags: "[\"gear\"]",
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "recall", "widget"])
        .output()
        .expect("run");
    let payload = json_of(&out);
    let hit = &payload.as_array().expect("array")[0];
    let keys: Vec<&str> = hit
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["id", "type", "title", "score", "path", "tags", "owner", "updated", "snippet", "space"]
    );
    assert_eq!(hit["space"], Json::String("memories".into()));
    assert_eq!(hit["type"], Json::String("memory".into()));
}

#[test]
fn recall_ranks_importance_first_under_no_decay() {
    let f = VaultFixture::new();
    for (id, importance) in [("m-LOW", 1), ("m-HIGH", 5)] {
        seed(
            &f,
            Seed {
                id,
                title: "Widget notes",
                body: "the widget is blue",
                importance,
                ..Seed::default()
            },
        );
    }
    let out = f
        .cmd()
        .args(["memory", "recall", "widget", "--no-decay"])
        .output()
        .expect("run");
    let payload = json_of(&out);
    assert_eq!(ids_of(&payload), ["m-HIGH", "m-LOW"]);
    let hits = payload.as_array().expect("array");
    let high = hits[0]["score"].as_f64().expect("score");
    let low = hits[1]["score"].as_f64().expect("score");
    // Same match score and no decay: the ratio is the importance weights alone.
    assert!((high / low - 1.1 / 0.7).abs() < 1e-9, "{high} / {low}");
}

#[test]
fn recall_decay_prefers_the_more_recently_updated_memory() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-STALE",
            title: "Widget notes",
            body: "the widget is blue",
            updated: days_ago(720),
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-FRESH",
            title: "Widget notes",
            body: "the widget is blue",
            updated: days_ago(1),
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "recall", "widget"])
        .output()
        .expect("run");
    assert_eq!(ids_of(&json_of(&out)), ["m-FRESH", "m-STALE"]);
    // Without decay the two are tied on score and the tie breaks on `updated` descending.
    let out = f
        .cmd()
        .args(["memory", "recall", "widget", "--no-decay"])
        .output()
        .expect("run");
    let hits = json_of(&out);
    let scores: Vec<f64> = hits
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|h| h["score"].as_f64())
        .collect();
    assert!((scores[0] - scores[1]).abs() < 1e-12, "{scores:?}");
}

#[test]
fn recall_excludes_expired_and_superseded_memories() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-LIVE",
            title: "Live widget",
            body: "widget",
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-GONE",
            title: "Gone widget",
            body: "widget",
            expires: "2000-01-01T00:00:00Z",
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-OLD",
            title: "Old widget",
            body: "widget",
            superseded_by: "m-LIVE",
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "recall", "widget"])
        .output()
        .expect("run");
    assert_eq!(ids_of(&json_of(&out)), ["m-LIVE"]);
    let out = f
        .cmd()
        .args(["memory", "recall", "widget", "--include-expired"])
        .output()
        .expect("run");
    let mut ids = ids_of(&json_of(&out));
    ids.sort();
    assert_eq!(ids, ["m-GONE", "m-LIVE"], "supersession has no override");
}

#[test]
fn recall_hides_another_agents_private_memory() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-PRIV",
            title: "Private widget",
            body: "widget",
            scope: "private",
            owner: "alice",
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "recall", "widget"])
        .output()
        .expect("run");
    assert!(ids_of(&json_of(&out)).is_empty());
    let out = f
        .cmd()
        .args(["--owner", "alice", "memory", "recall", "widget"])
        .output()
        .expect("run");
    assert_eq!(ids_of(&json_of(&out)), ["m-PRIV"]);
}

#[test]
fn the_recall_threshold_applies_to_the_final_score() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-A",
            title: "Widget",
            body: "widget",
            importance: 1,
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "recall", "widget", "--no-decay"])
        .output()
        .expect("run");
    let score = json_of(&out).as_array().expect("array")[0]["score"]
        .as_f64()
        .expect("score");
    assert!(score < 1.0, "the importance weight lowers a title match");
    let out = f
        .cmd()
        .args([
            "memory",
            "recall",
            "widget",
            "--no-decay",
            "--threshold",
            &format!("{}", score + 0.05),
        ])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), "[]\n");
}

#[test]
fn recall_filters_by_kind_owner_mine_and_min_importance() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-A",
            title: "Widget A",
            body: "widget",
            kind: "insight",
            importance: 5,
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-B",
            title: "Widget B",
            body: "widget",
            owner: "alice",
            importance: 1,
            tags: "[\"gear\"]",
            ..Seed::default()
        },
    );
    let ids = |args: &[&str]| -> Vec<String> {
        let mut cmd = f.cmd();
        cmd.args(["memory", "recall", "widget"]);
        cmd.args(args);
        ids_of(&json_of(&cmd.output().expect("run")))
    };
    assert_eq!(ids(&["--kind", "insight"]), ["m-A"]);
    assert_eq!(ids(&["--owner", "alice"]), ["m-B"]);
    assert_eq!(ids(&["--min-importance", "4"]), ["m-A"]);
    assert_eq!(ids(&["--tags", "gear"]), ["m-B"]);
    assert_eq!(ids(&["--mine"]), ["m-A"]);
}

#[test]
fn recall_limit_caps_the_hit_array() {
    let f = VaultFixture::new();
    for id in ["m-A", "m-B", "m-C"] {
        seed(
            &f,
            Seed {
                id,
                title: id,
                body: "widget",
                ..Seed::default()
            },
        );
    }
    let out = f
        .cmd()
        .args(["memory", "recall", "widget", "--limit=2"])
        .output()
        .expect("run");
    assert_eq!(json_of(&out).as_array().expect("array").len(), 2);
}

#[test]
fn recall_meta_only_drops_the_snippet_and_beats_full() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-A",
            title: "Widget",
            body: "widget body",
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "recall", "widget", "--meta-only", "--full"])
        .output()
        .expect("run");
    let hit = json_of(&out).as_array().expect("array")[0].clone();
    assert!(hit.get("snippet").is_none(), "{hit}");
}

#[test]
fn recall_full_puts_the_whole_body_in_the_snippet() {
    let f = VaultFixture::new();
    let long = format!("widget {}", "y".repeat(400));
    seed(
        &f,
        Seed {
            id: "m-A",
            title: "Widget",
            body: &long,
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "recall", "widget", "--full"])
        .output()
        .expect("run");
    let hit = json_of(&out).as_array().expect("array")[0].clone();
    assert_eq!(hit["snippet"].as_str().expect("snippet").len(), long.len());
}

#[test]
fn a_recall_with_no_match_is_an_empty_array() {
    let f = VaultFixture::new();
    new_memory(&f, "Alpha", "nothing relevant");
    let out = f
        .cmd()
        .args(["memory", "recall", "zzzznomatch"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), "[]\n");
}

// ---------------------------------------------------------------------------------------
// forget
// ---------------------------------------------------------------------------------------

#[test]
fn forget_refuses_without_force_on_a_machine_path() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    f.cmd()
        .args(["memory", "forget", &id, "--json"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "refusing to delete on a non-interactive path; pass --force to confirm",
        ));
    assert!(f.files().iter().any(|p| p == &format!("memories/{id}.md")));
}

#[test]
fn forget_force_removes_the_file() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    f.cmd()
        .args(["memory", "forget", &id, "--force"])
        .assert()
        .success()
        .stdout(format!("deleted {id}\n"));
    assert!(!f.files().iter().any(|p| p == &format!("memories/{id}.md")));
}

#[test]
fn forget_json_is_id_then_deleted() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    let out = f
        .cmd()
        .args(["memory", "forget", &id, "--force", "--json"])
        .output()
        .expect("run");
    let payload = json_of(&out);
    let keys: Vec<&str> = payload
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["id", "deleted"]);
    assert_eq!(payload["deleted"], Json::Bool(true));
}

#[test]
fn forget_quiet_prints_the_id_alone() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "x");
    let out = f
        .cmd()
        .args(["memory", "forget", &id, "--force", "--quiet", "--json"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{id}\n"));
}

#[test]
fn forget_expired_removes_every_expired_memory() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-GONE",
            title: "Gone",
            expires: "2000-01-01T00:00:00Z",
            ..Seed::default()
        },
    );
    seed(
        &f,
        Seed {
            id: "m-LIVE",
            title: "Live",
            ..Seed::default()
        },
    );
    let out = f
        .cmd()
        .args(["memory", "forget", "--expired", "--force", "--json"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0), "{}", stderr_of(&out));
    let payload = json_of(&out);
    assert_eq!(ids_of(&payload), ["m-GONE"]);
    assert_eq!(
        payload.as_array().expect("array")[0]["deleted"],
        Json::Bool(true)
    );
    assert!(!f.files().iter().any(|p| p == "memories/m-GONE.md"));
    assert!(f.files().iter().any(|p| p == "memories/m-LIVE.md"));
}

#[test]
fn forget_expired_refuses_without_force_when_there_is_something_to_remove() {
    let f = VaultFixture::new();
    seed(
        &f,
        Seed {
            id: "m-GONE",
            title: "Gone",
            expires: "2000-01-01T00:00:00Z",
            ..Seed::default()
        },
    );
    f.cmd()
        .args(["memory", "forget", "--expired", "--json"])
        .assert()
        .code(2);
    assert!(f.files().iter().any(|p| p == "memories/m-GONE.md"));
}

#[test]
fn forget_expired_with_nothing_expired_is_a_silent_success() {
    let f = VaultFixture::new();
    seed(&f, Seed::default());
    let out = f
        .cmd()
        .args(["memory", "forget", "--expired", "--json"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0));
    assert_eq!(stdout_of(&out), "[]\n");
    assert!(f.files().iter().any(|p| p == "memories/m-SEED.md"));
}

#[test]
fn forget_with_neither_a_target_nor_expired_is_exit_two() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["memory", "forget", "--force"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "no target: pass a memory id or --expired",
        ));
}

#[test]
fn forget_of_a_missing_memory_is_exit_three() {
    let f = VaultFixture::new();
    f.cmd()
        .args(["memory", "forget", "m-GHOST", "--force"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("memory not found: m-GHOST"));
}

#[test]
fn forget_removes_a_corrupt_file_as_the_repair_path() {
    let f = VaultFixture::new();
    f.write(
        "memories/m-BAD.md",
        "---\nid: m-BAD\ntitle: [unclosed\n---\n\nbroken\n",
    );
    f.cmd()
        .args(["memory", "forget", "m-BAD", "--force"])
        .assert()
        .success();
    assert!(f.files().is_empty());
}

// ---------------------------------------------------------------------------------------
// cross-cutting
// ---------------------------------------------------------------------------------------

#[test]
fn json_is_accepted_on_either_side_of_the_command_name() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "widget");
    let pairs: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (
            vec!["memory", "get", &id, "--json"],
            vec!["--json", "memory", "get", &id],
        ),
        (
            vec!["memory", "list", "--json"],
            vec!["--json", "memory", "list"],
        ),
        (
            vec!["memory", "recall", "widget", "--json"],
            vec!["--json", "memory", "recall", "widget"],
        ),
    ];
    for (local, global) in pairs {
        let a = f.cmd().args(&local).output().expect("run");
        let b = f.cmd().args(&global).output().expect("run");
        assert_eq!(stdout_of(&a), stdout_of(&b), "{local:?} vs {global:?}");
    }
    // A mutation shape too, compared on its keys (the ids and stamps differ by design).
    let a = json_of(
        &f.cmd()
            .args(["memory", "new", "One", "--body", "x", "--json"])
            .output()
            .expect("run"),
    );
    let b = json_of(
        &f.cmd()
            .args(["--json", "memory", "new", "Two", "--body", "x"])
            .output()
            .expect("run"),
    );
    let keys = |v: &Json| -> Vec<String> {
        v.as_object()
            .expect("object")
            .keys()
            .map(String::to_string)
            .collect()
    };
    assert_eq!(keys(&a), keys(&b));
}

#[test]
fn a_disabled_memories_space_is_exit_two_on_every_verb() {
    let f = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n\
         [tasks]\ncollections = []\n\n[spaces]\nmemories = false\n",
    );
    for args in [
        vec!["memory", "new", "A", "--body", "x"],
        vec!["memory", "list"],
        vec!["memory", "recall", "q"],
        vec!["memory", "get", "m-A"],
        vec!["memory", "forget", "--expired", "--force"],
    ] {
        f.cmd()
            .args(&args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "space 'memories' is disabled in [spaces]",
            ));
    }
}

#[test]
fn read_verbs_never_rewrite_a_file() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Alpha", "widget body");
    let before = f.read(&format!("memories/{id}.md"));
    for args in [
        vec!["memory", "list"],
        vec!["memory", "list", "--json"],
        vec!["memory", "get", &id],
        vec!["memory", "get", &id, "--json"],
        vec!["memory", "recall", "widget"],
        vec!["memory", "recall", "widget", "--full"],
    ] {
        f.cmd().args(&args).assert().success();
        assert_eq!(f.read(&format!("memories/{id}.md")), before, "{args:?}");
    }
}

#[test]
fn help_lists_the_seven_subcommands_in_registration_order() {
    let f = VaultFixture::new();
    let out = f.cmd().args(["memory", "--help"]).output().expect("run");
    let text = stdout_of(&out);
    let mut at = 0usize;
    for name in ["new", "append", "update", "get", "list", "recall", "forget"] {
        let found = text[at..]
            .find(&format!("\n  {name} "))
            .unwrap_or_else(|| panic!("{name} not listed after position {at} in\n{text}"));
        at += found + 1;
    }
}

#[test]
fn the_bare_memory_command_prints_help_to_stdout_and_exits_two() {
    let f = VaultFixture::new();
    let out = f.cmd().args(["memory"]).output().expect("run");
    assert_eq!(code_of(&out), Some(2));
    assert!(stdout_of(&out).contains("recall"), "help goes to stdout");
    assert_eq!(stderr_of(&out), "");
}

#[test]
fn a_missing_config_is_the_three_line_message() {
    let f = VaultFixture::new();
    let out = f.bare_cmd().args(["memory", "list"]).output().expect("run");
    assert_eq!(code_of(&out), Some(2));
    assert!(stderr_of(&out).contains("mesh: no config found at"));
}

#[test]
fn a_foreign_markdown_file_is_invisible_to_every_memory_verb() {
    let f = VaultFixture::new();
    f.write("memories/loose.md", "# Loose\n\ntext\n");
    let out = f
        .cmd()
        .args(["memory", "list", "--json"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), "[]\n");
    for args in [
        vec!["memory", "get", "loose"],
        vec!["memory", "append", "loose", "x"],
        vec!["memory", "forget", "loose", "--force"],
    ] {
        f.cmd().args(&args).assert().code(3);
    }
    assert!(f.files().iter().any(|p| p == "memories/loose.md"));
}

#[test]
fn a_memory_filed_into_a_subfolder_is_read_and_never_moved() {
    let f = VaultFixture::new();
    f.write(
        "memories/personal/m-FILED.md",
        "---\nid: m-FILED\ntype: memory\ntitle: Filed\ntags: []\nowner: test-agent\n\
         created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated: []\n\
         kind: fact\nscope: shared\nimportance: 3\nsource: null\nexpires: null\n\
         superseded_by: null\n---\n\nbody\n",
    );
    f.cmd()
        .args(["memory", "get", "m-FILED", "--quiet"])
        .assert()
        .success()
        .stdout("m-FILED\n");
    f.cmd()
        .args(["memory", "update", "m-FILED", "--importance", "5"])
        .assert()
        .success();
    assert!(f
        .files()
        .iter()
        .any(|p| p == "memories/personal/m-FILED.md"));
    assert!(!f.files().iter().any(|p| p == "memories/m-FILED.md"));
}

#[test]
fn the_lock_directory_never_appears_in_a_listing() {
    let f = VaultFixture::new();
    new_memory(&f, "Alpha", "x");
    let out = f
        .cmd()
        .args(["memory", "list", "--json"])
        .output()
        .expect("run");
    assert_eq!(json_of(&out).as_array().expect("array").len(), 1);
    assert!(!stdout_of(&out).contains(".locks"));
}

#[test]
fn a_memory_is_searchable_through_the_shared_search_verb() {
    let f = VaultFixture::new();
    let id = new_memory(&f, "Widget", "the widget is blue");
    let out = f
        .cmd()
        .args(["search", "widget", "--space", "memories", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(ids_of(&json_of(&out)), [id]);
}
