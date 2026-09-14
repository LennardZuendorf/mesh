//! `mesh note` end to end: every subcommand, every output mode, every error path.
//!
//! Each test drives the real binary through `VaultFixture`, so nothing here touches the
//! process environment and the file is safe to run at default parallelism.

mod common;

use std::path::Path;
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

/// Create a note and return its id.
fn new_note(fixture: &VaultFixture, title: &str, body: &str) -> String {
    let out = fixture
        .cmd()
        .args(["note", "new", title, "--body", body, "--quiet"])
        .output()
        .expect("note new");
    assert_eq!(code_of(&out), Some(0), "{}", stderr_of(&out));
    stdout_of(&out).trim().to_string()
}

fn json_of(out: &Output) -> Json {
    serde_json::from_str(&stdout_of(out)).expect("stdout is one JSON line")
}

/// `N1=n-6YQY N2=…` — the corpus ids, read from the frozen fixture manifest.
fn corpus_id(key: &str) -> String {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ids.txt");
    let text = std::fs::read_to_string(manifest).expect("ids.txt");
    let prefix = format!("{key}=");
    text.split_whitespace()
        .find_map(|pair| pair.strip_prefix(&prefix))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no id named {key} in ids.txt"))
}

/// A golden payload, re-serialised with our compact separators so only key order and values
/// are being compared.
fn golden(name: &str) -> Json {
    let text = std::fs::read_to_string(common::golden_dir().join(name)).expect("golden file");
    serde_json::from_str(&text).expect("golden json")
}

/// The one documented reading difference from the Python era: `extra: {nested: yes}` is a
/// string under YAML 1.2, where PyYAML's 1.1 resolution made it a bool (foundation
/// deviation 11). Patch the golden so every other byte still has to match.
fn patch_yaml11(value: &mut Json) {
    match value {
        Json::Array(items) => items.iter_mut().for_each(patch_yaml11),
        Json::Object(map) => {
            if let Some(Json::Object(extra)) = map.get_mut("extra") {
                if extra.get("nested") == Some(&Json::Bool(true)) {
                    extra.insert("nested".to_string(), Json::String("yes".to_string()));
                }
            }
        }
        _ => {}
    }
}

fn compact(value: &Json) -> String {
    serde_json::to_string(value).expect("serialise")
}

// ---------------------------------------------------------------------------------------
// note new
// ---------------------------------------------------------------------------------------

#[test]
fn new_writes_a_file_and_reports_the_id() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["note", "new", "Alpha", "--body", "hello"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0));
    let id = stdout_of(&out)
        .trim()
        .strip_prefix("created ")
        .expect("created line")
        .to_string();
    assert!(id.starts_with("n-"), "{id}");
    let text = f.read(&format!("notes/{id}.md"));
    assert!(text.starts_with("---\nid: "), "{text}");
    assert!(text.ends_with("hello\n"), "{text}");
}

#[test]
fn new_writes_the_eight_keys_in_declaration_order() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "body");
    let text = f.read(&format!("notes/{id}.md"));
    let keys: Vec<&str> = text
        .lines()
        .skip(1)
        .take_while(|l| *l != "---")
        .filter(|l| !l.starts_with(' ') && !l.starts_with('-'))
        .filter_map(|l| l.split(':').next())
        .collect();
    assert_eq!(
        keys,
        ["id", "type", "title", "tags", "owner", "created", "updated", "related"]
    );
}

#[test]
fn new_routes_every_type_into_its_folder() {
    let f = VaultFixture::new();
    for (note_type, folder) in [
        ("note", "notes"),
        ("log", "notes/logs"),
        ("decision", "notes/decisions"),
        ("reference", "notes/references"),
        ("project", "notes/projects"),
    ] {
        let out = f
            .cmd()
            .args([
                "note",
                "new",
                &format!("T {note_type}"),
                "--type",
                note_type,
                "--body",
                "x",
                "--quiet",
            ])
            .output()
            .expect("run");
        assert_eq!(code_of(&out), Some(0), "{note_type}");
        let id = stdout_of(&out).trim().to_string();
        assert!(
            f.files().contains(&format!("{folder}/{id}.md")),
            "{note_type} landed in {:?}",
            f.files()
        );
    }
}

#[test]
fn new_json_and_quiet_have_the_documented_shapes() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args([
            "note", "new", "Alpha", "--type", "log", "--body", "x", "--json",
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
    assert_eq!(keys, ["id", "type", "updated"]);
    assert_eq!(payload["type"], Json::String("log".into()));
    assert!(payload["updated"].as_str().expect("ts").ends_with('Z'));

    let out = f
        .cmd()
        .args(["note", "new", "Beta", "--body", "x", "--quiet"])
        .output()
        .expect("run");
    assert!(stdout_of(&out).trim().starts_with("n-"));
    assert!(!stdout_of(&out).contains("created"));
}

#[test]
fn new_quiet_beats_json_on_a_mutation() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["note", "new", "Alpha", "--body", "x", "--json", "--quiet"])
        .output()
        .expect("run");
    assert!(!stdout_of(&out).contains('{'), "{}", stdout_of(&out));
    assert!(stdout_of(&out).trim().starts_with("n-"));
}

