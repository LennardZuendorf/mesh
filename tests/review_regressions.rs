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

// ---------------------------------------------------------------------------------------
// a shared space belongs to the operator: mesh deletes only the files it wrote
// ---------------------------------------------------------------------------------------

/// A config whose assets space is the vault root — the layout `config.example.toml` documents.
fn shared_assets() -> String {
    "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n\
     [tasks]\ncollections = []\n\n[spaces]\nassets = \".\"\n"
        .to_string()
}

#[test]
fn gc_never_sweeps_a_file_mesh_did_not_write() {
    // With `assets = "."` the assets space is a folder the operator also keeps files in.
    // "No sidecar names it" is not an ownership test, and `gc --apply` unlinks hard, with no
    // trash and no recovery.
    let f = VaultFixture::with(&shared_assets());
    let operator_files = [
        ("important-spreadsheet.csv", "col1,col2\n1,2\n"),
        ("family-photo.jpg", "\u{fffd}JPEGDATA"),
        ("notes.txt", "do not delete\n"),
        (
            "a-not-an-id.bin",
            "an operator file that merely starts with a-\n",
        ),
    ];
    for (name, body) in operator_files {
        f.write(name, body);
    }
    // One real asset, so the sweep has something of its own to look at.
    let src = f.dir.path().join("payload.png");
    std::fs::write(&src, b"PNGDATA").expect("write source");
    let asset = ok(&f, &["--quiet", "asset", "add", &src.to_string_lossy()])
        .trim()
        .to_string();

    let report = ok(&f, &["--json", "asset", "gc", "--apply"]);
    let payload: Json = serde_json::from_str(&report).expect("json");
    assert_eq!(payload["orphan_blobs"], serde_json::json!([]), "{report}");
    assert_eq!(payload["removed"], serde_json::json!(0), "{report}");

    for (name, body) in operator_files {
        assert_eq!(f.read(name), body, "gc deleted the operator's {name}");
    }
    // The asset mesh does own still round-trips.
    assert!(!ok(&f, &["--quiet", "asset", "path", &asset])
        .trim()
        .is_empty());
}

#[test]
fn gc_still_sweeps_a_blob_mesh_wrote_whose_sidecar_is_gone() {
    // The ownership test must not turn `gc --apply` into a no-op: a blob mesh itself wrote,
    // whose sidecar the operator deleted by hand, is exactly what the sweep is for.
    let f = VaultFixture::new();
    let src = f.dir.path().join("payload.png");
    std::fs::write(&src, b"PNGDATA").expect("write source");
    let asset = ok(&f, &["--quiet", "asset", "add", &src.to_string_lossy()])
        .trim()
        .to_string();
    std::fs::remove_file(f.vault.join(format!("assets/{asset}.md"))).expect("drop the sidecar");

    let report = ok(&f, &["--json", "asset", "gc", "--apply"]);
    let payload: Json = serde_json::from_str(&report).expect("json");
    assert_eq!(payload["removed"], serde_json::json!(1), "{report}");
    assert!(
        !f.vault.join(format!("assets/{asset}.png")).exists(),
        "the orphan blob survived"
    );
}

#[test]
fn remove_deletes_this_assets_own_blob_and_nothing_else() {
    // `blob` is frontmatter: agent- and editor-writable. Used as a bare path it names any
    // file in the sandbox, and `safe_resolve` only proves the file is inside the union of the
    // space roots — not that it is this asset's blob.
    let f = VaultFixture::new();
    let src = f.dir.path().join("payload.bin");
    std::fs::write(&src, b"asset bytes").expect("write source");
    let asset = ok(&f, &["--quiet", "asset", "add", &src.to_string_lossy()])
        .trim()
        .to_string();
    let victim = ok(
        &f,
        &["--quiet", "note", "new", "Victim", "--body", "precious"],
    )
    .trim()
    .to_string();

    let sidecar = format!("assets/{asset}.md");
    let text = f.read(&sidecar);
    f.write(
        &sidecar,
        &text.replace(
            &format!("blob: {asset}.bin"),
            &format!("blob: ../notes/{victim}.md"),
        ),
    );
    assert!(f.read(&sidecar).contains("blob: ../notes/"), "fixture");

    ok(&f, &["--quiet", "asset", "remove", &asset, "--force"]);

    let victim_text = f.read(&format!("notes/{victim}.md"));
    assert!(
        victim_text.contains("precious"),
        "remove deleted a note in another space: {victim_text}"
    );
    assert!(
        !f.vault.join(format!("assets/{asset}.bin")).exists(),
        "the asset's real blob was left orphaned"
    );
}

