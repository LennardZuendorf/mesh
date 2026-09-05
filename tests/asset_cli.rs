//! Integration tests for `mesh asset …`, driven through the real binary.

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

use common::VaultFixture;
use serde_json::Value as Json;

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json_stdout(out: &Output) -> Json {
    serde_json::from_str(stdout_of(out).trim()).expect("json on stdout")
}

fn json_err(out: &Output) -> Json {
    serde_json::from_str(stderr_of(out).trim()).expect("json envelope on stderr")
}

fn keys(value: &Json) -> Vec<String> {
    value
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect::<Vec<String>>()
}

/// A shipped fixture under `tests/fixtures/asset/`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/asset")
        .join(name)
}

/// A file written next to the vault, so ingest always copies across a boundary.
fn scratch_file(f: &VaultFixture, name: &str, bytes: &[u8]) -> PathBuf {
    let path = f.dir.path().join("src").join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create src dir");
    }
    std::fs::write(&path, bytes).expect("write source");
    path
}

/// `asset add <src> [extra…] --quiet` — returns the id.
fn add_with(f: &VaultFixture, src: &Path, extra: &[&str]) -> String {
    let out = f
        .cmd()
        .args(["asset", "add"])
        .arg(src)
        .args(extra)
        .arg("--quiet")
        .output()
        .expect("run asset add");
    assert!(out.status.success(), "{}", stderr_of(&out));
    stdout_of(&out).trim().to_string()
}

/// Add a shipped fixture and return its id.
fn add_fixture(f: &VaultFixture, name: &str) -> String {
    add_with(f, &fixture(name), &[])
}

/// A note in the vault; returns its id.
fn new_note(f: &VaultFixture, title: &str) -> String {
    let out = f
        .cmd()
        .args(["note", "new", title, "--body", "seed", "--quiet"])
        .output()
        .expect("run note new");
    assert!(out.status.success(), "{}", stderr_of(&out));
    stdout_of(&out).trim().to_string()
}

/// A task in the vault; returns its id.
fn new_task(f: &VaultFixture, title: &str) -> String {
    let out = f
        .cmd()
        .args(["task", "new", title, "--quiet"])
        .output()
        .expect("run task new");
    assert!(out.status.success(), "{}", stderr_of(&out));
    stdout_of(&out).trim().to_string()
}

fn sidecar(f: &VaultFixture, id: &str) -> String {
    f.read(&format!("assets/{id}.md"))
}

/// The config with `[spaces] assets = false`.
fn disabled() -> VaultFixture {
    VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n\
         [tasks]\ncollections = []\n\n[spaces]\nassets = false\n",
    )
}

// ---------------------------------------------------------------------------------------
// add
// ---------------------------------------------------------------------------------------

#[test]
fn add_writes_a_blob_and_a_sidecar_that_share_a_stem() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "note.txt");
    assert!(id.starts_with("a-"), "{id}");
    let files = f.files();
    assert!(files.contains(&format!("assets/{id}.md")), "{files:?}");
    assert!(files.contains(&format!("assets/{id}.txt")), "{files:?}");
}

#[test]
fn add_copies_a_binary_fixture_byte_for_byte() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "pixel.png");
    let source = std::fs::read(fixture("pixel.png")).expect("read fixture");
    let stored = std::fs::read(f.vault.join(format!("assets/{id}.png"))).expect("read blob");
    assert_eq!(stored, source);
    assert!(sidecar(&f, &id).contains("media_type: image/png"));
    assert!(sidecar(&f, &id).contains(&format!("bytes: {}", source.len())));
}

#[test]
fn add_never_moves_or_links_the_source() {
    let f = VaultFixture::new();
    let src = scratch_file(&f, "keep.txt", b"still mine");
    let id = add_with(&f, &src, &[]);
    assert!(src.is_file(), "the operator's file survives ingest");
    assert_eq!(std::fs::read(&src).expect("read source"), b"still mine");
    // A copy, not a hard link: the two inodes are independent.
    std::fs::write(&src, b"changed").expect("rewrite source");
    assert_eq!(
        std::fs::read(f.vault.join(format!("assets/{id}.txt"))).expect("read blob"),
        b"still mine"
    );
}

#[test]
fn add_writes_the_frontmatter_in_asset_field_order() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "note.txt");
    let text = sidecar(&f, &id);
    let yaml = text.split("---\n").nth(1).expect("frontmatter").to_string();
    let written: Vec<&str> = yaml
        .lines()
        .filter(|l| !l.starts_with(' ') && !l.starts_with('-'))
        .filter_map(|l| l.split(':').next())
        .collect();
    assert_eq!(
        written,
        [
            "id",
            "type",
            "title",
            "tags",
            "owner",
            "created",
            "updated",
            "related",
            "filename",
            "media_type",
            "bytes",
            "sha256",
            "blob"
        ]
    );
}

#[test]
fn add_human_output_is_added_id() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["asset", "add"])
        .arg(fixture("note.txt"))
        .output()
        .expect("run");
    assert!(out.status.success());
    assert!(
        stdout_of(&out).starts_with("added a-"),
        "{:?}",
        stdout_of(&out)
    );
}

