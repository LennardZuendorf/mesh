//! `recent-activity`, `build-context`, `graph`, `project`, `session-start` and the `status`
//! payload, driven through the real binary.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::VaultFixture;
use serde_json::Value as Json;

// --------------------------------------------------------------------------------------------
// harness helpers
// --------------------------------------------------------------------------------------------

const CORPUS_CONFIG: &str = "[core]\nvault_path = \"{VAULT}\"\nagent = \"demo-agent\"\n\n\
                             [tasks]\ncollections = []\n";

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn lines(out: &std::process::Output) -> Vec<String> {
    stdout_of(out)
        .lines()
        .map(std::string::ToString::to_string)
        .collect()
}

fn json_of(out: &std::process::Output) -> Json {
    serde_json::from_str(stdout_of(out).trim()).expect("stdout is one JSON document")
}

fn ids_of(value: &Json) -> Vec<String> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|e| {
            e.get("id")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

fn strings(value: &Json, key: &str) -> Vec<String> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|e| {
            e.get(key)
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A vault fixture holding a copy of the Python corpus, with `demo-agent` as the identity.
fn corpus() -> VaultFixture {
    let fixture = VaultFixture::with(CORPUS_CONFIG);
    copy_tree(&common::corpus_dir(), &fixture.vault);
    fixture
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create dest");
    for entry in std::fs::read_dir(from).expect("read dir").flatten() {
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &dest);
        } else {
            std::fs::copy(&path, &dest).expect("copy file");
        }
    }
}

fn golden(name: &str) -> Json {
    let path = common::golden_dir().join(name);
    let text = std::fs::read_to_string(&path).expect("read golden");
    serde_json::from_str(&text).expect("golden is JSON")
}

/// Replace the values that legitimately differ between the Python run and ours: absolute
/// paths become basenames and `mtime` / `age_seconds` are dropped.
fn normalise(value: &Json) -> Json {
    match value {
        Json::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                let replaced = match key.as_str() {
                    "path" => Json::String(basename(item)),
                    "mtime" | "age_seconds" => Json::Null,
                    _ => normalise(item),
                };
                out.insert(key.clone(), replaced);
            }
            Json::Object(out)
        }
        Json::Array(items) => Json::Array(items.iter().map(normalise).collect()),
        other => other.clone(),
    }
}

fn basename(value: &Json) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Stamp the corpus copy with the mtimes the Python golden recorded, so mtime-ordered output
/// is comparable row for row.
fn apply_golden_mtimes(fixture: &VaultFixture) {
    for entry in golden("recent_activity.json").as_array().expect("array") {
        let name = basename(entry.get("path").unwrap_or(&Json::Null));
        let mtime = entry
            .get("mtime")
            .and_then(Json::as_f64)
            .expect("golden mtime");
        let Some(path) = find_file(&fixture.vault, &name) else {
            continue;
        };
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs_f64(mtime);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open for mtime");
        file.set_modified(when).expect("set mtime");
    }
}

fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}

/// Every file in the vault with its bytes — the read-only check.
fn snapshot(fixture: &VaultFixture) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    for rel in fixture.files() {
        let bytes = std::fs::read(fixture.vault.join(&rel)).unwrap_or_default();
        out.push((rel, bytes));
    }
    out
}

/// A minimal note, with an explicit `related` list.
fn note(fixture: &VaultFixture, id: &str, title: &str, owner: &str, related: &[&str], body: &str) {
    write_entity(fixture, "notes", id, title, owner, related, body, None);
}

/// A note with an explicit `updated` stamp.
fn note_at(
    fixture: &VaultFixture,
    id: &str,
    title: &str,
    owner: &str,
    related: &[&str],
    updated: &str,
) {
    write_entity(
        fixture,
        "notes",
        id,
        title,
        owner,
        related,
        "body",
        Some(updated),
    );
}

#[allow(clippy::too_many_arguments)]
fn write_entity(
    fixture: &VaultFixture,
    dir: &str,
    id: &str,
    title: &str,
    owner: &str,
    related: &[&str],
    body: &str,
    updated: Option<&str>,
) {
    let owner = if owner.is_empty() {
        "null".to_string()
    } else {
        owner.to_string()
    };
    let stamp = updated.unwrap_or("2026-01-01T00:00:00Z");
    let text = format!(
        "---\nid: {id}\ntype: note\ntitle: {title}\ntags: []\nowner: {owner}\n\
         created: 2026-01-01T00:00:00Z\nupdated: {stamp}\nrelated:{}\n---\n\n{body}\n",
        yaml_list(related)
    );
    fixture.write(&format!("{dir}/{id}.md"), &text);
}

#[allow(clippy::too_many_arguments)]
fn task(
    fixture: &VaultFixture,
    id: &str,
    title: &str,
    status: &str,
    owner: &str,
    claimed: &str,
    project: &str,
    related: &[&str],
    body: &str,
) {
    let sub = if status == "done" || status == "cancelled" {
        "done"
    } else {
        "open"
    };
    let owner = if owner.is_empty() {
        "null".to_string()
    } else {
        owner.to_string()
    };
    let claimed = if claimed.is_empty() {
        "null".to_string()
    } else {
        claimed.to_string()
    };
    let project = if project.is_empty() {
        "null".to_string()
    } else {
        project.to_string()
    };
    let text = format!(
        "---\nid: {id}\ntype: task\ntitle: {title}\ntags: []\nowner: {owner}\n\
         created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated:{}\n\
         status: {status}\npriority: null\nclaimed_by: {claimed}\nproject: {project}\n\
         blocks: []\nblocked_by: []\n---\n\n{body}\n",
        yaml_list(related)
    );
    fixture.write(&format!("tasks/{sub}/{id}.md"), &text);
}

fn memory(fixture: &VaultFixture, id: &str, title: &str, importance: i64, scope: &str) {
    let text = format!(
        "---\nid: {id}\ntype: memory\ntitle: {title}\ntags: []\nowner: null\n\
         created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated: []\n\
         kind: fact\nscope: {scope}\nimportance: {importance}\nsource: null\nexpires: null\n\
         superseded_by: null\n---\n\nmemory body\n"
    );
    fixture.write(&format!("memories/{id}.md"), &text);
}

