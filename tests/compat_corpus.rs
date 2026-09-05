//! The compat corpus: a Python-written vault, read back and re-serialised.
//!
//! This asserts SEMANTIC fidelity, not bytes (overrides.md O2): mesh writes its own canonical
//! frontmatter, so a re-dump is deliberately not byte-identical to PyYAML's output. What must
//! hold is that every value survives, unknown keys round-trip, and a dump reloads unchanged.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{corpus_files, golden_dir, VaultFixture};
use mesh::fm::{dump_doc, parse_meta, read_doc, read_meta_only, Doc, Meta, Value};
use mesh::model::common::BASE_FIELDS;
use mesh::model::task::TASK_FIELDS;
use mesh::render::entry;

/// The one file in the corpus whose frontmatter is deliberately unparseable.
const CORRUPT: &str = "n-BAD.md";
/// The one file in the corpus with no frontmatter at all.
const FOREIGN: &str = "foreign.md";

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn golden(file: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(golden_dir().join(file)).expect("golden file");
    serde_json::from_str(&text).expect("golden json")
}

fn golden_by_id(file: &str) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    if let Some(items) = golden(file).as_array() {
        for item in items {
            if let Some(id) = item.get("id").and_then(serde_json::Value::as_str) {
                out.insert(id.to_string(), item.clone());
            }
        }
    }
    out
}

#[test]
fn every_corpus_file_loads_except_the_deliberately_corrupt_one() {
    let files = corpus_files();
    assert_eq!(files.len(), 16, "the corpus has 16 markdown files");
    for path in files {
        let doc = read_doc(&path);
        if name(&path) == CORRUPT {
            assert!(doc.is_none(), "{} must fail to parse", path.display());
            continue;
        }
        let doc = doc.unwrap_or_else(|| panic!("{} must parse", path.display()));
        if name(&path) == FOREIGN {
            assert!(doc.meta.is_empty(), "foreign markdown has no frontmatter");
            assert!(doc.body.starts_with("# Foreign Heading"));
        } else {
            assert!(doc.meta.contains_key("id"), "{} has an id", path.display());
        }
        assert!(
            read_meta_only(&path).is_some(),
            "{} meta-only read",
            path.display()
        );
    }
}

#[test]
fn ids_txt_names_files_that_exist() {
    let listed = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ids.txt"),
    )
    .expect("ids.txt");
    let stems: Vec<String> = corpus_files()
        .iter()
        .map(|p| {
            p.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    for token in listed.split_whitespace() {
        let Some((label, id)) = token.split_once('=') else {
            continue;
        };
        assert!(
            stems.contains(&id.to_string()),
            "{label}={id} is missing from the corpus"
        );
    }
}

#[test]
fn typed_note_values_match_the_python_reference() {
    let reference = golden_by_id("note_list.json");
    for path in corpus_files() {
        let Some(meta) = read_meta_only(&path) else {
            continue;
        };
        let Some(id) = meta.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(want) = reference.get(id) else {
            continue;
        };
        let mut got = entry(&meta, BASE_FIELDS, None, None);
        let mut want = want.clone();
        // Documented deviation: YAML 1.2 scalar resolution keeps `nested: yes` a string,
        // where PyYAML's 1.1 resolution made it a boolean.
        if let Some(object) = got.as_object_mut() {
            object.remove("extra");
        }
        if let Some(object) = want.as_object_mut() {
            assert_eq!(
                object.remove("extra"),
                Some(serde_json::json!({"nested": true})).filter(|_| id == "n-HAND")
            );
        }
        assert_eq!(got, want, "note {id}");
    }
}

#[test]
fn typed_task_values_match_the_python_reference() {
    let reference = golden_by_id("task_list.json");
    assert_eq!(reference.len(), 6);
    for path in corpus_files() {
        let Some(meta) = read_meta_only(&path) else {
            continue;
        };
        let Some(id) = meta.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(want) = reference.get(id) else {
            continue;
        };
        assert_eq!(
            entry(&meta, TASK_FIELDS.fields(), None, None),
            *want,
            "task {id}"
        );
    }
}

#[test]
fn a_free_form_legacy_priority_survives_a_read() {
    let meta =
        read_meta_only(&common::corpus_dir().join("tasks/open/t-LEGP.md")).expect("legacy task");
    assert_eq!(meta.get("priority").and_then(Value::as_str), Some("urgent"));
}

#[test]
fn load_dump_load_is_stable_for_every_file() {
    for path in corpus_files() {
        let Some(doc) = read_doc(&path) else {
            continue;
        };
        let dumped = dump_doc(&doc);
        let (yaml, body) = mesh::fm::split_frontmatter(&dumped);
        let reloaded = match yaml {
            Some(block) => parse_meta(&block).expect("reparse"),
            None => Meta::new(),
        };
        assert_eq!(reloaded, doc.meta, "meta round-trip for {}", path.display());
        assert_eq!(body, doc.body, "body round-trip for {}", path.display());
        let again = dump_doc(&Doc::new(reloaded, body));
        assert_eq!(again, dumped, "dump is idempotent for {}", path.display());
    }
}

#[test]
fn unknown_keys_and_the_literal_extra_key_survive() {
    let doc = read_doc(&common::corpus_dir().join("notes/n-HAND.md")).expect("hand-written note");
    for key in ["aliases", "custom_key", "extra", "quoted_date", "offset_ts"] {
        assert!(doc.meta.contains_key(key), "{key} survives the load");
    }
    assert_eq!(
        doc.meta.get("custom_key").and_then(Value::as_str),
        Some("keep me")
    );
    // `extra` is an ordinary unknown key: stashed, never bound to a field.
    assert!(matches!(doc.meta.get("extra"), Some(Value::Map(_))));
    let dumped = dump_doc(&doc);
    let reloaded = read_meta_from(&dumped);
    assert_eq!(reloaded, doc.meta);
    let keys: Vec<&str> = reloaded.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "aliases",
            "created",
            "custom_key",
            "extra",
            "id",
            "owner",
            "related",
            "tags",
            "title",
            "type",
            "updated",
            "quoted_date",
            "offset_ts"
        ],
        "an untouched file keeps its original key order"
    );
}