#[test]
fn new_takes_the_body_from_a_file_and_body_wins_over_it() {
    let f = VaultFixture::new();
    let body_file = f.dir.path().join("body.md");
    std::fs::write(&body_file, "from the file").expect("write");

    let id = {
        let out = f
            .cmd()
            .args(["note", "new", "FromFile", "--quiet"])
            .arg("--file")
            .arg(&body_file)
            .output()
            .expect("run");
        assert_eq!(code_of(&out), Some(0));
        stdout_of(&out).trim().to_string()
    };
    assert!(f
        .read(&format!("notes/{id}.md"))
        .ends_with("from the file\n"));

    let out = f
        .cmd()
        .args(["note", "new", "BodyWins", "--quiet", "--body", "inline"])
        .arg("--file")
        .arg(&body_file)
        .output()
        .expect("run");
    let id = stdout_of(&out).trim().to_string();
    assert!(f.read(&format!("notes/{id}.md")).ends_with("inline\n"));
}

#[test]
fn new_rejects_an_unreadable_file_and_a_missing_body() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["note", "new", "T", "--file", "/nope/missing.md"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(2));
    assert!(
        stderr_of(&out).starts_with("cannot read --file /nope/missing.md: "),
        "{}",
        stderr_of(&out)
    );

    let out = f.cmd().args(["note", "new", "T"]).output().expect("run");
    assert_eq!(code_of(&out), Some(2));
    assert_eq!(
        stderr_of(&out).trim(),
        "no body: pass --body or --file on a non-interactive path"
    );
    assert!(f.files().is_empty(), "nothing was written");
}

#[test]
fn new_rejects_an_unknown_type_and_writes_nothing() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["note", "new", "T", "--type", "memo", "--body", "x"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(2));
    assert_eq!(stderr_of(&out).trim(), "invalid note type: memo");
    assert!(f.files().is_empty());
}

#[test]
fn new_parses_tags_as_plain_csv() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args([
            "note",
            "new",
            "T",
            "--body",
            "x",
            "--tags",
            " a , b ,, a ",
            "--quiet",
        ])
        .output()
        .expect("run");
    let id = stdout_of(&out).trim().to_string();
    let text = f.read(&format!("notes/{id}.md"));
    assert!(text.contains("tags:\n  - a\n  - b\n"), "{text}");
}

#[test]
fn new_records_the_owner_and_honours_the_roster() {
    let f = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n\
         [tasks]\ncollections = [\"alice\", \"test-agent\"]\n",
    );
    let out = f
        .cmd()
        .args([
            "note", "new", "T", "--body", "x", "--owner", "alice", "--quiet",
        ])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0));
    let id = stdout_of(&out).trim().to_string();
    assert!(f.read(&format!("notes/{id}.md")).contains("owner: alice\n"));

    let out = f
        .cmd()
        .args(["note", "new", "T2", "--body", "x", "--owner", "ghost"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(2));
    assert_eq!(stderr_of(&out).trim(), "unknown owner: 'ghost'");
}

#[test]
fn new_defaults_the_owner_to_the_configured_agent() {
    let f = VaultFixture::new();
    let id = new_note(&f, "T", "x");
    assert!(f
        .read(&format!("notes/{id}.md"))
        .contains("owner: test-agent\n"));
}

#[test]
fn new_warns_about_a_duplicate_title_on_stderr_only() {
    let f = VaultFixture::new();
    let first = new_note(&f, "Japan Visa", "x");

    // Slug-normalised: case and punctuation differences still collide.
    let out = f
        .cmd()
        .args(["note", "new", "  japan   visa! ", "--body", "x"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0), "the create still succeeds");
    assert_eq!(
        stderr_of(&out).trim(),
        format!("note new: duplicate title, also used by {first}")
    );

    // Still emitted under --json, and never inside the payload.
    let out = f
        .cmd()
        .args(["note", "new", "japan visa", "--body", "x", "--json"])
        .output()
        .expect("run");
    assert!(stderr_of(&out).contains("duplicate title"));
    assert!(!stdout_of(&out).contains("duplicate"));

    // Suppressed by --quiet.
    let out = f
        .cmd()
        .args(["note", "new", "japan visa", "--body", "x", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stderr_of(&out), "");
}

#[test]
fn new_derives_related_from_wikilinks() {
    let f = VaultFixture::new();
    let target = new_note(&f, "Alpha", "x");
    let linker = new_note(
        &f,
        "Linker",
        "see [[Alpha]], [[Alpha#Section|alias]], [[n-9999]] and [[Ghost Title]]",
    );
    let text = f.read(&format!("notes/{linker}.md"));
    assert!(
        text.contains(&format!("related:\n  - {target}\n  - n-9999\n")),
        "{text}"
    );
}

// ---------------------------------------------------------------------------------------
// note append
// ---------------------------------------------------------------------------------------

#[test]
fn append_adds_a_block_and_bumps_updated() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "first");
    let before = f.read(&format!("notes/{id}.md"));

    let out = f
        .cmd()
        .args(["note", "append", &id, "second"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0));
    assert_eq!(stdout_of(&out).trim(), format!("appended {id}"));

    let after = f.read(&format!("notes/{id}.md"));
    assert!(after.ends_with("first\n\nsecond\n"), "{after}");
    let created = |t: &str| {
        t.lines()
            .find(|l| l.starts_with("created:"))
            .expect("created")
            .to_string()
    };
    assert_eq!(created(&before), created(&after), "created is untouched");
    assert_ne!(before, after);
}