fn yaml_list(items: &[&str]) -> String {
    if items.is_empty() {
        " []".to_string()
    } else {
        format!(
            "\n{}",
            items
                .iter()
                .map(|i| format!("  - {i}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

/// `mesh --json <cmd>` and `mesh <cmd> --json` must produce byte-identical stdout.
fn assert_flag_parity(fixture: &VaultFixture, args: &[&str]) {
    let mut left_args = vec!["--json"];
    left_args.extend_from_slice(args);
    let left = fixture.cmd().args(&left_args).output().expect("run mesh");
    let mut right_args = args.to_vec();
    right_args.push("--json");
    let right = fixture.cmd().args(&right_args).output().expect("run mesh");
    assert_eq!(left.status.code(), right.status.code(), "{args:?}");
    assert_eq!(left.stdout, right.stdout, "{args:?}");
}

// --------------------------------------------------------------------------------------------
// recent-activity
// --------------------------------------------------------------------------------------------

#[test]
fn recent_activity_rows_have_six_tab_separated_columns() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "Alpha", "test-agent", &[], "body");
    task(
        &fixture,
        "t-a",
        "Ship",
        "claimed",
        "test-agent",
        "peer",
        "",
        &[],
        "b",
    );
    let out = fixture
        .cmd()
        .args(["recent-activity"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(0));
    for line in lines(&out) {
        let columns: Vec<&str> = line.split('\t').collect();
        assert_eq!(columns.len(), 6, "{line}");
    }
    let text = stdout_of(&out);
    assert!(text.contains("t-a\ttask\ttest-agent\tpeer\tShip\t"));
    assert!(text.contains("n-a\tnote\ttest-agent\t-\tAlpha\t"));
}

#[test]
fn recent_activity_json_is_the_seven_key_row() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "Alpha", "test-agent", &[], "body");
    let out = fixture
        .cmd()
        .args(["recent-activity", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    let row = payload.as_array().expect("array")[0].clone();
    let keys: Vec<&str> = row
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
            "path",
            "mtime",
            "owner",
            "claimed_by"
        ]
    );
    assert!(row["mtime"].is_f64());
    assert!(row["claimed_by"].is_null());
    assert!(row.get("created").is_none());
    assert!(row.get("updated").is_none());
}

#[test]
fn recent_activity_is_newest_first() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-old", "Old", "test-agent", &[], "b");
    note(&fixture, "n-new", "New", "test-agent", &[], "b");
    let old = fixture.vault.join("notes/n-old.md");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(old)
        .expect("open");
    file.set_modified(SystemTime::now() - Duration::from_secs(600))
        .expect("set mtime");
    let out = fixture
        .cmd()
        .args(["recent-activity", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&out)), ["n-new", "n-old"]);
}

#[test]
fn recent_activity_quiet_prints_ids_and_json_wins() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "Alpha", "test-agent", &[], "b");
    let quiet = fixture
        .cmd()
        .args(["recent-activity", "--quiet"])
        .output()
        .expect("run mesh");
    assert_eq!(lines(&quiet), ["n-a"]);
    let both = fixture
        .cmd()
        .args(["recent-activity", "--json", "--quiet"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&both)), ["n-a"]);
}

#[test]
fn recent_activity_limit_caps_and_negative_is_unbounded() {
    let fixture = VaultFixture::new();
    for id in ["n-a", "n-b", "n-c"] {
        note(&fixture, id, id, "test-agent", &[], "b");
    }
    let capped = fixture
        .cmd()
        .args(["recent-activity", "--limit", "2", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&capped)).len(), 2);
    let zero = fixture
        .cmd()
        .args(["recent-activity", "--limit", "0", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(stdout_of(&zero).trim(), "[]");
    let unbounded = fixture
        .cmd()
        .args(["recent-activity", "--limit=-1", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&unbounded)).len(), 3);
}

#[test]
fn recent_activity_since_filters_and_a_bad_window_is_exit_two() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-fresh", "Fresh", "test-agent", &[], "b");
    note(&fixture, "n-old", "Old", "test-agent", &[], "b");
    let old = fixture.vault.join("notes/n-old.md");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(old)
        .expect("open");
    file.set_modified(SystemTime::now() - Duration::from_secs(60 * 60 * 24 * 30))
        .expect("set mtime");
    let out = fixture
        .cmd()
        .args(["recent-activity", "--since", "7d", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&out)), ["n-fresh"]);

    let bad = fixture
        .cmd()
        .args(["recent-activity", "--since", "7x"])
        .output()
        .expect("run mesh");
    assert_eq!(bad.status.code(), Some(2));
    assert!(stdout_of(&bad).is_empty());
}

#[test]
fn recent_activity_owner_and_mine_filter_before_the_cap() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-mine", "Mine", "test-agent", &[], "b");
    task(
        &fixture,
        "t-claimed",
        "C",
        "claimed",
        "peer",
        "test-agent",
        "",
        &[],
        "b",
    );
    note(&fixture, "n-peer", "Peer", "peer", &[], "b");
    let mine = fixture
        .cmd()
        .args(["recent-activity", "--mine", "--json"])
        .output()
        .expect("run mesh");
    let mut got = ids_of(&json_of(&mine));
    got.sort();
    assert_eq!(got, ["n-mine", "t-claimed"]);

    let by_owner = fixture
        .cmd()
        .args(["recent-activity", "--owner", "peer", "--json"])
        .output()
        .expect("run mesh");
    let mut got = ids_of(&json_of(&by_owner));
    got.sort();
    assert_eq!(got, ["n-peer", "t-claimed"]);

    // The filter runs before the display cap, so a tight limit still yields owned rows.
    let capped = fixture
        .cmd()
        .args(["recent-activity", "--mine", "--limit", "1", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&capped)).len(), 1);
}

#[test]
fn recent_activity_reads_the_global_mine_and_owner() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-mine", "Mine", "test-agent", &[], "b");
    note(&fixture, "n-peer", "Peer", "peer", &[], "b");
    let global = fixture
        .cmd()
        .args(["--mine", "--json", "recent-activity"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&global)), ["n-mine"]);
    let owner = fixture
        .cmd()
        .args(["--owner", "peer", "--json", "recent-activity"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&owner)), ["n-peer"]);
}

#[test]
fn recent_activity_never_prints_a_daemon_notice() {
    let fixture = corpus();
    let out = fixture
        .cmd()
        .args(["recent-activity"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(0));
    assert!(stderr_of(&out).is_empty(), "{}", stderr_of(&out));
    let json = fixture
        .cmd()
        .args(["recent-activity", "--json"])
        .output()
        .expect("run mesh");
    assert!(!stderr_of(&json).contains("daemon"));
}

#[test]
fn recent_activity_space_csv_widens_and_rejects_an_unknown_name() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "Alpha", "test-agent", &[], "b");
    memory(&fixture, "m-a", "Fact", 3, "shared");
    let default = fixture
        .cmd()
        .args(["recent-activity", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&default)), ["n-a"]);

    let wide = fixture
        .cmd()
        .args(["recent-activity", "--space", "notes,memories", "--json"])
        .output()
        .expect("run mesh");
    let mut got = ids_of(&json_of(&wide));
    got.sort();
    assert_eq!(got, ["m-a", "n-a"]);

    let bad = fixture
        .cmd()
        .args(["recent-activity", "--space", "nope"])
        .output()
        .expect("run mesh");
    assert_eq!(bad.status.code(), Some(2));
    assert!(stderr_of(&bad).starts_with("invalid space: 'nope'"));
}