#[test]
fn remove_never_deletes_another_assets_blob() {
    // The in-space variant: `blob` pointing at a sibling asset's bytes would destroy them
    // while that sibling's sidecar still records a sha256 for content mesh no longer holds.
    let f = VaultFixture::new();
    let mut ids: Vec<String> = Vec::new();
    for (n, body) in [(0, "first bytes"), (1, "second bytes")] {
        let src = f.dir.path().join(format!("p{n}.bin"));
        std::fs::write(&src, body).expect("write source");
        ids.push(
            ok(&f, &["--quiet", "asset", "add", &src.to_string_lossy()])
                .trim()
                .to_string(),
        );
    }
    let (doomed, bystander) = (&ids[0], &ids[1]);
    let sidecar = format!("assets/{doomed}.md");
    let text = f.read(&sidecar);
    f.write(
        &sidecar,
        &text.replace(
            &format!("blob: {doomed}.bin"),
            &format!("blob: {bystander}.bin"),
        ),
    );

    ok(&f, &["--quiet", "asset", "remove", doomed, "--force"]);

    assert_eq!(
        f.read(&format!("assets/{bystander}.bin")),
        "second bytes",
        "remove destroyed a bystander asset's bytes"
    );
    let report = ok(&f, &["--json", "asset", "gc"]);
    let payload: Json = serde_json::from_str(&report).expect("json");
    assert_eq!(
        payload["orphan_sidecars"],
        serde_json::json!([]),
        "the bystander was left as a sidecar with no blob: {report}"
    );
}

// ---------------------------------------------------------------------------------------
// reconciliation moves a file; it never destroys one
// ---------------------------------------------------------------------------------------

#[test]
fn reconciliation_never_renames_over_an_occupied_destination() {
    // The destination is the correct folder plus the *source's own basename*, which two files
    // in different subfolders can share. `rename` is unconditional, so one sweep unlinked the
    // occupant — a different entity, whose writers this lock never excluded either.
    let f = VaultFixture::new();
    std::fs::create_dir_all(f.vault.join("notes/logs")).expect("logs dir");
    f.write(
        "notes/logs/Ideas.md",
        "---\nid: n-VICT\ntype: log\ntitle: Victim log\n---\n\nVICTIM CONTENT\n",
    );
    f.write(
        "notes/Ideas.md",
        "---\nid: n-INTR\ntype: log\ntitle: Intruder log\n---\n\nINTRUDER CONTENT\n",
    );

    ok(&f, &["watch", "--once", "--no-index"]);

    assert!(
        f.read("notes/logs/Ideas.md").contains("VICTIM CONTENT"),
        "reconciliation overwrote the occupant"
    );
    assert!(
        f.read("notes/Ideas.md").contains("INTRUDER CONTENT"),
        "the source must stay put when its destination is taken"
    );
}

#[test]
fn reconciliation_still_files_a_misplaced_note() {
    // The occupied-destination guard must not stop reconciliation doing its job.
    let f = VaultFixture::new();
    std::fs::create_dir_all(f.vault.join("notes")).expect("notes dir");
    f.write(
        "notes/Stray.md",
        "---\nid: n-STRY\ntype: log\ntitle: Stray log\n---\n\nSTRAY CONTENT\n",
    );

    ok(&f, &["watch", "--once", "--no-index"]);

    assert!(
        f.read("notes/logs/Stray.md").contains("STRAY CONTENT"),
        "a misfiled note was not reconciled"
    );
    assert!(
        !f.vault.join("notes/Stray.md").exists(),
        "the source stayed"
    );
}