#[test]
fn add_json_is_id_bytes_updated() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["asset", "add"])
        .arg(fixture("note.txt"))
        .arg("--json")
        .output()
        .expect("run");
    let payload = json_stdout(&out);
    assert_eq!(keys(&payload), ["id", "bytes", "updated"]);
    assert_eq!(payload["bytes"], Json::from(26));
    assert!(payload["updated"].as_str().expect("updated").ends_with('Z'));
}

#[test]
fn add_quiet_beats_json_and_prints_the_id_alone() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["asset", "add"])
        .arg(fixture("note.txt"))
        .args(["--json", "--quiet"])
        .output()
        .expect("run");
    let text = stdout_of(&out);
    assert!(text.starts_with("a-"), "{text:?}");
    assert!(!text.contains('{'), "{text:?}");
}

#[test]
fn add_defaults_the_title_to_the_source_basename() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "pixel.png");
    assert!(sidecar(&f, &id).contains("title: pixel.png"));
    assert!(sidecar(&f, &id).contains("filename: pixel.png"));
}

#[test]
fn add_honours_title_tags_owner_and_caption() {
    let f = VaultFixture::new();
    let id = add_with(
        &f,
        &fixture("note.txt"),
        &[
            "--title",
            "Trip Photo",
            "--tags",
            "japan, trip ,japan",
            "--owner",
            "alice",
            "--caption",
            "shot on the last day",
        ],
    );
    let text = sidecar(&f, &id);
    assert!(text.contains("title: Trip Photo"), "{text}");
    assert!(text.contains("  - japan\n  - trip\n"), "{text}");
    assert!(text.contains("owner: alice"), "{text}");
    assert!(text.trim_end().ends_with("shot on the last day"), "{text}");
}

#[test]
fn add_dedupes_identical_bytes_and_leaves_the_sidecar_untouched() {
    let f = VaultFixture::new();
    let first = add_fixture(&f, "note.txt");
    let before = sidecar(&f, &first);
    let copy = scratch_file(&f, "same.txt", b"hello from the asset lane\n");
    let out = f
        .cmd()
        .args(["asset", "add"])
        .arg(&copy)
        .output()
        .expect("run");
    assert!(out.status.success());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stderr_of(&out),
        format!("asset add: identical content already stored as {first}\n")
    );
    assert_eq!(stdout_of(&out), format!("added {first}\n"));
    assert_eq!(sidecar(&f, &first), before, "updated must not move");
    let sidecars: Vec<String> = f
        .files()
        .into_iter()
        .filter(|p| p.starts_with("assets/") && p.ends_with(".md"))
        .collect();
    assert_eq!(sidecars.len(), 1, "{sidecars:?}");
}

#[test]
fn the_dedupe_notice_is_suppressed_by_quiet() {
    let f = VaultFixture::new();
    let first = add_fixture(&f, "note.txt");
    let out = f
        .cmd()
        .args(["asset", "add"])
        .arg(fixture("note.txt"))
        .arg("--quiet")
        .output()
        .expect("run");
    assert_eq!(stderr_of(&out), "");
    assert_eq!(stdout_of(&out).trim(), first);
}

#[test]
fn identical_bytes_under_different_names_share_one_id() {
    let f = VaultFixture::new();
    let a = add_with(&f, &scratch_file(&f, "one.txt", b"same"), &[]);
    let b = add_with(&f, &scratch_file(&f, "two.bin", b"same"), &[]);
    assert_eq!(a, b);
    // The first ingest decided the extension; the second wrote nothing.
    assert!(f.files().contains(&format!("assets/{a}.txt")));
    assert!(!f.files().contains(&format!("assets/{a}.bin")));
}

#[test]
fn an_uppercase_extension_is_lowercased_on_the_blob() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "UPPER.TXT");
    assert!(f.files().contains(&format!("assets/{id}.txt")));
    let text = sidecar(&f, &id);
    assert!(text.contains("filename: UPPER.TXT"), "{text}");
    assert!(text.contains("media_type: text/plain"), "{text}");
}

#[test]
fn a_double_extension_keeps_only_the_last_segment() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "data.tar.gz");
    assert!(f.files().contains(&format!("assets/{id}.gz")));
    assert!(sidecar(&f, &id).contains("media_type: application/gzip"));
}

#[test]
fn a_file_with_no_extension_gets_no_extension_and_the_default_media_type() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "README");
    assert!(
        f.files().contains(&format!("assets/{id}")),
        "{:?}",
        f.files()
    );
    let text = sidecar(&f, &id);
    assert!(
        text.contains("media_type: application/octet-stream"),
        "{text}"
    );
    assert!(text.contains(&format!("blob: {id}")), "{text}");
}

#[test]
fn an_unusable_extension_is_dropped() {
    let f = VaultFixture::new();
    for (name, bytes) in [
        ("weird.p!ng", &b"one"[..]),
        ("long.abcdefghijklm", &b"two"[..]),
        ("trailing.", &b"three"[..]),
    ] {
        let src = scratch_file(&f, name, bytes);
        let id = add_with(&f, &src, &[]);
        assert!(
            f.files().contains(&format!("assets/{id}")),
            "{name} kept an extension: {:?}",
            f.files()
        );
    }
}