#[test]
fn recent_activity_matches_the_python_golden() {
    let fixture = corpus();
    apply_golden_mtimes(&fixture);
    let out = fixture
        .cmd()
        .args(["recent-activity", "--limit=-1", "--json"])
        .output()
        .expect("run mesh");
    let got = json_of(&out);
    let want = golden("recent_activity.json");
    assert_eq!(ids_of(&got), ids_of(&want));
    assert_eq!(normalise(&got), normalise(&want));
}

#[test]
fn recent_activity_skips_corrupt_and_foreign_corpus_files() {
    let fixture = corpus();
    let out = fixture
        .cmd()
        .args(["recent-activity", "--limit=-1", "--json"])
        .output()
        .expect("run mesh");
    let ids = ids_of(&json_of(&out));
    assert!(!ids.iter().any(|id| id == "n-BAD"));
    assert_eq!(ids.len(), 14);
    assert!(!stdout_of(&out).contains("foreign.md"));
    assert!(!stdout_of(&out).contains(".locks"));
}

#[test]
fn recent_activity_is_read_only() {
    let fixture = corpus();
    let before = snapshot(&fixture);
    for args in [
        vec!["recent-activity"],
        vec!["recent-activity", "--json"],
        vec!["recent-activity", "--mine", "--since", "7d"],
    ] {
        fixture.cmd().args(&args).output().expect("run mesh");
    }
    assert_eq!(before, snapshot(&fixture));
}

#[test]
fn recent_activity_flag_placement_is_symmetric() {
    let fixture = corpus();
    apply_golden_mtimes(&fixture);
    assert_flag_parity(&fixture, &["recent-activity"]);
    assert_flag_parity(&fixture, &["recent-activity", "--limit", "3"]);
}

// --------------------------------------------------------------------------------------------
// build-context
// --------------------------------------------------------------------------------------------

#[test]
fn build_context_rows_have_four_columns_and_json_carries_frontmatter() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "Alpha", "test-agent", &["n-b"], "b");
    note(&fixture, "n-b", "Beta", "test-agent", &[], "b");
    let human = fixture
        .cmd()
        .args(["build-context", "n-a"])
        .output()
        .expect("run mesh");
    let rows = lines(&human);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].starts_with("n-a\tnote\tAlpha\t"));
    for row in &rows {
        assert_eq!(row.split('\t').count(), 4);
    }

    let json = fixture
        .cmd()
        .args(["build-context", "n-a", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&json);
    let node = payload.as_array().expect("array")[0].clone();
    let keys: Vec<&str> = node
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["id", "type", "title", "tags", "owner", "created", "updated", "related", "path"]
    );
}

#[test]
fn build_context_walks_to_depth() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &["n-b"], "b");
    note(&fixture, "n-b", "B", "test-agent", &["n-c"], "b");
    note(&fixture, "n-c", "C", "test-agent", &[], "b");
    let at = |depth: &str| {
        let out = fixture
            .cmd()
            .args(["build-context", "n-a", "--depth", depth, "--json"])
            .output()
            .expect("run mesh");
        ids_of(&json_of(&out))
    };
    assert_eq!(at("0"), ["n-a"]);
    assert_eq!(at("1"), ["n-a", "n-b"]);
    assert_eq!(at("2"), ["n-a", "n-b", "n-c"]);
}

#[test]
fn build_context_dedupes_cycles_and_diamonds() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &["n-b", "n-c"], "b");
    note(&fixture, "n-b", "B", "test-agent", &["n-d", "n-a"], "b");
    note(&fixture, "n-c", "C", "test-agent", &["n-d"], "b");
    note(&fixture, "n-d", "D", "test-agent", &["n-a"], "b");
    let out = fixture
        .cmd()
        .args(["build-context", "n-a", "--depth", "5", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&out)), ["n-a", "n-b", "n-c", "n-d"]);
}

#[test]
fn build_context_skips_a_dangling_neighbour() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &["n-ghost", "n-b"], "b");
    note(&fixture, "n-b", "B", "test-agent", &[], "b");
    let out = fixture
        .cmd()
        .args(["build-context", "n-a", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(ids_of(&json_of(&out)), ["n-a", "n-b"]);
}

#[test]
fn build_context_reports_an_unknown_seed_as_not_found() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["build-context", "n-nope"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_of(&out).trim(), "seed not found: n-nope");

    let json = fixture
        .cmd()
        .args(["build-context", "n-nope", "--json"])
        .output()
        .expect("run mesh");
    let envelope: Json = serde_json::from_str(stderr_of(&json).trim()).expect("envelope");
    assert_eq!(envelope["kind"], Json::String("not_found".into()));
    assert_eq!(
        envelope["message"],
        Json::String("seed not found: n-nope".into())
    );
}

#[test]
fn build_context_quiet_lists_ids_and_json_wins() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &["n-b"], "b");
    note(&fixture, "n-b", "B", "test-agent", &[], "b");
    let quiet = fixture
        .cmd()
        .args(["build-context", "n-a", "--quiet"])
        .output()
        .expect("run mesh");
    assert_eq!(lines(&quiet), ["n-a", "n-b"]);
    let both = fixture
        .cmd()
        .args(["build-context", "n-a", "--json", "--quiet"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&both)), ["n-a", "n-b"]);
}

#[test]
fn build_context_crosses_notes_tasks_and_memories() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &["t-x", "m-a"], "b");
    task(&fixture, "t-x", "X", "open", "test-agent", "", "", &[], "b");
    memory(&fixture, "m-a", "Fact", 3, "shared");
    let out = fixture
        .cmd()
        .args(["build-context", "n-a", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    assert_eq!(ids_of(&payload), ["n-a", "t-x", "m-a"]);
    let nodes = payload.as_array().expect("array");
    assert_eq!(nodes[1]["status"], Json::String("open".into()));
    assert_eq!(nodes[2]["kind"], Json::String("fact".into()));

    // Narrowing the corpus drops the task and the memory.
    let narrow = fixture
        .cmd()
        .args(["build-context", "n-a", "--space", "notes", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&narrow)), ["n-a"]);
}