// ---------------------------------------------------------------------------------------
// an agent identity is an address component, never an empty one
// ---------------------------------------------------------------------------------------

#[test]
fn an_identity_that_slugifies_to_empty_is_refused() {
    // `slugify` keeps ASCII alphanumerics only, so an emoji, "..." or any all-non-ASCII name
    // slugifies to "". `root.join("")` is a no-op, so every such identity shared one file at
    // `<scratch>/<name>.md` and one lock — silently overwriting each other's state.
    let f = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}\"\nagent = \"test-agent\"\n\n[tasks]\ncollections = []\n",
    );
    for identity in [
        "\u{3051}\u{3093}\u{304d}\u{3087}",
        "\u{1f916}\u{1f916}",
        "...",
        "---",
    ] {
        let (_, stderr, code) = run(
            &f,
            &[
                "scratch", "set", "plan", "--agent", identity, "--body", "state",
            ],
        );
        assert_eq!(code, 2, "identity {identity:?} was accepted");
        assert_eq!(stderr, format!("invalid agent identity: '{identity}'"));
    }
    assert!(
        !f.vault.join("scratch/plan.md").exists(),
        "a refused identity still wrote to the namespace root"
    );
}

#[test]
fn distinct_identities_never_share_one_scratch_file() {
    let f = VaultFixture::new();
    ok(
        &f,
        &[
            "--quiet", "scratch", "set", "plan", "--agent", "alpha", "--body", "ALPHA",
        ],
    );
    ok(
        &f,
        &[
            "--quiet", "scratch", "set", "plan", "--agent", "beta", "--body", "BETA",
        ],
    );
    assert!(f.read("scratch/alpha/plan.md").contains("ALPHA"));
    assert!(f.read("scratch/beta/plan.md").contains("BETA"));
    assert!(
        !f.vault.join("scratch/plan.md").exists(),
        "a scratch file landed in the namespace root"
    );
}

// ---------------------------------------------------------------------------------------
// `--owner` is the acting identity everywhere, not only where it filters
// ---------------------------------------------------------------------------------------

/// Ids of the rows in a JSON array.
fn ids(payload: &str) -> Vec<String> {
    serde_json::from_str::<Json>(payload)
        .expect("json array")
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|row| row.get("id").and_then(Json::as_str))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_global_owner_is_the_identity_mine_resolves_in_every_space() {
    let f = VaultFixture::new();
    ok(&f, &["task", "new", "mine-task", "--owner", "test-agent"]);
    let bob_task = ok(&f, &["--quiet", "--owner", "bob", "task", "new", "bobs"]);
    let bob_note = ok(
        &f,
        &[
            "--quiet", "--owner", "bob", "note", "new", "bobs", "--body", "b",
        ],
    );

    // memory list was already right; task list and recent-activity read `[core].agent`
    // and answered empty for anyone but the configured agent.
    assert_eq!(
        ids(&ok(
            &f,
            &["--owner", "bob", "--mine", "task", "list", "--json"]
        )),
        std::slice::from_ref(&bob_task),
        "task list --mine must follow --owner"
    );
    let seen = ids(&ok(
        &f,
        &["--owner", "bob", "--mine", "recent-activity", "--json"],
    ));
    assert!(
        seen.contains(&bob_task) && seen.contains(&bob_note),
        "recent-activity --mine must follow --owner, got {seen:?}"
    );

    // And the configured agent still sees only its own row.
    assert_eq!(
        ids(&ok(&f, &["--mine", "task", "list", "--json"])).len(),
        1,
        "[core].agent stays the default identity"
    );
}