#[test]
fn append_under_a_section_creates_it_when_absent() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "Intro.\n\n## A\n\nitem1\n\n## B\n\nitem2");
    f.cmd()
        .args(["note", "append", &id, "NEW", "--section", "A", "--quiet"])
        .assert()
        .success();
    let text = f.read(&format!("notes/{id}.md"));
    assert!(
        text.ends_with("Intro.\n\n## A\n\nitem1\n\nNEW\n\n## B\n\nitem2\n"),
        "{text}"
    );

    f.cmd()
        .args(["note", "append", &id, "TAIL", "--section", "Zed", "--quiet"])
        .assert()
        .success();
    assert!(f
        .read(&format!("notes/{id}.md"))
        .ends_with("## Zed\n\nTAIL\n"));
}

#[test]
fn append_timestamp_stamps_the_body_with_the_acting_agent() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "first");
    f.cmd()
        .args(["note", "append", &id, "line", "--timestamp", "--quiet"])
        .assert()
        .success();
    let text = f.read(&format!("notes/{id}.md"));
    assert!(text.contains(" — test-agent\nline\n"), "{text}");

    // The global --owner is the actor, not the note's owner.
    f.cmd()
        .args(["--owner", "other-agent", "note", "append", &id, "again"])
        .args(["--timestamp", "--quiet"])
        .assert()
        .success();
    let text = f.read(&format!("notes/{id}.md"));
    assert!(text.contains(" — other-agent\nagain\n"), "{text}");
    assert!(text.contains("owner: test-agent\n"), "owner is unchanged");
}

#[test]
fn append_recomputes_related_wholesale() {
    let f = VaultFixture::new();
    let target = new_note(&f, "Alpha", "x");
    let id = new_note(&f, "Linker", "no links yet");
    assert!(f.read(&format!("notes/{id}.md")).contains("related: []"));
    f.cmd()
        .args(["note", "append", &id, "now [[Alpha]]", "--quiet"])
        .assert()
        .success();
    assert!(f
        .read(&format!("notes/{id}.md"))
        .contains(&format!("related:\n  - {target}\n")));
}

#[test]
fn append_reports_a_missing_or_corrupt_note_as_not_found() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["note", "append", "n-NOPE", "x"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(3));
    assert_eq!(stderr_of(&out).trim(), "note not found: n-NOPE");

    f.write(
        "notes/n-BAD.md",
        "---\nid: n-BAD\ntitle: [unclosed\n---\n\nbroken\n",
    );
    let before = f.read("notes/n-BAD.md");
    let out = f
        .cmd()
        .args(["note", "append", "n-BAD", "x"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(3));
    assert_eq!(f.read("notes/n-BAD.md"), before, "the file is untouched");
}

#[test]
fn append_is_addressable_by_slug() {
    let f = VaultFixture::new();
    let id = new_note(&f, "My Long Title", "first");
    f.cmd()
        .args(["note", "append", "my-long-title", "second", "--quiet"])
        .assert()
        .success()
        .stdout(format!("{id}\n"));
}

// ---------------------------------------------------------------------------------------
// note update
// ---------------------------------------------------------------------------------------

#[test]
fn update_applies_the_whole_tag_grammar() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "x");
    let tags_of = || {
        let text = f.read(&format!("notes/{id}.md"));
        text.lines()
            .skip_while(|l| !l.starts_with("tags:"))
            .skip(1)
            .take_while(|l| l.starts_with("  - "))
            .map(|l| l.trim_start_matches("  - ").to_string())
            .collect::<Vec<String>>()
    };
    for (spec, expected) in [
        ("a,b", vec!["a", "b"]),
        ("a,c", vec!["a", "b", "c"]),
        ("+d,-a", vec!["b", "c", "d"]),
        ("=x,y", vec!["x", "y"]),
    ] {
        f.cmd()
            .args(["note", "update", &id, "--tags", spec, "--quiet"])
            .assert()
            .success();
        assert_eq!(tags_of(), expected, "spec {spec}");
    }
}

#[test]
fn update_rejects_a_mixed_tag_spec_without_writing() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "x");
    f.cmd()
        .args(["note", "update", &id, "--tags", "a,b", "--quiet"])
        .assert()
        .success();
    let before = f.read(&format!("notes/{id}.md"));

    let out = f
        .cmd()
        .args(["note", "update", &id, "--tags", "+c,d"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(2));
    assert!(
        stderr_of(&out).starts_with("ambiguous tag spec '+c,d': mixes prefixed (+/-)"),
        "{}",
        stderr_of(&out)
    );
    assert_eq!(f.read(&format!("notes/{id}.md")), before);
}

#[test]
fn update_type_moves_the_file_and_keeps_the_filename() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "x");
    let out = f
        .cmd()
        .args(["note", "update", &id, "--type", "decision", "--json"])
        .output()
        .expect("run");
    assert_eq!(json_of(&out)["type"], Json::String("decision".into()));
    let files = f.files();
    assert!(
        files.contains(&format!("notes/decisions/{id}.md")),
        "{files:?}"
    );
    assert!(!files.contains(&format!("notes/{id}.md")));
}