#[test]
fn a_hostile_filename_lands_inside_the_assets_root() {
    let f = VaultFixture::new();
    let src = scratch_file(&f, "..\\..\\evil.png", b"hostile bytes");
    let id = add_with(&f, &src, &[]);
    let files = f.files();
    assert_eq!(
        files
            .iter()
            .filter(|p| p.starts_with("assets/"))
            .cloned()
            .collect::<Vec<String>>(),
        vec![format!("assets/{id}.md"), format!("assets/{id}.png")]
    );
    assert!(!f.vault.join("evil.png").exists());
    assert!(!f.dir.path().join("evil.png").exists());
    // The original name survives as data, never as a path.
    assert!(sidecar(&f, &id).contains("evil.png"));
}

#[test]
fn an_unreadable_source_is_exit_two_and_writes_nothing() {
    let f = VaultFixture::new();
    let missing = f.dir.path().join("nope.png");
    let out = f
        .cmd()
        .args(["asset", "add"])
        .arg(&missing)
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr_of(&out).starts_with(&format!("cannot read {}: ", missing.display())),
        "{}",
        stderr_of(&out)
    );
    assert!(f.files().iter().all(|p| !p.starts_with("assets/")));
}

#[test]
fn a_directory_source_is_exit_two() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["asset", "add"])
        .arg(f.dir.path())
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr_of(&out).starts_with("cannot read "),
        "{}",
        stderr_of(&out)
    );
}

#[test]
fn add_rejects_an_owner_outside_the_roster() {
    let f = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n\
         [tasks]\ncollections = [\"alice\"]\n",
    );
    let out = f
        .cmd()
        .args(["asset", "add"])
        .arg(fixture("note.txt"))
        .args(["--owner", "ghost"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr_of(&out), "unknown owner: 'ghost'\n");
    assert!(f.files().iter().all(|p| !p.starts_with("assets/")));
}

#[test]
fn add_attach_links_the_target_in_one_command() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_with(&f, &fixture("pixel.png"), &["--attach", &note]);
    let note_text = f.read(&format!("notes/{note}.md"));
    assert!(note_text.contains(&format!("![[{id}.png]]")), "{note_text}");
    assert!(note_text.contains(&format!("  - {id}")), "{note_text}");
    assert!(sidecar(&f, &id).contains(&format!("  - {note}")));
}

// ---------------------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------------------

#[test]
fn get_human_is_twelve_meta_lines_then_the_caption() {
    let f = VaultFixture::new();
    let id = add_with(&f, &fixture("note.txt"), &["--caption", "a caption"]);
    let out = f.cmd().args(["asset", "get", &id]).output().expect("run");
    let text = stdout_of(&out);
    let lines: Vec<&str> = text.trim_end().split('\n').collect();
    assert_eq!(lines.len(), 14, "{text}");
    assert_eq!(lines[0], format!("id: {id}"));
    assert_eq!(lines[1], "type: asset");
    assert_eq!(lines[2], "title: note.txt");
    assert_eq!(lines[3], "filename: note.txt");
    assert_eq!(lines[4], "media_type: text/plain");
    assert_eq!(lines[5], "bytes: 26");
    assert!(lines[6].starts_with("sha256: "));
    assert_eq!(lines[7], "owner: test-agent");
    assert_eq!(lines[8], "tags: ");
    assert!(lines[9].starts_with("created: "));
    assert!(lines[10].starts_with("updated: "));
    assert_eq!(lines[11], "related: ");
    assert_eq!(lines[12], "");
    assert_eq!(lines[13], "a caption");
}

#[test]
fn get_meta_only_drops_the_caption() {
    let f = VaultFixture::new();
    let id = add_with(&f, &fixture("note.txt"), &["--caption", "a caption"]);
    let out = f
        .cmd()
        .args(["asset", "get", &id, "--meta-only"])
        .output()
        .expect("run");
    let text = stdout_of(&out);
    assert_eq!(text.trim_end().split('\n').count(), 12);
    assert!(!text.contains("a caption"));
    let payload = json_stdout(
        &f.cmd()
            .args(["asset", "get", &id, "--meta-only", "--json"])
            .output()
            .expect("run"),
    );
    assert!(payload.get("body").is_none());
}

#[test]
fn get_previews_two_hundred_code_points_unless_full() {
    let f = VaultFixture::new();
    let caption = "x".repeat(250);
    let id = add_with(&f, &fixture("note.txt"), &["--caption", &caption]);
    let short = stdout_of(&f.cmd().args(["asset", "get", &id]).output().expect("run"));
    assert!(short.contains(&"x".repeat(200)));
    assert!(!short.contains(&"x".repeat(201)));
    let full = stdout_of(
        &f.cmd()
            .args(["asset", "get", &id, "--full"])
            .output()
            .expect("run"),
    );
    assert!(full.contains(&caption));
}