#[test]
fn task_new_records_the_global_owner_like_every_other_space() {
    let f = VaultFixture::new();
    // `mesh --owner bob X new` and `mesh X new --owner bob` are the same invocation.
    for space in ["task", "note", "memory"] {
        let pre = ok(
            &f,
            &[
                "--quiet", "--owner", "bob", space, "new", "pre", "--body", "b",
            ],
        );
        let post = ok(
            &f,
            &[
                "--quiet", space, "new", "post", "--owner", "bob", "--body", "b",
            ],
        );
        for id in [&pre, &post] {
            let row: Json =
                serde_json::from_str(&ok(&f, &[space, "get", id, "--json"])).expect("json object");
            assert_eq!(
                row["owner"],
                Json::String("bob".into()),
                "{space} {id}: flag placement changed the recorded owner"
            );
        }
    }
}

#[test]
fn task_next_claims_for_the_same_identity_it_selected_for() {
    let f = VaultFixture::new();
    let alice = ok(&f, &["--quiet", "task", "new", "alice work"]);
    ok(&f, &["--owner", "bob", "task", "new", "bob work"]);

    // Selection read `[core].agent` while the claim wrote `--owner`, so bob was handed
    // alice's task with his own name in `claimed_by`.
    let picked: Json = serde_json::from_str(&ok(
        &f,
        &[
            "--owner", "bob", "task", "next", "--mine", "--claim", "--json",
        ],
    ))
    .expect("json object");
    assert_eq!(picked["owner"], Json::String("bob".into()));
    assert_eq!(picked["claimed_by"], Json::String("bob".into()));
    assert_ne!(
        picked["id"],
        Json::String(alice),
        "bob must never be handed alice's task"
    );
}

// ---------------------------------------------------------------------------------------
// `config set` writes the TOML type the config reader accepts
// ---------------------------------------------------------------------------------------

#[test]
fn config_set_writes_a_value_the_reader_then_honours() {
    let f = VaultFixture::new();
    // Each of these used to land as an integer, a boolean or a bare string that
    // `load_config` dropped: `config set` exited 0 and the setting never took effect.
    for (key, value, want) in [
        ("tasks.strict", "1", serde_json::json!(true)),
        ("search.hybrid", "0", serde_json::json!(false)),
        ("core.agent", "true", serde_json::json!("true")),
        (
            "tasks.collections",
            "alice,bob",
            serde_json::json!(["alice", "bob"]),
        ),
    ] {
        ok(&f, &["config", "set", key, value]);
        let shown: Json =
            serde_json::from_str(&ok(&f, &["--json", "config", "show"])).expect("json object");
        let (table, leaf) = key.split_once('.').expect("dotted key");
        assert_eq!(shown[table][leaf], want, "config set {key} {value}");
    }

    // The roster the operator just typed is enforced on the next write.
    let (_, stderr, code) = run(&f, &["task", "new", "t", "--owner", "mallory"]);
    assert_eq!(code, 2);
    assert_eq!(stderr, "unknown owner: 'mallory'");
}

#[test]
fn config_set_refuses_a_value_its_reader_cannot_use() {
    let f = VaultFixture::new();
    for (key, value, want) in [
        (
            "tasks.strict",
            "maybe",
            "config set tasks.strict: expected a boolean (true/false), got 'maybe'",
        ),
        (
            "search.threshold",
            "nan",
            "config set search.threshold: expected a finite number, got 'nan'",
        ),
        (
            "tasks.collections",
            "[1, 2]",
            "config set tasks.collections: expected an array of strings, got '[1, 2]'",
        ),
        ("core.nope", "x", "unknown config key: 'core.nope'"),
    ] {
        let (_, stderr, code) = run(&f, &["config", "set", key, value]);
        assert_eq!(code, 2, "config set {key} {value}");
        assert_eq!(stderr, want, "config set {key} {value}");
    }
}

// ---------------------------------------------------------------------------------------
// a section heading the writer emits is one the reader can find again
// ---------------------------------------------------------------------------------------