#[test]
fn update_rejects_an_unknown_type_without_touching_the_file() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "x");
    let before = f.read(&format!("notes/{id}.md"));
    let out = f
        .cmd()
        .args(["note", "update", &id, "--type", "memo"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(2));
    assert_eq!(stderr_of(&out).trim(), "invalid note type: memo");
    assert_eq!(f.read(&format!("notes/{id}.md")), before);
}

#[test]
fn update_title_rewrites_the_title_and_warns_about_dangling_links() {
    let f = VaultFixture::new();
    let target = new_note(&f, "Old Title", "x");
    let linker = new_note(&f, "Linker", "see [[Old Title]]");

    let out = f
        .cmd()
        .args(["note", "update", &target, "--title", "New Title"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0));
    assert_eq!(
        stderr_of(&out).trim(),
        format!("note update: renaming dangles 1 title link(s) in {linker}")
    );
    assert!(f
        .read(&format!("notes/{target}.md"))
        .contains("title: New Title\n"));

    // Advisory only: it never enters the payload and --quiet silences it.
    let out = f
        .cmd()
        .args([
            "note",
            "update",
            &target,
            "--title",
            "Third Title",
            "--json",
        ])
        .output()
        .expect("run");
    assert!(!stdout_of(&out).contains("dangles"));
    let out = f
        .cmd()
        .args(["note", "update", &target, "--title", "Fourth", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stderr_of(&out), "");
}

#[test]
fn update_reports_the_new_type_and_bumps_updated() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "x");
    let before = f.read(&format!("notes/{id}.md"));
    let out = f
        .cmd()
        .args(["note", "update", &id, "--tags", "a"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out).trim(), format!("updated {id}"));
    assert_ne!(f.read(&format!("notes/{id}.md")), before);
}

// ---------------------------------------------------------------------------------------
// note get
// ---------------------------------------------------------------------------------------

#[test]
fn get_prints_eight_meta_lines_then_a_blank_line_then_the_preview() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "Hello body");
    let out = f.cmd().args(["note", "get", &id]).output().expect("run");
    let text = stdout_of(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 10, "{text}");
    assert_eq!(lines[0], format!("id: {id}"));
    assert_eq!(lines[1], "type: note");
    assert_eq!(lines[2], "title: Alpha");
    assert_eq!(lines[3], "tags: ");
    assert_eq!(lines[4], "owner: test-agent");
    assert!(lines[5].starts_with("created: ") && lines[5].ends_with('Z'));
    assert!(lines[6].starts_with("updated: ") && lines[6].ends_with('Z'));
    assert_eq!(lines[7], "related: ");
    assert_eq!(lines[8], "");
    assert_eq!(lines[9], "Hello body");
}

#[test]
fn get_truncates_the_preview_at_two_hundred_code_points() {
    let f = VaultFixture::new();
    let body = "é".repeat(300);
    let id = new_note(&f, "Long", &body);

    let out = f.cmd().args(["note", "get", &id]).output().expect("run");
    let preview = stdout_of(&out)
        .lines()
        .next_back()
        .expect("body line")
        .to_string();
    assert_eq!(preview.chars().count(), 200);

    let out = f
        .cmd()
        .args(["note", "get", &id, "--full"])
        .output()
        .expect("run");
    let full = stdout_of(&out)
        .lines()
        .next_back()
        .expect("body line")
        .to_string();
    assert_eq!(full.chars().count(), 300);
}

#[test]
fn get_meta_only_drops_the_body_from_both_surfaces() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "Hello body");
    let out = f
        .cmd()
        .args(["note", "get", &id, "--meta-only"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out).lines().count(), 8);
    assert!(!stdout_of(&out).contains("Hello body"));

    let out = f
        .cmd()
        .args(["note", "get", &id, "--meta-only", "--json"])
        .output()
        .expect("run");
    assert!(json_of(&out).get("body").is_none());
}

#[test]
fn get_json_puts_body_last_after_the_frontmatter() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "Hello body");
    let out = f
        .cmd()
        .args(["note", "get", &id, "--json"])
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
        ["id", "type", "title", "tags", "owner", "created", "updated", "related", "body"]
    );
    assert_eq!(payload["body"], Json::String("Hello body".into()));
    assert!(stdout_of(&out).ends_with("}\n"));
    assert!(!stdout_of(&out).contains(", "), "compact separators");
}

#[test]
fn get_related_prints_ids_or_a_related_object() {
    let f = VaultFixture::new();
    let target = new_note(&f, "Alpha", "x");
    let id = new_note(&f, "Linker", "see [[Alpha]]");

    f.cmd()
        .args(["note", "get", &id, "--related"])
        .assert()
        .success()
        .stdout(format!("{target}\n"));
    f.cmd()
        .args(["note", "get", &id, "--related", "--json"])
        .assert()
        .success()
        .stdout(format!("{{\"related\":[\"{target}\"]}}\n"));

    // An empty list prints an empty line.
    f.cmd()
        .args(["note", "get", &target, "--related"])
        .assert()
        .success()
        .stdout("\n");
}

#[test]
fn get_quiet_returns_the_id_and_beats_related() {
    let f = VaultFixture::new();
    let target = new_note(&f, "Alpha", "x");
    let id = new_note(&f, "Linker", "see [[Alpha]]");
    f.cmd()
        .args(["note", "get", &id, "--quiet"])
        .assert()
        .success()
        .stdout(format!("{id}\n"));
    f.cmd()
        .args(["note", "get", &id, "--related", "--quiet"])
        .assert()
        .success()
        .stdout(format!("{id}\n"));
    assert!(!target.is_empty());
}