#[test]
fn build_context_accepts_a_task_seed_and_a_title_slug() {
    let fixture = VaultFixture::new();
    task(
        &fixture,
        "t-root",
        "Root",
        "open",
        "test-agent",
        "",
        "",
        &["n-x"],
        "b",
    );
    note(&fixture, "n-x", "Ex Note", "test-agent", &[], "b");
    let from_task = fixture
        .cmd()
        .args(["build-context", "t-root", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&from_task)), ["t-root", "n-x"]);
    let by_slug = fixture
        .cmd()
        .args(["build-context", "ex-note", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&by_slug)), ["n-x"]);
}

#[test]
fn build_context_flag_placement_is_symmetric() {
    let fixture = corpus();
    assert_flag_parity(&fixture, &["build-context", "n-6YQY"]);
}

// --------------------------------------------------------------------------------------------
// graph
// --------------------------------------------------------------------------------------------

#[test]
fn graph_renders_a_two_space_indented_tree() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &["n-b", "n-c"], "b");
    note(&fixture, "n-b", "B", "test-agent", &["n-d"], "b");
    note(&fixture, "n-c", "C", "test-agent", &[], "b");
    note(&fixture, "n-d", "D", "test-agent", &[], "b");
    let out = fixture
        .cmd()
        .args(["graph", "n-a", "--depth", "2"])
        .output()
        .expect("run mesh");
    assert_eq!(
        lines(&out),
        [
            "n-a\tnote\tA",
            "  n-b\tnote\tB",
            "    n-d\tnote\tD",
            "  n-c\tnote\tC",
        ]
    );
}

#[test]
fn graph_json_is_seed_nodes_edges() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &["n-b"], "b");
    note(&fixture, "n-b", "B", "test-agent", &[], "b");
    let out = fixture
        .cmd()
        .args(["graph", "n-a", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    let keys: Vec<&str> = payload
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["seed", "nodes", "edges"]);
    assert_eq!(payload["seed"], Json::String("n-a".into()));
    assert_eq!(payload["edges"], serde_json::json!([["n-a", "n-b"]]));
    assert_eq!(ids_of(&payload["nodes"]), ["n-a", "n-b"]);
}

#[test]
fn graph_quiet_lists_ids_in_bfs_order() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &["n-b", "n-c"], "b");
    note(&fixture, "n-b", "B", "test-agent", &[], "b");
    note(&fixture, "n-c", "C", "test-agent", &[], "b");
    let out = fixture
        .cmd()
        .args(["graph", "n-a", "--quiet"])
        .output()
        .expect("run mesh");
    assert_eq!(lines(&out), ["n-a", "n-b", "n-c"]);
}

#[test]
fn graph_json_beats_quiet() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &[], "b");
    let out = fixture
        .cmd()
        .args(["graph", "n-a", "--json", "--quiet"])
        .output()
        .expect("run mesh");
    assert_eq!(json_of(&out)["seed"], Json::String("n-a".into()));
}

#[test]
fn graph_direction_in_walks_backlinks_and_keeps_link_direction() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &["n-b"], "b");
    note(&fixture, "n-b", "B", "test-agent", &[], "b");
    let out = fixture
        .cmd()
        .args(["graph", "n-b", "--direction", "in", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    assert_eq!(ids_of(&payload["nodes"]), ["n-b", "n-a"]);
    assert_eq!(payload["edges"], serde_json::json!([["n-a", "n-b"]]));

    let tree = fixture
        .cmd()
        .args(["graph", "n-b", "--direction", "in"])
        .output()
        .expect("run mesh");
    assert_eq!(lines(&tree), ["n-b\tnote\tB", "  n-a\tnote\tA"]);
}

#[test]
fn graph_direction_both_takes_out_first_and_dedupes_a_mutual_link() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &["n-b"], "b");
    note(&fixture, "n-b", "B", "test-agent", &["n-a"], "b");
    note(&fixture, "n-c", "C", "test-agent", &["n-a"], "b");
    let out = fixture
        .cmd()
        .args(["graph", "n-a", "--direction", "both", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    assert_eq!(ids_of(&payload["nodes"]), ["n-a", "n-b", "n-c"]);
    assert_eq!(
        payload["edges"],
        serde_json::json!([["n-a", "n-b"], ["n-c", "n-a"]])
    );
}

#[test]
fn graph_validates_direction_before_the_seed() {
    let fixture = VaultFixture::new();
    let bad = fixture
        .cmd()
        .args(["graph", "n-nope", "--direction", "sideways"])
        .output()
        .expect("run mesh");
    assert_eq!(bad.status.code(), Some(2));
    assert_eq!(
        stderr_of(&bad).trim(),
        "invalid direction: 'sideways' (use out, in, both)"
    );

    let missing = fixture
        .cmd()
        .args(["graph", "n-nope", "--direction", "in"])
        .output()
        .expect("run mesh");
    assert_eq!(missing.status.code(), Some(3));
    assert_eq!(stderr_of(&missing).trim(), "seed not found: n-nope");
}

#[test]
fn graph_at_depth_zero_has_no_edges() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &["n-b"], "b");
    note(&fixture, "n-b", "B", "test-agent", &["n-a"], "b");
    let out = fixture
        .cmd()
        .args([
            "graph",
            "n-a",
            "--depth",
            "0",
            "--direction",
            "both",
            "--json",
        ])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    assert_eq!(ids_of(&payload["nodes"]), ["n-a"]);
    assert_eq!(payload["edges"], serde_json::json!([]));
}

#[test]
fn graph_matches_the_python_golden() {
    let fixture = corpus();
    let out = fixture
        .cmd()
        .args(["graph", "n-6YQY", "--json"])
        .output()
        .expect("run mesh");
    let got = json_of(&out);
    let want = golden("graph_n1.json");
    assert_eq!(got["seed"], want["seed"]);
    assert_eq!(got["edges"], want["edges"]);
    assert_eq!(normalise(&got["nodes"]), normalise(&want["nodes"]));
}

#[test]
fn graph_flag_placement_is_symmetric() {
    let fixture = corpus();
    assert_flag_parity(&fixture, &["graph", "n-6YQY"]);
    assert_flag_parity(&fixture, &["graph", "n-6YQY", "--direction", "both"]);
}

// --------------------------------------------------------------------------------------------
// project
// --------------------------------------------------------------------------------------------