#[test]
fn get_json_is_the_frontmatter_then_the_body() {
    let f = VaultFixture::new();
    let id = add_with(&f, &fixture("note.txt"), &["--caption", "cap"]);
    let payload = json_stdout(
        &f.cmd()
            .args(["asset", "get", &id, "--json"])
            .output()
            .expect("run"),
    );
    assert_eq!(
        keys(&payload),
        [
            "id",
            "type",
            "title",
            "tags",
            "owner",
            "created",
            "updated",
            "related",
            "filename",
            "media_type",
            "bytes",
            "sha256",
            "blob",
            "body"
        ]
    );
    assert_eq!(payload["body"], Json::from("cap"));
    assert_eq!(payload["bytes"], Json::from(26));
}

#[test]
fn get_quiet_prints_the_id() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "note.txt");
    let out = f
        .cmd()
        .args(["asset", "get", &id, "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{id}\n"));
}

#[test]
fn get_on_a_missing_id_is_exit_three_with_candidates() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "note.txt");
    let out = f
        .cmd()
        .args(["--json", "asset", "get", "a-ZZZZZZ"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(3));
    let envelope = json_err(&out);
    assert_eq!(envelope["kind"], Json::from("not_found"));
    assert_eq!(envelope["message"], Json::from("asset not found: a-ZZZZZZ"));
    assert_eq!(envelope["candidates"], Json::from(vec![id]));
}

#[test]
fn a_corrupt_sidecar_is_exit_three_on_read_and_still_removable() {
    let f = VaultFixture::new();
    f.write(
        "assets/a-BAD.md",
        "---\nid: a-BAD\ntitle: [oops\n---\n\nx\n",
    );
    f.write("assets/a-BAD.png", "blobby");
    for args in [
        vec!["asset", "get", "a-BAD"],
        vec!["asset", "path", "a-BAD"],
    ] {
        let out = f.cmd().args(&args).output().expect("run");
        assert_eq!(out.status.code(), Some(3), "{args:?}");
    }
    f.cmd()
        .args(["asset", "remove", "a-BAD", "--force"])
        .assert()
        .success();
    assert!(f.files().iter().all(|p| !p.starts_with("assets/a-BAD")));
}

// ---------------------------------------------------------------------------------------
// path
// ---------------------------------------------------------------------------------------

#[test]
fn path_prints_the_absolute_blob_path_and_nothing_else() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "pixel.png");
    let out = f.cmd().args(["asset", "path", &id]).output().expect("run");
    let text = stdout_of(&out);
    assert_eq!(text.lines().count(), 1);
    let path = PathBuf::from(text.trim());
    assert!(path.is_absolute(), "{path:?}");
    assert!(path.is_file(), "{path:?}");
    assert!(path.ends_with(format!("{id}.png")));
}

#[test]
fn path_json_carries_the_id_and_the_path() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "pixel.png");
    let payload = json_stdout(
        &f.cmd()
            .args(["asset", "path", &id, "--json"])
            .output()
            .expect("run"),
    );
    assert_eq!(keys(&payload), ["id", "path"]);
    assert_eq!(payload["id"], Json::from(id));
    assert!(payload["path"].as_str().expect("path").starts_with('/'));
}

#[test]
fn path_is_exit_three_when_the_id_or_the_blob_is_missing() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "pixel.png");
    let out = f
        .cmd()
        .args(["asset", "path", "a-NOPE"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_of(&out), "asset not found: a-NOPE\n");

    std::fs::remove_file(f.vault.join(format!("assets/{id}.png"))).expect("remove blob");
    let out = f.cmd().args(["asset", "path", &id]).output().expect("run");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_of(&out), format!("asset not found: {id}\n"));
}

// ---------------------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------------------

#[test]
fn list_rows_are_tab_separated() {
    let f = VaultFixture::new();
    let id = add_with(&f, &fixture("note.txt"), &["--title", "Note file"]);
    let out = f.cmd().args(["asset", "list"]).output().expect("run");
    assert_eq!(
        stdout_of(&out),
        format!("{id}\ttext/plain\t26\tNote file\n")
    );
}

#[test]
fn list_json_is_an_array_of_frontmatter_with_no_body() {
    let f = VaultFixture::new();
    add_fixture(&f, "note.txt");
    let payload = json_stdout(
        &f.cmd()
            .args(["asset", "list", "--json"])
            .output()
            .expect("run"),
    );
    let rows = payload.as_array().expect("array");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].get("body").is_none());
    assert_eq!(rows[0]["type"], Json::from("asset"));
}