#[test]
fn get_resolves_a_slug_and_reports_ambiguity_with_sorted_ids() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Only One", "x");
    f.cmd()
        .args(["note", "get", "only-one", "--quiet"])
        .assert()
        .success()
        .stdout(format!("{id}\n"));

    let a = new_note(&f, "Twin", "x");
    let b = new_note(&f, "twin", "x");
    let mut ids = [a, b];
    ids.sort();
    let out = f.cmd().args(["note", "get", "twin"]).output().expect("run");
    assert_eq!(code_of(&out), Some(2));
    assert_eq!(
        stderr_of(&out).trim(),
        format!("ambiguous slug 'twin': {}, {}", ids[0], ids[1])
    );
}

#[test]
fn a_not_found_envelope_carries_candidates() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Japan Visa", "x");
    let out = f
        .cmd()
        .args(["--json", "note", "get", "japan-visas"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(3));
    let payload: Json = serde_json::from_str(&stderr_of(&out)).expect("envelope");
    let keys: Vec<&str> = payload
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["kind", "message", "next_action", "id_or_slug", "candidates"]
    );
    assert_eq!(payload["kind"], Json::String("not_found".into()));
    assert_eq!(payload["candidates"], serde_json::json!([id]));
    assert_eq!(stdout_of(&out), "");
}

#[test]
fn get_reports_a_corrupt_note_as_not_found() {
    let f = VaultFixture::new();
    f.write(
        "notes/n-BAD.md",
        "---\nid: n-BAD\ntitle: [unclosed\n---\n\nbroken\n",
    );
    let out = f
        .cmd()
        .args(["note", "get", "n-BAD"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(3));
    assert_eq!(stderr_of(&out).trim(), "note not found: n-BAD");
}

#[test]
fn get_foreign_reads_a_non_mesh_file_by_stem_or_path() {
    let f = VaultFixture::new();
    f.write("notes/loose.md", "# Loose Heading\n\nsome text\n");

    let out = f
        .cmd()
        .args(["note", "get", "loose"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(3), "invisible without --foreign");

    for target in ["loose", "loose.md", "notes/loose.md"] {
        let out = f
            .cmd()
            .args(["note", "get", target, "--foreign", "--json"])
            .output()
            .expect("run");
        assert_eq!(code_of(&out), Some(0), "{target}");
        let payload = json_of(&out);
        let keys: Vec<&str> = payload
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["id", "type", "title", "body", "path"]);
        assert!(payload["id"].is_null() && payload["type"].is_null());
        assert_eq!(payload["title"], Json::String("Loose Heading".into()));
        assert_eq!(
            payload["body"],
            Json::String("# Loose Heading\n\nsome text".into())
        );
    }
}

#[test]
fn get_foreign_falls_back_to_the_filename_stem() {
    let f = VaultFixture::new();
    f.write("notes/plain.md", "no heading here\n");
    let out = f
        .cmd()
        .args(["note", "get", "plain", "--foreign", "--json"])
        .output()
        .expect("run");
    assert_eq!(json_of(&out)["title"], Json::String("plain".into()));
}

#[test]
fn get_foreign_still_prefers_a_real_note() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "mesh body");
    f.write("notes/loose.md", "# Loose\n");
    let out = f
        .cmd()
        .args(["note", "get", &id, "--foreign", "--json"])
        .output()
        .expect("run");
    assert_eq!(json_of(&out)["id"], Json::String(id));
}

// ---------------------------------------------------------------------------------------
// note list
// ---------------------------------------------------------------------------------------

#[test]
fn list_rows_are_id_type_title_separated_by_two_spaces() {
    let f = VaultFixture::new();
    let alpha = new_note(&f, "Alpha", "x");
    f.cmd()
        .args(["note", "update", &alpha, "--type", "log", "--quiet"])
        .assert()
        .success();
    let out = f.cmd().args(["note", "list"]).output().expect("run");
    assert_eq!(stdout_of(&out), format!("{alpha}  log  Alpha\n"));
}

#[test]
fn list_json_beats_quiet_and_quiet_prints_ids() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "x");
    f.cmd()
        .args(["note", "list", "--quiet"])
        .assert()
        .success()
        .stdout(format!("{id}\n"));
    let out = f
        .cmd()
        .args(["note", "list", "--json", "--quiet"])
        .output()
        .expect("run");
    assert!(stdout_of(&out).starts_with('['), "{}", stdout_of(&out));
}