#[test]
fn project_renders_the_note_then_indented_tasks() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-p", "Proj", "test-agent", &[], "b");
    task(
        &fixture,
        "t-a",
        "Scoped",
        "open",
        "test-agent",
        "",
        "n-p",
        &[],
        "b",
    );
    let out = fixture
        .cmd()
        .args(["project", "n-p"])
        .output()
        .expect("run mesh");
    let rows = lines(&out);
    assert_eq!(rows[0], "n-p\tnote\tProj");
    assert_eq!(rows[1], "  t-a\topen\tScoped");
}

#[test]
fn project_json_has_project_and_tasks() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-p", "Proj", "test-agent", &[], "b");
    task(
        &fixture,
        "t-a",
        "Scoped",
        "open",
        "test-agent",
        "",
        "n-p",
        &[],
        "b",
    );
    let out = fixture
        .cmd()
        .args(["project", "n-p", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    let keys: Vec<&str> = payload
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["project", "tasks"]);
    assert_eq!(payload["project"]["id"], Json::String("n-p".into()));
    assert_eq!(ids_of(&payload["tasks"]), ["t-a"]);
    assert!(payload["tasks"][0]["path"].as_str().is_some());
}

#[test]
fn project_quiet_lists_the_project_then_its_tasks() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-p", "Proj", "test-agent", &[], "b");
    task(
        &fixture,
        "t-a",
        "Scoped",
        "open",
        "test-agent",
        "",
        "n-p",
        &[],
        "b",
    );
    let out = fixture
        .cmd()
        .args(["project", "n-p", "--quiet"])
        .output()
        .expect("run mesh");
    assert_eq!(lines(&out), ["n-p", "t-a"]);
}

#[test]
fn project_returns_every_status_and_excludes_other_scopes() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-p", "Proj", "test-agent", &[], "b");
    task(
        &fixture,
        "t-open",
        "O",
        "open",
        "test-agent",
        "",
        "n-p",
        &[],
        "b",
    );
    task(
        &fixture,
        "t-done",
        "D",
        "done",
        "test-agent",
        "",
        "n-p",
        &[],
        "b",
    );
    task(
        &fixture,
        "t-other",
        "X",
        "open",
        "test-agent",
        "",
        "n-q",
        &[],
        "b",
    );
    task(
        &fixture,
        "t-none",
        "N",
        "open",
        "test-agent",
        "",
        "",
        &[],
        "b",
    );
    let out = fixture
        .cmd()
        .args(["project", "n-p", "--json"])
        .output()
        .expect("run mesh");
    let mut got = ids_of(&json_of(&out)["tasks"]);
    got.sort();
    assert_eq!(got, ["t-done", "t-open"]);
}

#[test]
fn project_with_no_tasks_is_still_a_result() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-p", "Proj", "test-agent", &[], "b");
    let out = fixture
        .cmd()
        .args(["project", "n-p", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_of(&out)["tasks"], serde_json::json!([]));
}

#[test]
fn project_reports_an_unknown_id_as_not_found() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["project", "n-nope"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stderr_of(&out).trim(), "project not found: n-nope");
}