#[test]
fn list_json_beats_quiet_and_quiet_alone_prints_ids() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "note.txt");
    let out = f
        .cmd()
        .args(["asset", "list", "--json", "--quiet"])
        .output()
        .expect("run");
    assert!(stdout_of(&out).starts_with('['), "{}", stdout_of(&out));
    let out = f
        .cmd()
        .args(["asset", "list", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{id}\n"));
}

#[test]
fn list_sorts_by_bytes_descending() {
    let f = VaultFixture::new();
    let small = add_with(&f, &scratch_file(&f, "s.png", b"1"), &[]);
    let big = add_with(&f, &scratch_file(&f, "b.png", &[b'x'; 400]), &[]);
    let mid = add_with(&f, &scratch_file(&f, "m.png", b"12345"), &[]);
    let out = f
        .cmd()
        .args(["asset", "list", "--sort", "bytes", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{big}\n{mid}\n{small}\n"));
}

#[test]
fn list_sorts_by_title_and_created() {
    let f = VaultFixture::new();
    let b = add_with(&f, &scratch_file(&f, "1.txt", b"one"), &["--title", "Beta"]);
    let a = add_with(
        &f,
        &scratch_file(&f, "2.txt", b"two"),
        &["--title", "alpha"],
    );
    let out = f
        .cmd()
        .args(["asset", "list", "--sort", "title", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{a}\n{b}\n"));
    let out = f
        .cmd()
        .args(["asset", "list", "--sort", "created", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{a}\n{b}\n"), "newest first");
}

#[test]
fn list_filters_by_media_type() {
    let f = VaultFixture::new();
    let png = add_fixture(&f, "pixel.png");
    add_fixture(&f, "note.txt");
    let out = f
        .cmd()
        .args(["asset", "list", "--media-type", "image/png", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{png}\n"));
    let out = f
        .cmd()
        .args(["asset", "list", "--media-type", "image/gif", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), "");
}

#[test]
fn list_filters_by_tags_any_tag_owner_and_mine() {
    let f = VaultFixture::new();
    let tagged = add_with(
        &f,
        &scratch_file(&f, "1.txt", b"one"),
        &["--tags", "a,b", "--owner", "alice"],
    );
    let other = add_with(&f, &scratch_file(&f, "2.txt", b"two"), &["--tags", "c"]);
    let quiet_ids = |args: &[&str]| -> String {
        stdout_of(
            &f.cmd()
                .args(["asset", "list"])
                .args(args)
                .arg("--quiet")
                .output()
                .expect("run"),
        )
    };
    assert_eq!(quiet_ids(&["--tags", "a,b"]), format!("{tagged}\n"));
    assert_eq!(quiet_ids(&["--tags", "a,c"]), "");
    let any = quiet_ids(&["--tags", "a,c", "--any-tag"]);
    assert!(any.contains(&tagged) && any.contains(&other), "{any}");
    assert_eq!(quiet_ids(&["--owner", "alice"]), format!("{tagged}\n"));
    // `--mine` is the acting identity: test-agent owns only the second asset.
    assert_eq!(quiet_ids(&["--mine"]), format!("{other}\n"));
}

#[test]
fn list_limit_slices_and_negative_is_unbounded() {
    let f = VaultFixture::new();
    add_with(&f, &scratch_file(&f, "1.txt", b"one"), &[]);
    add_with(&f, &scratch_file(&f, "2.txt", b"two"), &[]);
    let count = |args: &[&str]| -> usize {
        stdout_of(
            &f.cmd()
                .args(["asset", "list"])
                .args(args)
                .arg("--quiet")
                .output()
                .expect("run"),
        )
        .lines()
        .count()
    };
    assert_eq!(count(&["--limit", "1"]), 1);
    assert_eq!(count(&["--limit", "0"]), 0);
    assert_eq!(count(&["--limit=-1"]), 2);
}

#[test]
fn list_since_filters_on_updated() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "note.txt");
    let out = f
        .cmd()
        .args(["asset", "list", "--since", "1d", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{id}\n"));
    let out = f
        .cmd()
        .args(["asset", "list", "--since", "2999-01-01", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), "");
}

#[test]
fn list_rejects_an_unknown_sort_field() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["asset", "list", "--sort", "bogus"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out),
        "invalid sort field: 'bogus' (use updated, created, title, bytes)\n"
    );
}

#[test]
fn list_never_shows_a_foreign_or_invalid_file() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "note.txt");
    f.write("assets/loose.md", "# not an asset\n");
    f.write(
        "assets/n-1234.md",
        "---\nid: n-1234\ntype: note\ntitle: T\ncreated: 2026-01-02\nupdated: 2026-01-02\n---\n\nx\n",
    );
    let out = f
        .cmd()
        .args(["asset", "list", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{id}\n"));
}

// ---------------------------------------------------------------------------------------
// attach / detach
// ---------------------------------------------------------------------------------------

#[test]
fn attach_to_a_note_embeds_the_blob_and_links_both_related_lists() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_fixture(&f, "pixel.png");
    let out = f
        .cmd()
        .args(["asset", "attach", &id, &note])
        .output()
        .expect("run");
    assert!(out.status.success());
    assert_eq!(stdout_of(&out), format!("attached {id} to {note}\n"));
    let note_text = f.read(&format!("notes/{note}.md"));
    assert!(note_text.contains(&format!("![[{id}.png]]")), "{note_text}");
    assert!(
        note_text.contains(&format!("related:\n  - {id}")),
        "{note_text}"
    );
    assert!(sidecar(&f, &id).contains(&format!("related:\n  - {note}")));
}

#[test]
fn attach_to_a_task_embeds_the_blob_and_links_both_related_lists() {
    let f = VaultFixture::new();
    let task = new_task(&f, "Book flights");
    let id = add_fixture(&f, "pixel.png");
    f.cmd()
        .args(["asset", "attach", &id, &task])
        .assert()
        .success();
    let task_text = f.read(&format!("tasks/open/{task}.md"));
    assert!(task_text.contains(&format!("![[{id}.png]]")), "{task_text}");
    assert!(task_text.contains(&format!("  - {id}")), "{task_text}");
    assert!(sidecar(&f, &id).contains(&format!("  - {task}")));
}

#[test]
#[ignore = "memory lane pending"]
fn attach_to_a_memory_embeds_the_blob_and_links_both_related_lists() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["memory", "new", "Prefers aisle seats", "--quiet"])
        .output()
        .expect("run");
    let memory = stdout_of(&out).trim().to_string();
    let id = add_fixture(&f, "pixel.png");
    f.cmd()
        .args(["asset", "attach", &id, &memory])
        .assert()
        .success();
    let text = f.read(&format!("memories/{memory}.md"));
    assert!(text.contains(&format!("![[{id}.png]]")), "{text}");
    assert!(text.contains(&format!("  - {id}")), "{text}");
    assert!(sidecar(&f, &id).contains(&format!("  - {memory}")));
}

#[test]
fn attach_json_is_id_target_updated() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_fixture(&f, "pixel.png");
    let payload = json_stdout(
        &f.cmd()
            .args(["asset", "attach", &id, &note, "--json"])
            .output()
            .expect("run"),
    );
    assert_eq!(keys(&payload), ["id", "target", "updated"]);
    assert_eq!(payload["id"], Json::from(id));
    assert_eq!(payload["target"], Json::from(note));
}

#[test]
fn attach_quiet_prints_the_asset_id_alone() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_fixture(&f, "pixel.png");
    let out = f
        .cmd()
        .args(["asset", "attach", &id, &note, "--json", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{id}\n"));
}

#[test]
fn attach_can_target_a_section() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_fixture(&f, "pixel.png");
    f.cmd()
        .args(["asset", "attach", &id, &note, "--section", "Photos"])
        .assert()
        .success();
    let text = f.read(&format!("notes/{note}.md"));
    assert!(
        text.contains(&format!("## Photos\n\n![[{id}.png]]")),
        "{text}"
    );
}

#[test]
fn attaching_twice_changes_nothing() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_fixture(&f, "pixel.png");
    f.cmd()
        .args(["asset", "attach", &id, &note])
        .assert()
        .success();
    let before = (f.read(&format!("notes/{note}.md")), sidecar(&f, &id));
    f.cmd()
        .args(["asset", "attach", &id, &note])
        .assert()
        .success();
    assert_eq!(f.read(&format!("notes/{note}.md")), before.0);
    assert_eq!(sidecar(&f, &id), before.1);
}

#[test]
fn attach_rejects_a_target_that_is_not_an_entity_id() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "pixel.png");
    let out = f
        .cmd()
        .args(["asset", "attach", &id, "trip"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out),
        "invalid target id: 'trip' (use an n-, t- or m- id)\n"
    );
}