#[test]
fn list_filters_are_conjunctive() {
    let f = VaultFixture::new();
    let a = {
        let out = f
            .cmd()
            .args([
                "note", "new", "A", "--body", "x", "--tags", "x,y", "--quiet",
            ])
            .output()
            .expect("run");
        stdout_of(&out).trim().to_string()
    };
    let b = {
        let out = f
            .cmd()
            .args(["note", "new", "B", "--body", "x", "--tags", "x"])
            .args(["--type", "log", "--owner", "other", "--quiet"])
            .output()
            .expect("run");
        stdout_of(&out).trim().to_string()
    };

    let ids = |args: &[&str]| {
        let mut cmd = f.cmd();
        cmd.args(["note", "list", "--quiet"]);
        cmd.args(args);
        let out = cmd.output().expect("run");
        stdout_of(&out)
            .lines()
            .map(str::to_string)
            .collect::<Vec<String>>()
    };
    assert_eq!(ids(&["--tags", "x,y"]), vec![a.clone()]);
    assert_eq!(ids(&["--tags", "x,y", "--any-tag"]).len(), 2);
    assert_eq!(ids(&["--type", "log"]), vec![b.clone()]);
    assert_eq!(ids(&["--owner", "other"]), [b]);
    assert_eq!(ids(&["--owner", "test-agent"]), [a]);
    assert!(ids(&["--tags", "nope"]).is_empty());
}

#[test]
fn list_sorts_and_limits() {
    let f = VaultFixture::new();
    let first = new_note(&f, "Zulu", "x");
    let second = new_note(&f, "Alpha", "x");

    let ids = |args: &[&str]| {
        let mut cmd = f.cmd();
        cmd.args(["note", "list", "--quiet"]);
        cmd.args(args);
        let out = cmd.output().expect("run");
        stdout_of(&out)
            .lines()
            .map(str::to_string)
            .collect::<Vec<String>>()
    };
    // Newest first by default.
    assert_eq!(ids(&[]), [second.clone(), first.clone()]);
    assert_eq!(ids(&["--sort", "created"]), [second.clone(), first.clone()]);
    assert_eq!(ids(&["--sort", "title"]), [second.clone(), first.clone()]);
    assert_eq!(ids(&["--limit", "1"]), [second]);
    assert!(ids(&["--limit", "0"]).is_empty());
    assert_eq!(ids(&["--limit=-1"]).len(), 2);
    assert!(!first.is_empty());
}

#[test]
fn list_rejects_a_bad_sort_field_and_a_bad_since() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["note", "list", "--sort", "bogus"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(2));
    assert_eq!(
        stderr_of(&out).trim(),
        "invalid sort field: 'bogus' (use updated, created, title)"
    );

    let out = f
        .cmd()
        .args(["note", "list", "--since", "7y"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(2));
}

#[test]
fn list_since_keeps_only_recent_notes() {
    let f = VaultFixture::new();
    let recent = new_note(&f, "Recent", "x");
    f.write(
        "notes/n-OLD1.md",
        "---\nid: n-OLD1\ntype: note\ntitle: Old\ntags: []\nowner: null\n\
         created: 2020-01-01T00:00:00Z\nupdated: 2020-01-01T00:00:00Z\nrelated: []\n---\n\nold\n",
    );
    let out = f
        .cmd()
        .args(["note", "list", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out).lines().count(), 2);
    let out = f
        .cmd()
        .args(["note", "list", "--since", "1d", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{recent}\n"));
}

#[test]
fn list_skips_foreign_and_corrupt_files_unless_asked() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "x");
    f.write("notes/loose.md", "# Loose Heading\n\ntext\n");
    f.write(
        "notes/n-BAD.md",
        "---\nid: n-BAD\ntitle: [unclosed\n---\n\nbroken\n",
    );

    let out = f.cmd().args(["note", "list"]).output().expect("run");
    assert_eq!(stdout_of(&out), format!("{id}  note  Alpha\n"));

    let out = f
        .cmd()
        .args(["note", "list", "--foreign"])
        .output()
        .expect("run");
    assert_eq!(
        stdout_of(&out),
        format!("{id}  note  Alpha\n-  -  Loose Heading\n"),
        "a corrupt mesh file is never surfaced as foreign"
    );

    let out = f
        .cmd()
        .args(["note", "list", "--foreign", "--json"])
        .output()
        .expect("run");
    let rows = json_of(&out);
    let last = rows.as_array().expect("array").last().expect("row").clone();
    assert!(last["id"].is_null() && last["type"].is_null());
    assert_eq!(last["title"], Json::String("Loose Heading".into()));
    assert!(last["path"]
        .as_str()
        .expect("path")
        .ends_with("notes/loose.md"));
}

#[test]
fn a_filtered_listing_never_admits_a_foreign_row() {
    let f = VaultFixture::new();
    new_note(&f, "Alpha", "x");
    f.write("notes/loose.md", "# Loose\n");
    let out = f
        .cmd()
        .args(["note", "list", "--foreign", "--type", "note"])
        .output()
        .expect("run");
    assert!(!stdout_of(&out).contains("Loose"), "{}", stdout_of(&out));
}

// ---------------------------------------------------------------------------------------
// note delete
// ---------------------------------------------------------------------------------------

#[test]
fn delete_removes_the_file_and_reports_it() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "x");
    f.cmd()
        .args(["note", "delete", &id, "--force"])
        .assert()
        .success()
        .stdout(format!("deleted {id}\n"));
    assert!(f.files().is_empty());
}

#[test]
fn delete_json_and_quiet_shapes() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "x");
    f.cmd()
        .args(["note", "delete", &id, "--force", "--json"])
        .assert()
        .success()
        .stdout(format!("{{\"id\":\"{id}\",\"deleted\":true}}\n"));

    let id = new_note(&f, "Beta", "x");
    f.cmd()
        .args(["note", "delete", &id, "--force", "--quiet"])
        .assert()
        .success()
        .stdout(format!("{id}\n"));
}