#[test]
fn two_appends_to_one_section_share_one_heading() {
    let f = VaultFixture::new();
    let id = ok(&f, &["--quiet", "note", "new", "sec", "--body", "start"]);
    // The writer built `## {section}` untrimmed while the reader matched on `line.trim()`,
    // so a padded name never matched the heading it had just written.
    for name in ["Outcome ", " Outcome", "Outcome\n", "Outcome"] {
        ok(&f, &["note", "append", &id, "x", "--section", name]);
    }
    let body = ok(&f, &["note", "get", &id, "--full"]);
    assert_eq!(
        body.lines().filter(|l| l.trim() == "## Outcome").count(),
        1,
        "one section, four appends:\n{body}"
    );
}

// ---------------------------------------------------------------------------------------
// nothing is written that the walk would then refuse to read
// ---------------------------------------------------------------------------------------

#[test]
fn a_write_past_the_readable_size_cap_is_refused_not_stranded() {
    let f = VaultFixture::new();
    let big = f.dir.path().join("big.md");
    // Just under the 4 MiB walk cap, so the note is created and addressable.
    std::fs::write(&big, "x".repeat(4 * 1024 * 1024 - 4096)).expect("write body");
    let id = ok(
        &f,
        &[
            "--quiet",
            "note",
            "new",
            "big",
            "--file",
            &big.to_string_lossy(),
        ],
    );
    ok(&f, &["note", "get", &id]);

    // The append that would cross the cap must fail, not exit 0 into unaddressability.
    let (_, stderr, code) = run(&f, &["note", "append", &id, &"y".repeat(8192)]);
    assert_eq!(code, 2, "an over-cap append must fail: {stderr}");
    assert!(
        stderr.contains("over the 4194304-byte readable limit"),
        "{stderr}"
    );

    // The note the caller was handed an id for is still there.
    ok(&f, &["note", "get", &id]);
    assert_eq!(ids(&ok(&f, &["note", "list", "--json"])), [id]);
}

// ---------------------------------------------------------------------------------------
// one `--tags` spelling across the whole surface
// ---------------------------------------------------------------------------------------

#[test]
fn search_reads_the_same_csv_tags_every_other_verb_writes() {
    let f = VaultFixture::new();
    let id = ok(
        &f,
        &[
            "--quiet",
            "note",
            "new",
            "tagged",
            "--body",
            "hello world",
            "--tags",
            "alpha,beta",
        ],
    );
    // A CSV used to be one literal tag on `search` alone, so the AND filter matched nothing.
    for args in [
        vec!["search", "hello", "--tags", "alpha,beta", "--json"],
        vec![
            "search", "hello", "--tags", "alpha", "--tags", "beta", "--json",
        ],
    ] {
        assert_eq!(ids(&ok(&f, &args)), std::slice::from_ref(&id), "{args:?}");
    }
    assert!(
        ids(&ok(
            &f,
            &["search", "hello", "--tags", "alpha,absent", "--json"]
        ))
        .is_empty(),
        "AND semantics still hold"
    );
}

// ---------------------------------------------------------------------------------------
// a note's folder never contradicts the frontmatter that names its type
// ---------------------------------------------------------------------------------------

#[test]
fn a_note_update_files_the_note_where_its_type_says_it_lives() {
    let f = VaultFixture::new();
    let id = ok(&f, &["--quiet", "note", "new", "d3", "--body", "b"]);
    ok(&f, &["note", "update", &id, "--type", "log"]);
    let logs = f.vault.join("notes").join("logs").join(format!("{id}.md"));
    assert!(logs.is_file(), "a type change moves the file");

    // Strand it the way an interrupted write-then-move would, then check the next update
    // heals it — the idempotent repair `tasks::terminate` already had and this did not.
    let root = f.vault.join("notes").join(format!("{id}.md"));
    std::fs::rename(&logs, &root).expect("strand the note");
    ok(&f, &["note", "update", &id, "--title", "renamed"]);
    assert!(logs.is_file(), "an update heals a misfiled note");
    assert!(!root.is_file(), "and leaves nothing behind");
}