#[test]
fn attach_to_a_missing_target_is_exit_three() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "pixel.png");
    let out = f
        .cmd()
        .args(["asset", "attach", &id, "n-9999"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_of(&out), "note not found: n-9999\n");
    let out = f
        .cmd()
        .args(["asset", "attach", "a-NOPE", "n-9999"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_of(&out), "asset not found: a-NOPE\n");
}

#[test]
fn detach_unlinks_both_related_lists_and_leaves_the_body() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_fixture(&f, "pixel.png");
    f.cmd()
        .args(["asset", "attach", &id, &note])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["asset", "detach", &id, &note])
        .output()
        .expect("run");
    assert!(out.status.success());
    assert_eq!(stdout_of(&out), format!("detached {id} from {note}\n"));
    let note_text = f.read(&format!("notes/{note}.md"));
    assert!(note_text.contains("related: []"), "{note_text}");
    assert!(
        note_text.contains(&format!("![[{id}.png]]")),
        "the body belongs to the agent: {note_text}"
    );
    assert!(sidecar(&f, &id).contains("related: []"));
}

#[test]
fn detaching_twice_changes_nothing() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_fixture(&f, "pixel.png");
    f.cmd()
        .args(["asset", "attach", &id, &note])
        .assert()
        .success();
    f.cmd()
        .args(["asset", "detach", &id, &note])
        .assert()
        .success();
    let before = (f.read(&format!("notes/{note}.md")), sidecar(&f, &id));
    f.cmd()
        .args(["asset", "detach", &id, &note])
        .assert()
        .success();
    assert_eq!(f.read(&format!("notes/{note}.md")), before.0);
    assert_eq!(sidecar(&f, &id), before.1);
}

#[test]
fn detach_json_is_id_target_updated() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_fixture(&f, "pixel.png");
    f.cmd()
        .args(["asset", "attach", &id, &note])
        .assert()
        .success();
    let payload = json_stdout(
        &f.cmd()
            .args(["asset", "detach", &id, &note, "--json"])
            .output()
            .expect("run"),
    );
    assert_eq!(keys(&payload), ["id", "target", "updated"]);
}

// ---------------------------------------------------------------------------------------
// remove
// ---------------------------------------------------------------------------------------