fn read_meta_from(document: &str) -> Meta {
    let (yaml, _) = mesh::fm::split_frontmatter(document);
    parse_meta(&yaml.expect("frontmatter")).expect("parse")
}

#[test]
fn awkward_scalars_round_trip_through_the_emitter() {
    let cases: Vec<(&str, Value)> = vec![
        ("plain", Value::str("plain")),
        ("colon", Value::str("a: b")),
        ("hash", Value::str("#x")),
        ("yes_like", Value::str("yes")),
        ("null_like", Value::str("null")),
        ("int_like", Value::str("12")),
        ("float_like", Value::str("1.5")),
        ("empty", Value::str("")),
        ("date_like", Value::str("2026-01-02")),
        ("unicode", Value::str("Ünicöde Tîtle héllo ✓")),
        ("long", Value::str("x".repeat(400))),
        ("long_words", Value::str(
            "This is a very long title that keeps going and going past eighty characters for sure yes indeed",
        )),
        ("multiline", Value::str("two\nlines")),
        ("bare_date", parse_scalar("2026-01-02")),
        ("naive_dt", parse_scalar("2026-01-02T03:04:05")),
        ("offset_dt", parse_scalar("2026-01-02 03:04:05+02:00")),
        ("utc_dt", parse_scalar("2026-09-05 07:27:02.307028+00:00")),
        ("nothing", Value::Null),
        ("flag", Value::Bool(true)),
        ("number", Value::Int(42)),
        ("ratio", Value::Float(0.5)),
        ("empty_list", Value::List(vec![])),
        ("list", Value::strings(["a", "b"])),
        ("nested", Value::Map(one("k", Value::str("v")))),
    ];
    let mut meta = Meta::new();
    for (key, value) in &cases {
        meta.insert((*key).to_string(), value.clone());
    }
    let doc = Doc::new(meta.clone(), "body");
    let dumped = dump_doc(&doc);
    let reloaded = read_meta_from(&dumped);
    assert_eq!(reloaded, meta, "dumped:\n{dumped}");
    assert_eq!(dump_doc(&Doc::new(reloaded, "body")), dumped);
}

fn one(key: &str, value: Value) -> Meta {
    let mut meta = Meta::new();
    meta.insert(key.to_string(), value);
    meta
}

fn parse_scalar(text: &str) -> Value {
    let meta = parse_meta(&format!("k: {text}\n")).expect("scalar parses");
    meta.get("k").cloned().expect("scalar present")
}

