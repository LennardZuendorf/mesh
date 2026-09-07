//! Regressions from the adversarial review of the merged rewrite.
//!
//! One test per confirmed finding, each named for the invariant it pins rather than for the
//! bug, so the file reads as a contract and not as a changelog.

mod common;

use std::process::Output;

use common::VaultFixture;
use serde_json::Value as Json;

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn run(f: &VaultFixture, args: &[&str]) -> (String, String, i32) {
    let out = f.cmd().args(args).output().expect("run mesh");
    (
        stdout_of(&out).trim_end().to_string(),
        stderr_of(&out).trim_end().to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn ok(f: &VaultFixture, args: &[&str]) -> String {
    let (stdout, stderr, code) = run(f, args);
    assert_eq!(code, 0, "args {args:?} -> {code}, stderr: {stderr}");
    stdout
}

/// A config whose named space is `false`.
fn disabled(space: &str) -> String {
    format!(
        "[core]\nvault_path = \"{{VAULT}}\"\nagent = \"test-agent\"\n\n\
         [tasks]\ncollections = []\n\n[spaces]\n{space} = false\n"
    )
}

// ---------------------------------------------------------------------------------------
// a disabled space fails validation on every verb, never degrades to an empty corpus
// ---------------------------------------------------------------------------------------

#[test]
fn a_disabled_tasks_space_fails_list_and_next_with_exit_2() {
    let f = VaultFixture::with(&disabled("tasks"));
    for args in [
        vec!["task", "list"],
        vec!["task", "next"],
        vec!["task", "get", "t-AAAA"],
    ] {
        let (_, stderr, code) = run(&f, &args);
        assert_eq!(code, 2, "args {args:?} -> {code}");
        assert_eq!(stderr, "space 'tasks' is disabled in [spaces]", "{args:?}");
    }
}

#[test]
fn a_disabled_space_reports_the_validation_envelope_under_json() {
    let f = VaultFixture::with(&disabled("tasks"));
    let (_, stderr, code) = run(&f, &["--json", "task", "list"]);
    assert_eq!(code, 2);
    let envelope: Json = serde_json::from_str(&stderr).expect("json envelope");
    assert_eq!(envelope["kind"], Json::String("validation".into()));
    assert_eq!(
        envelope["message"],
        Json::String("space 'tasks' is disabled in [spaces]".into())
    );
}

#[test]
fn a_disabled_notes_space_fails_list_and_get_with_exit_2() {
    let f = VaultFixture::with(&disabled("notes"));
    for args in [
        vec!["note", "list"],
        vec!["note", "get", "n-AAAA"],
        vec!["note", "delete", "n-AAAA"],
    ] {
        let (_, stderr, code) = run(&f, &args);
        assert_eq!(code, 2, "args {args:?} -> {code}");
        assert_eq!(stderr, "space 'notes' is disabled in [spaces]", "{args:?}");
    }
}

// ---------------------------------------------------------------------------------------
// the global --owner is folded into every listing's owner filter
// ---------------------------------------------------------------------------------------

#[test]
fn the_global_owner_filters_note_task_and_asset_listings() {
    let f = VaultFixture::new();
    ok(&f, &["note", "new", "A", "--owner", "alice", "--body", "b"]);
    let mine = ok(
        &f,
        &[
            "--quiet", "note", "new", "B", "--owner", "bob", "--body", "b",
        ],
    );
    assert_eq!(ok(&f, &["--quiet", "--owner", "bob", "note", "list"]), mine);
    assert_eq!(
        ok(&f, &["--quiet", "note", "list", "--owner", "bob"]),
        mine,
        "local and global must agree"
    );

    ok(&f, &["task", "new", "A", "--owner", "alice"]);
    let t = ok(&f, &["--quiet", "task", "new", "B", "--owner", "bob"]);
    assert_eq!(ok(&f, &["--quiet", "--owner", "bob", "task", "list"]), t);
}

// ---------------------------------------------------------------------------------------
// the lock path is the sandbox boundary for the lock file itself
// ---------------------------------------------------------------------------------------

#[test]
fn a_traversal_id_never_touches_a_lock_outside_the_space() {
    let f = VaultFixture::new();
    ok(&f, &["task", "new", "seed"]);
    let victim = f.dir.path().join("victim");
    std::fs::create_dir_all(&victim).expect("victim dir");
    let keep = victim.join("keep.lock");
    std::fs::write(&keep, "999999\n").expect("seed lock");

    for verb in ["append", "release", "delete"] {
        let mut args = vec!["task", verb, "../../victim/keep"];
        if verb == "append" {
            args.push("x");
        }
        if verb == "delete" {
            args.push("--force");
        }
        let (_, stderr, code) = run(&f, &args);
        assert_eq!(code, 2, "{verb} -> {code}, stderr {stderr}");
        assert!(stderr.starts_with("path escapes sandbox "), "{stderr}");
    }
    assert!(keep.is_file(), "the outside lock file was unlinked");
    assert!(
        !f.dir.path().join("victim/keep.lock.lock").exists(),
        "a lock was created outside the vault"
    );
}

// ---------------------------------------------------------------------------------------
// frontmatter round-trips: what is written plain must read back as a string
// ---------------------------------------------------------------------------------------

#[test]
fn a_hex_looking_title_survives_the_write_read_round_trip() {
    let f = VaultFixture::new();
    for (space, extra) in [
        ("note", vec!["--body", "b"]),
        ("memory", vec!["--body", "b"]),
    ] {
        let mut args = vec![space, "new", "0x1F"];
        args.extend(extra);
        args.insert(0, "--quiet");
        let id = ok(&f, &args);
        let got = ok(&f, &["--json", space, "get", &id]);
        let payload: Json = serde_json::from_str(&got).expect("json");
        assert_eq!(payload["title"], Json::String("0x1F".into()), "{space}");
    }
    let id = ok(&f, &["--quiet", "task", "new", "0x1F"]);
    assert!(!ok(&f, &["--quiet", "task", "get", &id]).is_empty());
}

#[test]
fn a_hex_looking_tag_stays_a_string_on_the_json_surface() {
    let f = VaultFixture::new();
    let id = ok(
        &f,
        &[
            "--quiet", "note", "new", "HexTag", "--body", "b", "--tags", "0x1F",
        ],
    );
    let payload: Json =
        serde_json::from_str(&ok(&f, &["--json", "note", "get", &id])).expect("json");
    assert_eq!(payload["tags"], serde_json::json!(["0x1F"]));
}

// ---------------------------------------------------------------------------------------
// an asset blob never collides with its own sidecar
// ---------------------------------------------------------------------------------------

#[test]
fn adding_a_markdown_file_keeps_the_ingested_bytes() {
    let f = VaultFixture::new();
    let src = f.dir.path().join("x.md");
    std::fs::write(&src, "HELLO").expect("write source");
    let id = ok(
        &f,
        &["--quiet", "asset", "add", src.to_str().expect("utf8")],
    );

    let blob = ok(&f, &["asset", "path", &id]);
    assert_ne!(
        blob,
        f.vault
            .join(format!("assets/{id}.md"))
            .display()
            .to_string(),
        "the blob must not be the sidecar"
    );
    assert_eq!(std::fs::read_to_string(&blob).expect("read blob"), "HELLO");
    let payload: Json =
        serde_json::from_str(&ok(&f, &["--json", "asset", "get", &id])).expect("json");
    assert_eq!(payload["media_type"], Json::String("text/markdown".into()));
    assert_eq!(payload["bytes"], serde_json::json!(5));
    // gc sees a matched pair, not an orphan on either side.
    let report: Json = serde_json::from_str(&ok(&f, &["--json", "asset", "gc"])).expect("json");
    assert_eq!(report["orphan_blobs"], serde_json::json!([]));
    assert_eq!(report["orphan_sidecars"], serde_json::json!([]));
}

// ---------------------------------------------------------------------------------------
// a mirror write never rewrites a value it could not parse
// ---------------------------------------------------------------------------------------

#[test]
fn a_mirror_refuses_a_task_whose_edge_key_is_not_a_list() {
    let f = VaultFixture::new();
    let blocker = ok(&f, &["--quiet", "task", "new", "B"]);
    let other = ok(&f, &["--quiet", "task", "new", "Other"]);
    let subject = ok(&f, &["--quiet", "task", "new", "T"]);

    let rel = format!("tasks/open/{blocker}.md");
    let text = f
        .read(&rel)
        .replace("blocks: []", &format!("blocks: {other}"));
    f.write(&rel, &text);

    let (_, stderr, code) = run(&f, &["task", "block", &subject, "--on", &blocker]);
    assert_eq!(code, 0, "the subject's own write still succeeds");
    assert!(
        stderr.contains("corrupt"),
        "expected a corrupt warning: {stderr}"
    );
    // The unparseable value is untouched — mesh owns the interface, not the data.
    assert!(
        f.read(&rel).contains(&format!("blocks: {other}")),
        "{}",
        f.read(&rel)
    );
}

// ---------------------------------------------------------------------------------------
// every write normalises key order
// ---------------------------------------------------------------------------------------

#[test]
fn a_rewrite_restores_declaration_key_order_and_keeps_unknown_keys() {
    let f = VaultFixture::new();
    let id = ok(&f, &["--quiet", "note", "new", "Ordered", "--body", "b"]);
    let rel = format!("notes/{id}.md");
    f.write(
        &rel,
        &format!(
            "---\ncreated: 2024-01-01T00:00:00Z\nextra: keep\nid: {id}\ntags: []\n\
             title: Ordered\ntype: note\nupdated: 2024-01-01T00:00:00Z\n---\n\nb\n"
        ),
    );
    ok(&f, &["note", "append", &id, "more"]);
    let text = f.read(&rel);
    assert!(
        text.starts_with(&format!("---\nid: {id}\ntype: note\ntitle: Ordered\n")),
        "{text}"
    );
    assert!(text.contains("extra: keep"), "unknown key dropped: {text}");
    let head = text.split("\n---").next().unwrap_or_default();
    assert!(
        head.find("extra:") > head.find("updated:"),
        "unknown keys must land last: {text}"
    );
}