#[test]
fn remove_refuses_a_referenced_asset_without_force() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_fixture(&f, "pixel.png");
    f.cmd()
        .args(["asset", "attach", &id, &note])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["asset", "remove", &id])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out),
        format!("asset {id} is referenced by 1 entities; pass --force\n")
    );
    assert!(f.files().contains(&format!("assets/{id}.md")));
}

#[test]
fn remove_with_force_deletes_the_sidecar_and_the_blob() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_fixture(&f, "pixel.png");
    f.cmd()
        .args(["asset", "attach", &id, &note])
        .assert()
        .success();
    let out = f
        .cmd()
        .args(["asset", "remove", &id, "--force"])
        .output()
        .expect("run");
    assert!(out.status.success());
    assert_eq!(stdout_of(&out), format!("deleted {id}\n"));
    assert!(f
        .files()
        .iter()
        .all(|p| !p.starts_with(&format!("assets/{id}"))));
}

#[test]
fn remove_without_force_on_a_machine_path_is_the_delete_guard() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "pixel.png");
    let out = f
        .cmd()
        .args(["asset", "remove", &id, "--json"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        json_err(&out)["message"],
        Json::from("refusing to delete on a non-interactive path; pass --force to confirm")
    );
    assert!(f.files().contains(&format!("assets/{id}.md")));
}

#[test]
fn remove_json_and_quiet_shapes() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "pixel.png");
    let payload = json_stdout(
        &f.cmd()
            .args(["asset", "remove", &id, "--force", "--json"])
            .output()
            .expect("run"),
    );
    assert_eq!(keys(&payload), ["id", "deleted"]);
    assert_eq!(payload["deleted"], Json::Bool(true));

    let second = add_fixture(&f, "note.txt");
    let out = f
        .cmd()
        .args(["asset", "remove", &second, "--force", "--quiet"])
        .output()
        .expect("run");
    assert_eq!(stdout_of(&out), format!("{second}\n"));
}

#[test]
fn remove_on_a_missing_id_is_exit_three() {
    let f = VaultFixture::new();
    let out = f
        .cmd()
        .args(["asset", "remove", "a-NOPE", "--force"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_of(&out), "asset not found: a-NOPE\n");
}

// ---------------------------------------------------------------------------------------
// gc
// ---------------------------------------------------------------------------------------

#[test]
fn gc_reports_orphan_blobs_and_orphan_sidecars() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "pixel.png");
    f.write("assets/stray.bin", "junk");
    f.write(
        "assets/a-GHOST.md",
        "---\nid: a-GHOST\ntype: asset\ntitle: g\ntags: []\nowner: null\n\
         created: 2026-01-02T00:00:00Z\nupdated: 2026-01-02T00:00:00Z\nrelated: []\n\
         filename: g.png\nmedia_type: image/png\nbytes: 1\nsha256: ff\nblob: a-GHOST.png\n\
         ---\n\nx\n",
    );
    let out = f.cmd().args(["asset", "gc"]).output().expect("run");
    assert!(out.status.success());
    assert_eq!(
        stdout_of(&out),
        "orphan_blobs: stray.bin\norphan_sidecars: a-GHOST\nremoved: 0\n"
    );
    // Read-only: nothing moved.
    assert!(f.files().contains(&"assets/stray.bin".to_string()));
    assert!(f.files().contains(&format!("assets/{id}.png")));
}

#[test]
fn gc_json_is_orphan_blobs_orphan_sidecars_removed() {
    let f = VaultFixture::new();
    add_fixture(&f, "pixel.png");
    f.write("assets/stray.bin", "junk");
    let payload = json_stdout(
        &f.cmd()
            .args(["asset", "gc", "--json"])
            .output()
            .expect("run"),
    );
    assert_eq!(
        keys(&payload),
        ["orphan_blobs", "orphan_sidecars", "removed"]
    );
    assert_eq!(payload["orphan_blobs"], Json::from(vec!["stray.bin"]));
    assert_eq!(payload["orphan_sidecars"], Json::from(Vec::<String>::new()));
    assert_eq!(payload["removed"], Json::from(0));
}

#[test]
fn gc_apply_removes_orphan_blobs_only() {
    let f = VaultFixture::new();
    let id = add_fixture(&f, "pixel.png");
    f.write("assets/stray.bin", "junk");
    f.write(
        "assets/a-GHOST.md",
        "---\nid: a-GHOST\ntype: asset\ntitle: g\ntags: []\nowner: null\n\
         created: 2026-01-02T00:00:00Z\nupdated: 2026-01-02T00:00:00Z\nrelated: []\n\
         filename: g.png\nmedia_type: image/png\nbytes: 1\nsha256: ff\nblob: a-GHOST.png\n\
         ---\n\nx\n",
    );
    let payload = json_stdout(
        &f.cmd()
            .args(["asset", "gc", "--apply", "--json"])
            .output()
            .expect("run"),
    );
    assert_eq!(payload["removed"], Json::from(1));
    let files = f.files();
    assert!(!files.contains(&"assets/stray.bin".to_string()));
    assert!(
        files.contains(&"assets/a-GHOST.md".to_string()),
        "sidecars stay"
    );
    assert!(files.contains(&format!("assets/{id}.png")));
}