#[test]
fn an_append_changes_only_updated_related_and_the_block() {
    let path = common::corpus_dir().join("notes/n-6YQY.md");
    let before = read_doc(&path).expect("alpha note");
    let mut after = before.clone();
    after.body = mesh::text::append_to_end(&before.body, "A new paragraph.");
    after.meta.insert(
        "updated".to_string(),
        mesh::model::common::ts_value(&mesh::timefmt::now_utc()),
    );
    after
        .meta
        .insert("related".to_string(), Value::strings(["n-2CC3"]));

    let changed: Vec<&String> = after
        .meta
        .keys()
        .filter(|k| before.meta.get(*k) != after.meta.get(*k))
        .collect();
    assert_eq!(
        changed,
        ["updated"],
        "related was already n-2CC3, so only updated moved"
    );
    assert!(after.body.starts_with(&before.body));
    assert!(after.body.ends_with("A new paragraph."));
    assert_eq!(before.meta.keys().len(), after.meta.keys().len());
}

#[test]
fn a_read_never_rewrites_a_file() {
    let fixture = VaultFixture::from_corpus();
    let before: Vec<(String, u64, std::time::SystemTime)> = fixture
        .files()
        .into_iter()
        .filter_map(|rel| {
            let meta = std::fs::metadata(fixture.vault.join(&rel)).ok()?;
            Some((rel, meta.len(), meta.modified().ok()?))
        })
        .collect();
    for (rel, _, _) in &before {
        let _ = read_doc(&fixture.vault.join(rel));
        let _ = read_meta_only(&fixture.vault.join(rel));
    }
    for (rel, len, modified) in &before {
        let meta = std::fs::metadata(fixture.vault.join(rel)).expect("still there");
        assert_eq!(meta.len(), *len, "{rel} changed size");
        assert_eq!(
            meta.modified().expect("mtime"),
            *modified,
            "{rel} was touched"
        );
    }
}

#[test]
fn the_stale_lock_and_the_dot_directory_survive_a_corpus_copy() {
    let fixture = VaultFixture::from_corpus();
    let files = fixture.files();
    assert!(files.iter().any(|f| f == "tasks/.locks/t-STAL.lock"));
    assert!(files.iter().any(|f| f == "notes/foreign.md"));
    assert!(files.iter().any(|f| f == "notes/n-BAD.md"));
    let lock = fixture.vault.join("tasks/.locks/t-STAL.lock");
    assert!(
        mesh::storage::is_stale(&lock),
        "pid 999999 is dead, so the lock is stale"
    );
}

#[test]
fn the_walk_skips_dot_directories_and_finds_every_markdown_file() {
    let fixture = VaultFixture::from_corpus();
    let notes: Vec<String> = mesh::storage::iter_md(&fixture.vault.join("notes"), true, &[])
        .filter_map(|p| {
            p.strip_prefix(&fixture.vault)
                .ok()
                .map(|r| r.to_string_lossy().into_owned())
        })
        .collect();
    assert_eq!(
        notes,
        [
            "notes/decisions/n-1B3G.md",
            "notes/foreign.md",
            "notes/logs/n-2CC3.md",
            "notes/n-6YQY.md",
            "notes/n-BAD.md",
            "notes/n-HAND.md",
            "notes/n-NYM7.md",
            "notes/n-Y62G.md",
            "notes/projects/n-19EP.md",
            "notes/references/n-1506.md",
        ]
    );
    let open: Vec<String> = mesh::storage::iter_md(&fixture.vault.join("tasks/open"), false, &[])
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    assert_eq!(open, ["t-1FN1.md", "t-D0YQ.md", "t-LEGP.md", "t-TCY1.md"]);
}

#[test]
fn wikilinks_resolve_against_the_corpus_title_index() {
    let fixture = VaultFixture::from_corpus();
    let cfg = mesh::config::load_config(Some(&fixture.config), None).expect("config");
    let index = mesh::domain::title_index(&cfg);
    assert_eq!(index.get("Beta Note").map(String::as_str), Some("n-2CC3"));
    assert_eq!(index.get("Alpha Note").map(String::as_str), Some("n-6YQY"));
    assert!(
        !index.contains_key("Foreign Heading"),
        "foreign files never shadow a link"
    );
    assert_eq!(
        mesh::domain::resolve_wikilinks(&cfg, "[[Beta Note]] and [[Missing Title]]"),
        ["n-2CC3"]
    );
    assert_eq!(mesh::domain::find_dangling(&cfg), ["Missing Title"]);
}