#[test]
fn project_matches_the_python_golden() {
    let fixture = corpus();
    let out = fixture
        .cmd()
        .args(["project", "n-19EP", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(
        normalise(&json_of(&out)),
        normalise(&golden("project_p1.json"))
    );
}

#[test]
fn project_flag_placement_is_symmetric() {
    let fixture = corpus();
    assert_flag_parity(&fixture, &["project", "n-19EP"]);
}

// --------------------------------------------------------------------------------------------
// session-start
// --------------------------------------------------------------------------------------------

#[test]
fn session_start_rows_have_seven_columns_with_reason_third() {
    let fixture = VaultFixture::new();
    task(
        &fixture,
        "t-a",
        "Mine",
        "open",
        "test-agent",
        "",
        "",
        &[],
        "b",
    );
    let out = fixture
        .cmd()
        .args(["session-start"])
        .output()
        .expect("run mesh");
    let rows = lines(&out);
    assert!(rows[0].starts_with("t-a\ttask\ttask\ttest-agent\t-\tMine\t"));
    for row in &rows {
        assert_eq!(row.split('\t').count(), 7, "{row}");
    }
}

#[test]
fn session_start_orders_tasks_then_mentions_then_memories_then_activity() {
    let fixture = VaultFixture::new();
    task(
        &fixture,
        "t-mine",
        "Mine",
        "open",
        "test-agent",
        "",
        "",
        &[],
        "b",
    );
    note_at(
        &fixture,
        "n-peer",
        "Peer",
        "peer",
        &["t-mine"],
        "2099-01-01T00:00:00Z",
    );
    memory(&fixture, "m-a", "Fact", 5, "shared");
    note(&fixture, "n-loose", "Loose", "test-agent", &[], "b");
    let out = fixture
        .cmd()
        .args(["session-start", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    assert_eq!(ids_of(&payload), ["t-mine", "n-peer", "m-a", "n-loose"]);
    assert_eq!(
        strings(&payload, "reason"),
        ["task", "mention", "memory", "activity"]
    );
}

#[test]
fn session_start_keeps_only_live_tasks_in_the_task_section() {
    let fixture = VaultFixture::new();
    task(
        &fixture,
        "t-open",
        "O",
        "open",
        "test-agent",
        "",
        "",
        &[],
        "b",
    );
    task(
        &fixture,
        "t-claimed",
        "C",
        "claimed",
        "test-agent",
        "test-agent",
        "",
        &[],
        "b",
    );
    task(
        &fixture,
        "t-done",
        "D",
        "done",
        "test-agent",
        "",
        "",
        &[],
        "b",
    );
    let out = fixture
        .cmd()
        .args(["session-start", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    let entries = payload.as_array().expect("array");
    let task_ids: Vec<String> = entries
        .iter()
        .filter(|e| e["reason"] == Json::String("task".into()))
        .map(|e| e["id"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut sorted = task_ids.clone();
    sorted.sort();
    assert_eq!(sorted, ["t-claimed", "t-open"]);
    // A finished task of mine may still reappear as activity.
    assert!(entries
        .iter()
        .any(|e| e["id"] == Json::String("t-done".into())
            && e["reason"] == Json::String("activity".into())));
}

#[test]
fn session_start_reads_bodies_only_for_live_tasks() {
    let fixture = VaultFixture::new();
    task(
        &fixture,
        "t-a",
        "Mine",
        "open",
        "test-agent",
        "",
        "",
        &[],
        "the task body",
    );
    note_at(
        &fixture,
        "n-peer",
        "Peer",
        "peer",
        &["t-a"],
        "2099-01-01T00:00:00Z",
    );
    let out = fixture
        .cmd()
        .args(["session-start", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    let entries = payload.as_array().expect("array");
    assert_eq!(entries[0]["body"], Json::String("the task body".into()));
    let keys: Vec<&str> = entries[0]
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(&keys[keys.len() - 3..], ["path", "reason", "body"]);
    for entry in entries.iter().skip(1) {
        assert!(entry.get("body").is_none(), "{entry}");
    }
}

#[test]
fn session_start_meta_only_omits_every_body() {
    let fixture = VaultFixture::new();
    task(
        &fixture,
        "t-a",
        "Mine",
        "open",
        "test-agent",
        "",
        "",
        &[],
        "body text",
    );
    let out = fixture
        .cmd()
        .args(["session-start", "--meta-only", "--json"])
        .output()
        .expect("run mesh");
    for entry in json_of(&out).as_array().expect("array") {
        assert!(entry.get("body").is_none());
    }
}

#[test]
fn session_start_mentions_exclude_self_authored_and_stale_sources() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-mine", "Mine", "test-agent", &[], "target");
    note_at(
        &fixture,
        "n-peer",
        "Peer",
        "peer",
        &["n-mine"],
        "2099-01-01T00:00:00Z",
    );
    note_at(
        &fixture,
        "n-self",
        "Self",
        "test-agent",
        &["n-mine"],
        "2099-01-01T00:00:00Z",
    );
    note_at(
        &fixture,
        "n-stale",
        "Stale",
        "peer",
        &["n-mine"],
        "2000-01-01T00:00:00Z",
    );
    let out = fixture
        .cmd()
        .args(["session-start", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    let mentions: Vec<String> = payload
        .as_array()
        .expect("array")
        .iter()
        .filter(|e| e["reason"] == Json::String("mention".into()))
        .map(|e| e["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(mentions, ["n-peer"]);
}

#[test]
fn session_start_memories_are_capped_ranked_and_bodyless() {
    let fixture = VaultFixture::new();
    task(
        &fixture,
        "t-a",
        "Mine",
        "open",
        "test-agent",
        "",
        "",
        &[],
        "b",
    );
    for (index, importance) in [1, 2, 3, 4, 5, 5].iter().enumerate() {
        memory(
            &fixture,
            &format!("m-{index}"),
            &format!("Memory {index}"),
            *importance,
            "shared",
        );
    }
    let out = fixture
        .cmd()
        .args(["session-start", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    let picked: Vec<&Json> = payload
        .as_array()
        .expect("array")
        .iter()
        .filter(|e| e["reason"] == Json::String("memory".into()))
        .collect();
    assert_eq!(picked.len(), 5);
    assert_eq!(picked[0]["importance"], Json::from(5));
    for entry in &picked {
        assert!(entry.get("body").is_none());
        assert_eq!(entry["type"], Json::String("memory".into()));
    }
}

#[test]
fn session_start_no_memories_suppresses_the_section() {
    let fixture = VaultFixture::new();
    task(
        &fixture,
        "t-a",
        "Mine",
        "open",
        "test-agent",
        "",
        "",
        &[],
        "b",
    );
    memory(&fixture, "m-a", "Fact", 5, "shared");
    let out = fixture
        .cmd()
        .args(["session-start", "--no-memories", "--json"])
        .output()
        .expect("run mesh");
    assert!(!strings(&json_of(&out), "reason").contains(&"memory".to_string()));
}

#[test]
fn session_start_team_widens_only_the_activity_half() {
    let fixture = VaultFixture::new();
    task(
        &fixture,
        "t-mine",
        "Mine",
        "open",
        "test-agent",
        "",
        "",
        &[],
        "b",
    );
    task(&fixture, "t-peer", "Peer", "open", "peer", "", "", &[], "b");
    let mine = fixture
        .cmd()
        .args(["session-start", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&mine)), ["t-mine"]);

    let team = fixture
        .cmd()
        .args(["session-start", "--team", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&team);
    let mut ids = ids_of(&payload);
    ids.sort();
    assert_eq!(ids, ["t-mine", "t-peer"]);
    // The task half stays mine: the peer's task arrives as activity.
    let peer = payload
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["id"] == Json::String("t-peer".into()))
        .expect("peer entry");
    assert_eq!(peer["reason"], Json::String("activity".into()));
}

#[test]
fn session_start_owner_swaps_the_effective_identity() {
    let fixture = VaultFixture::new();
    task(&fixture, "t-peer", "Peer", "open", "peer", "", "", &[], "b");
    task(
        &fixture,
        "t-mine",
        "Mine",
        "open",
        "test-agent",
        "",
        "",
        &[],
        "b",
    );
    let out = fixture
        .cmd()
        .args(["session-start", "--owner", "peer", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    let peer = payload
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["id"] == Json::String("t-peer".into()))
        .expect("peer entry");
    assert_eq!(peer["reason"], Json::String("task".into()));
    // The same request on the root side is byte-identical.
    let global = fixture
        .cmd()
        .args(["--owner", "peer", "--json", "session-start"])
        .output()
        .expect("run mesh");
    assert_eq!(out.stdout, global.stdout);
}

#[test]
fn session_start_without_an_identity_claims_nothing() {
    let fixture =
        VaultFixture::with("[core]\nvault_path = \"{VAULT}\"\n\n[tasks]\ncollections = []\n");
    task(&fixture, "t-peer", "Peer", "open", "peer", "", "", &[], "b");
    note(&fixture, "n-peer", "Peer", "peer", &[], "b");
    let out = fixture
        .cmd()
        .args(["session-start", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out).trim(), "[]");
}

#[test]
fn session_start_budget_trims_and_reports_the_drop() {
    let fixture = VaultFixture::new();
    for id in ["t-a", "t-b", "t-c"] {
        task(
            &fixture,
            id,
            id,
            "open",
            "test-agent",
            "",
            "",
            &[],
            "a fairly long body that costs a good number of characters in the payload",
        );
    }
    let full = fixture
        .cmd()
        .args(["session-start", "--json"])
        .output()
        .expect("run mesh");
    let full_len = stdout_of(&full).trim().len();
    assert_eq!(ids_of(&json_of(&full)).len(), 3);

    let trimmed = fixture
        .cmd()
        .args(["session-start", "--budget", "400", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&trimmed);
    let entries = payload.as_array().expect("array");
    assert!(stdout_of(&trimmed).trim().len() < full_len);
    let last = entries.last().expect("an entry");
    if last["reason"] == Json::String("truncated".into()) {
        assert!(last["id"].is_null());
        assert_eq!(last["type"], Json::String("meta".into()));
        assert!(last["dropped"].as_u64().expect("dropped") >= 1);
    } else {
        // Under a budget that only bodies had to give up, no entry was dropped.
        assert!(entries.iter().any(|e| e.get("body").is_none()));
    }

    let tiny = fixture
        .cmd()
        .args(["session-start", "--budget", "30", "--json"])
        .output()
        .expect("run mesh");
    let payload = json_of(&tiny);
    let last = payload.as_array().expect("array").last().expect("entry");
    assert_eq!(last["reason"], Json::String("truncated".into()));
    assert_eq!(last["type"], Json::String("meta".into()));
    assert!(last["id"].is_null());
    assert!(last["dropped"].as_u64().expect("dropped") >= 1);
}

#[test]
fn session_start_quiet_lists_ids_and_json_wins() {
    let fixture = VaultFixture::new();
    task(
        &fixture,
        "t-a",
        "Mine",
        "open",
        "test-agent",
        "",
        "",
        &[],
        "b",
    );
    let quiet = fixture
        .cmd()
        .args(["session-start", "--quiet"])
        .output()
        .expect("run mesh");
    assert_eq!(lines(&quiet), ["t-a"]);
    let both = fixture
        .cmd()
        .args(["session-start", "--json", "--quiet"])
        .output()
        .expect("run mesh");
    assert_eq!(ids_of(&json_of(&both)), ["t-a"]);
}

#[test]
fn session_start_never_writes_to_stderr() {
    let fixture = corpus();
    for args in [
        vec!["session-start"],
        vec!["session-start", "--json"],
        vec!["session-start", "--team", "--meta-only", "--json"],
    ] {
        let out = fixture.cmd().args(&args).output().expect("run mesh");
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(stderr_of(&out).is_empty(), "{args:?}: {}", stderr_of(&out));
    }
}

#[test]
fn session_start_is_read_only() {
    let fixture = corpus();
    let before = snapshot(&fixture);
    fixture
        .cmd()
        .args(["session-start", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(before, snapshot(&fixture));
}

#[test]
fn session_start_matches_the_python_golden() {
    let fixture = corpus();
    apply_golden_mtimes(&fixture);
    let out = fixture
        .cmd()
        .args(["session-start", "--json"])
        .output()
        .expect("run mesh");
    let got = json_of(&out);
    let want = golden("session_start.json");
    assert_eq!(ids_of(&got), ids_of(&want));
    assert_eq!(strings(&got, "reason"), strings(&want, "reason"));
    assert_eq!(normalise(&got), normalise(&want));
}

#[test]
fn session_start_hook_command_is_the_shipped_one() {
    let hook = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("hooks/session_start.json");
    let text = std::fs::read_to_string(&hook).expect("read hook");
    assert!(text.contains("mesh session-start --meta-only --json"));
    let parsed: Json = serde_json::from_str(&text).expect("hook is JSON");
    let entries = parsed["hooks"]["SessionStart"]
        .as_array()
        .expect("SessionStart array");
    assert_eq!(entries.len(), 1);
    let inner = entries[0]["hooks"].as_array().expect("hooks array");
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0]["type"], Json::String("command".into()));

    // And the command the hook ships actually runs.
    let fixture = corpus();
    let out = fixture
        .cmd()
        .args(["session-start", "--meta-only", "--json"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(0));
    assert!(json_of(&out).is_array());
}

#[test]
fn session_start_flag_placement_is_symmetric() {
    let fixture = corpus();
    apply_golden_mtimes(&fixture);
    assert_flag_parity(&fixture, &["session-start"]);
    assert_flag_parity(&fixture, &["session-start", "--meta-only"]);
}

#[test]
fn session_start_rejects_an_unknown_space() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["session-start", "--space", "nope"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr_of(&out).starts_with("invalid space: 'nope'"));
}

// --------------------------------------------------------------------------------------------
// status (the payload is the lens half of `mesh status`)
// --------------------------------------------------------------------------------------------

#[test]
fn status_on_an_empty_vault_has_the_pinned_key_order() {
    let fixture = VaultFixture::new();
    let out = fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(0));
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
            "notes",
            "tasks",
            "tasks_total",
            "freshness",
            "dangling_links",
            "stale_locks",
            "vault",
            "daemon",
            "agents",
            "dangling_links_total",
            "memories",
            "scratch",
            "assets",
            "deps",
            "spaces",
            "watcher",
        ]
    );
    assert_eq!(payload["notes"], Json::from(0));
    assert_eq!(
        payload["tasks"],
        serde_json::json!({"open": 0, "claimed": 0, "done": 0, "cancelled": 0})
    );
    assert!(payload["freshness"]["mtime"].is_null());
    assert_eq!(payload["agents"], serde_json::json!({}));
    assert_eq!(
        payload["daemon"],
        serde_json::json!({"running": false, "pid": null})
    );
    assert_eq!(
        payload["watcher"],
        serde_json::json!({"running": false, "pid": null})
    );
    assert_eq!(
        payload["deps"],
        serde_json::json!({"blocked": 0, "ready": 0, "cycles": [], "dangling_blockers": 0})
    );
}

#[test]
fn status_human_block_renders_every_group() {
    let fixture = corpus();
    let out = fixture.cmd().args(["status"]).output().expect("run mesh");
    let text = stdout_of(&out);
    for expected in [
        "notes: 8",
        "tasks: open=3 claimed=1 done=1 cancelled=1",
        "dangling links: 1 (Missing Title)",
        "stale locks: 1",
        "daemon: stopped",
        "agents:",
        "  demo-agent: open=3 claimed=1 stale=0",
        "memories: total=0 expired=0 superseded=0",
        "scratch: files=0 agents=0",
        "assets: count=0 bytes=0 orphan_blobs=0",
        "deps: blocked=1 ready=2 cycles=0 dangling_blockers=0",
        "spaces:",
        "watcher: stopped",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    assert!(text.contains("s ago"));
}

#[test]
fn status_on_the_corpus_matches_the_python_golden_counts() {
    let fixture = corpus();
    let out = fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run mesh");
    let got = json_of(&out);
    let want = golden("status.json");
    for key in ["notes", "tasks", "tasks_total", "dangling_links", "agents"] {
        assert_eq!(got[key], want[key], "{key}");
    }
    assert_eq!(got["dangling_links"], serde_json::json!(["Missing Title"]));
    assert_eq!(got["dangling_links_total"], Json::from(1));
    let locks = got["stale_locks"].as_array().expect("array");
    assert_eq!(locks.len(), 1);
    assert!(locks[0]
        .as_str()
        .expect("string")
        .ends_with("tasks/.locks/t-STAL.lock"));
    assert_eq!(got["vault"]["exists"], Json::Bool(true));
    assert!(got["freshness"]["mtime"].is_f64());
    assert!(got["freshness"]["age_seconds"].as_f64().expect("age") >= 0.0);
}

#[test]
fn status_reports_a_missing_vault_without_failing() {
    let fixture = VaultFixture::with(
        "[core]\nvault_path = \"{VAULT}/gone\"\nagent = \"test-agent\"\n\n[tasks]\ncollections = []\n",
    );
    let out = fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(0));
    let payload = json_of(&out);
    assert_eq!(payload["vault"]["exists"], Json::Bool(false));
    assert_eq!(payload["notes"], Json::from(0));
    assert_eq!(payload["stale_locks"], serde_json::json!([]));

    let human = fixture.cmd().args(["status"]).output().expect("run mesh");
    assert!(stdout_of(&human).contains("(does not exist)"));
}

#[test]
fn status_counts_memories_scratch_and_deps() {
    let fixture = VaultFixture::new();
    memory(&fixture, "m-a", "Fact", 3, "shared");
    memory(&fixture, "m-b", "Other", 3, "private");
    fixture.write(
        "scratch/test-agent/notes.md",
        "---\ntype: scratch\nname: notes\nagent: test-agent\ntags: []\n\
         created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\n---\n\nscratch\n",
    );
    task(&fixture, "t-a", "A", "open", "test-agent", "", "", &[], "b");
    fixture.write(
        "tasks/open/t-b.md",
        "---\nid: t-b\ntype: task\ntitle: B\ntags: []\nowner: test-agent\n\
         created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated: []\n\
         status: open\npriority: null\nclaimed_by: null\nproject: null\nblocks: []\n\
         blocked_by:\n  - t-a\n  - t-ghost\n---\n\nb\n",
    );
    let out = fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    assert_eq!(payload["memories"]["total"], Json::from(2));
    assert_eq!(payload["scratch"]["files"], Json::from(1));
    assert_eq!(payload["scratch"]["agents"], Json::from(1));
    assert_eq!(payload["deps"]["blocked"], Json::from(1));
    assert_eq!(payload["deps"]["ready"], Json::from(1));
    assert_eq!(payload["deps"]["dangling_blockers"], Json::from(1));
    assert_eq!(
        payload["assets"],
        serde_json::json!({"count": 0, "bytes": 0, "orphan_blobs": 0})
    );
}

#[test]
fn status_lists_a_stale_lock_but_not_a_live_one() {
    let fixture = VaultFixture::new();
    note(&fixture, "n-a", "A", "test-agent", &[], "b");
    fixture.write("notes/.locks/n-dead.lock", "999999");
    fixture.write("notes/.locks/n-live.lock", &std::process::id().to_string());
    let out = fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run mesh");
    let locks = json_of(&out)["stale_locks"]
        .as_array()
        .expect("array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(locks.len(), 1);
    assert!(locks[0].ends_with("n-dead.lock"));
}

#[test]
fn status_caps_the_dangling_link_list_at_fifty() {
    let fixture = VaultFixture::new();
    let mut body = String::new();
    for index in 0..60 {
        body.push_str(&format!("[[Ghost {index}]] "));
    }
    note(&fixture, "n-a", "A", "test-agent", &[], &body);
    let out = fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    assert_eq!(
        payload["dangling_links"].as_array().expect("array").len(),
        50
    );
    assert_eq!(payload["dangling_links_total"], Json::from(60));
}

#[test]
fn status_is_read_only() {
    let fixture = corpus();
    let before = snapshot(&fixture);
    fixture.cmd().args(["status"]).output().expect("run mesh");
    fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run mesh");
    assert_eq!(before, snapshot(&fixture));
}

#[test]
fn status_agent_rows_register_owners_and_claimers() {
    let fixture = VaultFixture::new();
    task(&fixture, "t-a", "A", "open", "alice", "", "", &[], "b");
    task(
        &fixture,
        "t-b",
        "B",
        "claimed",
        "alice",
        "bob",
        "",
        &[],
        "b",
    );
    task(&fixture, "t-c", "C", "done", "carol", "", "", &[], "b");
    let out = fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run mesh");
    let agents = json_of(&out)["agents"].clone();
    let names: Vec<&str> = agents
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(names, ["alice", "bob", "carol"]);
    assert_eq!(agents["alice"]["owns_open"], Json::from(1));
    assert_eq!(agents["bob"]["claimed"], Json::from(1));
    assert_eq!(
        agents["carol"],
        serde_json::json!({"owns_open": 0, "claimed": 0, "stale_claims": 0})
    );
}

#[test]
fn status_counts_a_stale_claim() {
    let fixture = VaultFixture::new();
    fixture.write(
        "tasks/open/t-old.md",
        "---\nid: t-old\ntype: task\ntitle: Old\ntags: []\nowner: alice\n\
         created: 2020-01-01T00:00:00Z\nupdated: 2020-01-01T00:00:00Z\nrelated: []\n\
         status: claimed\npriority: null\nclaimed_by: alice\nproject: null\n\
         blocks: []\nblocked_by: []\n---\n\nold\n",
    );
    let out = fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run mesh");
    assert_eq!(
        json_of(&out)["agents"]["alice"]["stale_claims"],
        Json::from(1)
    );
}

#[test]
fn status_reports_a_hand_made_cycle() {
    let fixture = VaultFixture::new();
    for (id, blocker) in [("t-a", "t-b"), ("t-b", "t-a")] {
        fixture.write(
            &format!("tasks/open/{id}.md"),
            &format!(
                "---\nid: {id}\ntype: task\ntitle: {id}\ntags: []\nowner: test-agent\n\
                 created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated: []\n\
                 status: open\npriority: null\nclaimed_by: null\nproject: null\nblocks: []\n\
                 blocked_by:\n  - {blocker}\n---\n\nx\n"
            ),
        );
    }
    let out = fixture
        .cmd()
        .args(["--json", "status"])
        .output()
        .expect("run mesh");
    let payload = json_of(&out);
    assert!(!payload["deps"]["cycles"]
        .as_array()
        .expect("array")
        .is_empty());
    assert_eq!(payload["deps"]["ready"], Json::from(0));
    assert_eq!(payload["deps"]["blocked"], Json::from(2));
    let human = fixture.cmd().args(["status"]).output().expect("run mesh");
    assert!(stdout_of(&human).contains("cycles=1"));
}