#[test]
fn gc_on_a_healthy_vault_reports_nothing() {
    let f = VaultFixture::new();
    add_fixture(&f, "pixel.png");
    let payload = json_stdout(
        &f.cmd()
            .args(["asset", "gc", "--json"])
            .output()
            .expect("run"),
    );
    assert_eq!(payload["orphan_blobs"], Json::from(Vec::<String>::new()));
    assert_eq!(payload["orphan_sidecars"], Json::from(Vec::<String>::new()));
    assert_eq!(payload["removed"], Json::from(0));
}

// ---------------------------------------------------------------------------------------
// cross-cutting
// ---------------------------------------------------------------------------------------

#[test]
fn a_disabled_assets_space_is_exit_two_on_every_subcommand() {
    let f = disabled();
    let note = new_note(&f, "Trip");
    let cases: Vec<Vec<String>> = vec![
        vec![
            "asset".into(),
            "add".into(),
            fixture("note.txt").display().to_string(),
        ],
        vec!["asset".into(), "get".into(), "a-1".into()],
        vec!["asset".into(), "path".into(), "a-1".into()],
        vec!["asset".into(), "list".into()],
        vec!["asset".into(), "attach".into(), "a-1".into(), note],
        vec!["asset".into(), "detach".into(), "a-1".into(), "n-1".into()],
        vec![
            "asset".into(),
            "remove".into(),
            "a-1".into(),
            "--force".into(),
        ],
        vec!["asset".into(), "gc".into()],
    ];
    for args in cases {
        let out = f.cmd().args(&args).output().expect("run");
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(
            stderr_of(&out),
            "space 'assets' is disabled in [spaces]\n",
            "{args:?}"
        );
    }
}

#[test]
fn a_missing_config_is_exit_two_on_every_subcommand() {
    let f = VaultFixture::new();
    let out = f.bare_cmd().args(["asset", "list"]).output().expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr_of(&out).starts_with("mesh: no config found at "));
}

#[test]
fn json_is_accepted_on_either_side_of_the_command_name() {
    let f = VaultFixture::new();
    let note = new_note(&f, "Trip");
    let id = add_fixture(&f, "pixel.png");
    let pairs: Vec<Vec<String>> = vec![
        vec!["asset".into(), "get".into(), id.clone()],
        vec!["asset".into(), "path".into(), id.clone()],
        vec!["asset".into(), "list".into()],
        vec!["asset".into(), "gc".into()],
        vec!["asset".into(), "attach".into(), id.clone(), note.clone()],
    ];
    for args in pairs {
        let left = f
            .cmd()
            .arg("--json")
            .args(&args)
            .output()
            .expect("run left");
        let right = f
            .cmd()
            .args(&args)
            .arg("--json")
            .output()
            .expect("run right");
        assert_eq!(left.status.code(), right.status.code(), "{args:?}");
        assert_eq!(stdout_of(&left), stdout_of(&right), "{args:?}");
    }
}

#[test]
fn add_json_placement_is_byte_identical() {
    let a = VaultFixture::new();
    let b = VaultFixture::new();
    let left = a
        .cmd()
        .args(["--json", "asset", "add"])
        .arg(fixture("note.txt"))
        .output()
        .expect("run left");
    let right = b
        .cmd()
        .args(["asset", "add"])
        .arg(fixture("note.txt"))
        .arg("--json")
        .output()
        .expect("run right");
    let strip = |out: &Output| -> Json {
        let mut value = json_stdout(out);
        if let Some(map) = value.as_object_mut() {
            map.remove("updated");
        }
        value
    };
    assert_eq!(strip(&left), strip(&right));
}

#[test]
fn read_verbs_never_rewrite_a_sidecar() {
    let f = VaultFixture::new();
    let id = add_with(&f, &fixture("note.txt"), &["--caption", "cap"]);
    let before = std::fs::read(f.vault.join(format!("assets/{id}.md"))).expect("read");
    for args in [
        vec!["asset", "get", &id],
        vec!["asset", "get", &id, "--json"],
        vec!["asset", "path", &id],
        vec!["asset", "list"],
        vec!["asset", "list", "--json"],
        vec!["asset", "gc"],
    ] {
        f.cmd().args(&args).assert().success();
    }
    assert_eq!(
        std::fs::read(f.vault.join(format!("assets/{id}.md"))).expect("read"),
        before
    );
}

#[test]
fn asset_help_lists_the_subcommands_in_registration_order() {
    let f = VaultFixture::new();
    let out = f.cmd().args(["asset", "--help"]).output().expect("run");
    let text = stdout_of(&out);
    let mut cursor = 0usize;
    for name in [
        "add", "get", "path", "list", "attach", "detach", "remove", "gc",
    ] {
        let at = text[cursor..]
            .find(&format!("  {name}"))
            .unwrap_or_else(|| panic!("{name} missing or out of order in:\n{text}"));
        cursor += at + name.len();
    }
}

#[test]
fn asset_with_no_subcommand_prints_help_to_stdout_and_exits_two() {
    let f = VaultFixture::new();
    let out = f.cmd().arg("asset").output().expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stdout_of(&out).contains("Usage: asset"),
        "{}",
        stdout_of(&out)
    );
    assert_eq!(stderr_of(&out), "");
}