#[test]
fn delete_refuses_a_machine_path_without_force() {
    let f = VaultFixture::new();
    let id = new_note(&f, "Alpha", "x");
    for extra in [vec![], vec!["--json"], vec!["--quiet"]] {
        let mut cmd = f.cmd();
        cmd.args(["note", "delete", &id]);
        cmd.args(&extra);
        let out = cmd.output().expect("run");
        assert_eq!(code_of(&out), Some(2), "{extra:?}");
        assert!(f.files().contains(&format!("notes/{id}.md")));
    }
    let out = f.cmd().args(["note", "delete", &id]).output().expect("run");
    assert_eq!(
        stderr_of(&out).trim(),
        "refusing to delete on a non-interactive path; pass --force to confirm"
    );
}

#[test]
fn delete_exits_three_before_the_guard_when_the_target_is_missing() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["note", "delete", "n-NOPE"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(3), "not-found beats the delete guard");
    assert_eq!(stderr_of(&out).trim(), "note not found: n-NOPE");
}

#[test]
fn delete_can_remove_a_corrupt_note() {
    let f = VaultFixture::new();
    f.write(
        "notes/n-BAD.md",
        "---\nid: n-BAD\ntitle: [unclosed\n---\n\nbroken\n",
    );
    f.cmd()
        .args(["note", "delete", "n-BAD", "--force", "--quiet"])
        .assert()
        .success()
        .stdout("n-BAD\n");
    assert!(f.files().is_empty());
}

#[test]
fn delete_never_touches_a_foreign_file() {
    let f = VaultFixture::new();
    f.write("notes/loose.md", "# Loose\n");
    let out = f
        .cmd()
        .args(["note", "delete", "loose", "--force"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(3));
    assert!(f.files().contains(&"notes/loose.md".to_string()));
}

// ---------------------------------------------------------------------------------------
// flag placement, help, and read-only guarantees
// ---------------------------------------------------------------------------------------

#[test]
fn json_on_either_side_of_the_command_name_is_byte_identical() {
    let f = VaultFixture::from_corpus();
    let id = corpus_id("N1");
    for tail in [
        vec!["note", "list"],
        vec!["note", "list", "--foreign"],
        vec!["note", "get", id.as_str()],
        vec!["note", "get", id.as_str(), "--meta-only"],
        vec!["note", "get", id.as_str(), "--related"],
    ] {
        let mut left = f.cmd();
        left.arg("--json").args(&tail);
        let left = left.output().expect("run");

        let mut right = f.cmd();
        right.args(&tail).arg("--json");
        let right = right.output().expect("run");

        assert_eq!(left.stdout, right.stdout, "{tail:?}");
        assert_eq!(code_of(&left), code_of(&right), "{tail:?}");
    }
}

#[test]
fn note_help_lists_the_six_subcommands_in_order() {
    let f = VaultFixture::new();
    let out = f.cmd().args(["note", "--help"]).output().expect("run");
    let text = stdout_of(&out);
    let mut at = 0usize;
    for name in ["new", "append", "update", "get", "list", "delete"] {
        let found = text.get(at..).and_then(|rest| rest.find(name));
        let offset = found.unwrap_or_else(|| panic!("{name} missing or out of order in {text}"));
        at += offset + name.len();
    }
    f.cmd()
        .args(["note", "new", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Note type: note | log | decision | reference | project.",
        ));
    f.cmd()
        .args(["note", "list", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("updated | created | title."));
}

#[test]
fn read_verbs_never_rewrite_a_file() {
    let f = VaultFixture::from_corpus();
    let before: Vec<(String, String)> = f
        .files()
        .into_iter()
        .filter(|p| p.ends_with(".md"))
        .map(|p| {
            let text = f.read(&p);
            (p, text)
        })
        .collect();

    for args in [
        vec!["note", "list"],
        vec!["note", "list", "--json"],
        vec!["note", "list", "--foreign", "--json"],
        vec!["note", "get", "n-6YQY", "--json"],
        vec!["note", "get", "foreign", "--foreign"],
    ] {
        f.cmd().args(&args).output().expect("run");
    }
    for (path, text) in before {
        assert_eq!(f.read(&path), text, "{path} was rewritten by a read verb");
    }
}

// ---------------------------------------------------------------------------------------
// the Python-written compat corpus
// ---------------------------------------------------------------------------------------

#[test]
fn list_over_the_python_corpus_matches_the_golden_payload() {
    let f = VaultFixture::from_corpus();
    let out = f
        .cmd()
        .args(["note", "list", "--json"])
        .output()
        .expect("run");
    let mut expected = golden("note_list.json");
    patch_yaml11(&mut expected);
    assert_eq!(stdout_of(&out).trim_end(), compact(&expected));
}

#[test]
fn get_over_the_python_corpus_matches_the_golden_payload() {
    let f = VaultFixture::from_corpus();
    let out = f
        .cmd()
        .args(["note", "get", &corpus_id("N1"), "--json"])
        .output()
        .expect("run");
    let expected = golden("note_get_n1.json");
    assert_eq!(stdout_of(&out).trim_end(), compact(&expected));
}

#[test]
fn every_python_written_note_is_readable_by_id_and_by_slug() {
    let f = VaultFixture::from_corpus();
    for (key, slug) in [
        ("N1", "alpha-note"),
        ("N2", "beta-note"),
        ("N3", "gamma-decision"),
        ("N4", "delta-reference"),
        ("P1", "project-apollo"),
        ("N6", "this-is-a-very-long-title-that-keeps-going-and-going-past-eighty-characters-for-sure-yes-indeed"),
    ] {
        let id = corpus_id(key);
        f.cmd()
            .args(["note", "get", &id, "--quiet"])
            .assert()
            .success()
            .stdout(format!("{id}\n"));
        f.cmd()
            .args(["note", "get", slug, "--quiet"])
            .assert()
            .success()
            .stdout(format!("{id}\n"));
    }
}

#[test]
fn appending_to_a_python_note_changes_only_updated_related_and_the_body() {
    let f = VaultFixture::from_corpus();
    let id = corpus_id("N1");
    let rel = format!("notes/{id}.md");
    let before = f.read(&rel);

    f.cmd()
        .args(["note", "append", &id, "a new paragraph", "--quiet"])
        .assert()
        .success();
    let after = f.read(&rel);

    let field = |text: &str, key: &str| -> String {
        text.lines()
            .find(|l| l.starts_with(key))
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(field(&before, "created:"), field(&after, "created:"));
    assert_eq!(field(&before, "title:"), field(&after, "title:"));
    assert_eq!(field(&before, "owner:"), field(&after, "owner:"));
    assert_ne!(field(&before, "updated:"), field(&after, "updated:"));
    assert!(after.ends_with("a new paragraph\n"), "{after}");
    assert!(
        after.contains("Appended paragraph."),
        "the old body survives"
    );
}

#[test]
fn a_python_note_with_unknown_keys_round_trips_through_an_amend() {
    let f = VaultFixture::from_corpus();
    f.cmd()
        .args(["note", "append", "n-HAND", "another line", "--quiet"])
        .assert()
        .success();
    let text = f.read("notes/n-HAND.md");
    for marker in [
        "aliases:",
        "  - hand",
        "custom_key: keep me",
        "extra:",
        "quoted_date:",
        "offset_ts: 2026-01-02 03:04:05+02:00",
    ] {
        assert!(text.contains(marker), "{marker} lost:\n{text}");
    }
    assert!(
        text.contains("created: 2026-01-02\n"),
        "an untouched Ts re-emits verbatim"
    );
}

#[test]
fn the_corpus_broken_note_is_not_found_everywhere_but_deletable() {
    let f = VaultFixture::from_corpus();
    for args in [
        vec!["note", "get", "n-BAD"],
        vec!["note", "append", "n-BAD", "x"],
        vec!["note", "update", "n-BAD", "--tags", "a"],
    ] {
        let out = f.cmd().args(&args).output().expect("run");
        assert_eq!(code_of(&out), Some(3), "{args:?}");
    }
    f.cmd()
        .args(["note", "delete", "n-BAD", "--force", "--quiet"])
        .assert()
        .success();
    assert!(!f.files().contains(&"notes/n-BAD.md".to_string()));
}

#[test]
fn the_corpus_foreign_file_is_readable_only_through_foreign() {
    let f = VaultFixture::from_corpus();
    let out = f
        .cmd()
        .args(["note", "get", "foreign", "--foreign", "--json"])
        .output()
        .expect("run");
    assert_eq!(code_of(&out), Some(0));
    let payload = json_of(&out);
    assert!(payload["id"].is_null());
    assert_eq!(payload["title"], Json::String("Foreign Heading".into()));
    assert!(payload["body"]
        .as_str()
        .expect("body")
        .contains("zebra appears here"));

    for args in [
        vec!["note", "get", "foreign"],
        vec!["note", "append", "foreign", "x"],
        vec!["note", "update", "foreign", "--tags", "a"],
        vec!["note", "delete", "foreign", "--force"],
    ] {
        let out = f.cmd().args(&args).output().expect("run");
        assert_eq!(code_of(&out), Some(3), "{args:?}");
    }
    assert!(f.files().contains(&"notes/foreign.md".to_string()));
}

#[test]
fn a_python_note_survives_a_type_move() {
    let f = VaultFixture::from_corpus();
    let id = corpus_id("N2");
    f.cmd()
        .args(["note", "update", &id, "--type", "reference", "--quiet"])
        .assert()
        .success();
    let files = f.files();
    assert!(
        files.contains(&format!("notes/references/{id}.md")),
        "{files:?}"
    );
    assert!(!files.contains(&format!("notes/logs/{id}.md")));
    f.cmd()
        .args(["note", "get", &id, "--quiet"])
        .assert()
        .success()
        .stdout(format!("{id}\n"));
}

#[test]
fn the_corpus_lock_directory_is_never_listed_as_a_note() {
    let f = VaultFixture::from_corpus();
    let out = f
        .cmd()
        .args(["note", "list", "--foreign", "--limit=-1"])
        .output()
        .expect("run");
    let text = stdout_of(&out);
    assert!(!text.contains(".locks"), "{text}");
    assert!(!text.contains("gitkeep"), "{text}");
    // Eight mesh notes plus the one foreign file; n-BAD is skipped.
    assert_eq!(text.lines().count(), 9, "{text}");
}
