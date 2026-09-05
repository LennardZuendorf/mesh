//! `mesh search` contract: routing, the two engines, the tag pull, `--health`, the `indexed`
//! subprocess, and the class-S output rules.

mod common;

use std::path::{Path, PathBuf};

use common::{golden_dir, VaultFixture};
use serde_json::Value as Json;

// ---------------------------------------------------------------- fixtures

/// A vault spanning every space, plus a foreign file, a corrupt file, a nested task that must
/// not be walked, and a scratch note that is out of the default corpus.
fn seeded_with(cfg: &str) -> VaultFixture {
    let f = VaultFixture::with(cfg);
    f.write(
        "notes/n-AAAA.md",
        "---\nid: n-AAAA\ntype: note\ntitle: Alpha Note\ntags:\n  - a\n  - b\n\
         owner: demo-agent\ncreated: 2026-01-01T00:00:00Z\nupdated: 2026-06-01T00:00:00Z\n---\n\n\
         Alpha body mentions zebra here.\n",
    );
    f.write(
        "notes/logs/n-BBBB.md",
        "---\nid: n-BBBB\ntype: log\ntitle: Beta Log\ntags:\n  - a\n\
         owner: other-agent\ncreated: 2026-01-01T00:00:00Z\nupdated: 2026-05-01T00:00:00Z\n---\n\n\
         # Heading\n\nLog entry about the zebra migration.\n",
    );
    f.write(
        "notes/foreign.md",
        "# Foreign Heading\n\nA foreign markdown file with no frontmatter. zebra appears here.\n",
    );
    f.write("notes/n-BAAD.md", "---\ntitle: [unclosed\n---\n\nbroken\n");
    f.write(
        "tasks/open/t-CCCC.md",
        "---\nid: t-CCCC\ntype: task\ntitle: Task One\nstatus: open\ntags:\n  - a\n\
         owner: demo-agent\nupdated: 2026-04-01T00:00:00Z\n---\n\nDo the zebra thing.\n",
    );
    f.write(
        "tasks/done/t-DDDD.md",
        "---\nid: t-DDDD\ntype: task\ntitle: Task Two\nstatus: done\n\
         updated: 2026-03-01T00:00:00Z\n---\n\nFinished.\n",
    );
    f.write(
        "tasks/open/nested/t-EEEE.md",
        "---\nid: t-EEEE\ntype: task\ntitle: Nested Task\nstatus: open\n\
         updated: 2026-09-01T00:00:00Z\n---\n\nnested zebra\n",
    );
    f.write(
        "memories/m-FFFF.md",
        "---\nid: m-FFFF\ntype: memory\ntitle: Memory One\nkind: fact\nscope: shared\n\
         updated: 2026-07-01T00:00:00Z\n---\n\nThe operator prefers zebra stripes.\n",
    );
    f.write(
        "assets/a-GGGG.md",
        "---\nid: a-GGGG\ntype: asset\ntitle: Asset One\nfilename: z.png\n\
         updated: 2026-02-01T00:00:00Z\n---\n\nA zebra picture.\n",
    );
    f.write(
        "scratch/agent/x.md",
        "---\ntype: scratch\nname: x\nagent: agent\nupdated: 2026-08-01T00:00:00Z\n---\n\n\
         scratch zebra state\n",
    );
    f
}

fn seeded() -> VaultFixture {
    seeded_with(common::DEFAULT_CONFIG)
}

/// A Python-era vault: `notes/` and `tasks/` and nothing else.
fn legacy() -> VaultFixture {
    let f = VaultFixture::new();
    f.write(
        "notes/n-AAAA.md",
        "---\nid: n-AAAA\ntype: note\ntitle: Alpha Note\ntags:\n  - a\n\
         updated: 2026-06-01T00:00:00Z\n---\n\nzebra body\n",
    );
    f.write(
        "tasks/open/t-CCCC.md",
        "---\nid: t-CCCC\ntype: task\ntitle: Task One\nstatus: open\n\
         updated: 2026-04-01T00:00:00Z\n---\n\nzebra task\n",
    );
    f
}

/// A config whose `[search]` block opens every hybrid gate.
fn hybrid_config() -> String {
    format!(
        "{}\n[search]\ncollection = \"test-vault\"\nhybrid = true\n",
        common::DEFAULT_CONFIG
    )
}

/// Install one of `tests/fixtures/fake-indexed/*.sh` as the `indexed` on the fixture's PATH.
fn install_fake(f: &VaultFixture, script: &str) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fake-indexed")
        .join(script);
    let dest = f.dir.path().join("bin").join("indexed");
    std::fs::copy(&source, &dest).expect("install fake indexed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake indexed");
    }
}

// ---------------------------------------------------------------- helpers

fn run(f: &VaultFixture, args: &[&str]) -> std::process::Output {
    f.cmd().args(args).output().expect("run mesh")
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn hits(f: &VaultFixture, args: &[&str]) -> Vec<Json> {
    let out = run(f, args);
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.ends_with('\n'), "payload must end in a newline");
    assert_eq!(text.lines().count(), 1, "payload must be one line: {text}");
    match serde_json::from_str::<Json>(text.trim_end()).expect("payload is json") {
        Json::Array(items) => items,
        other => panic!("payload must be an array, got {other}"),
    }
}

fn ids(hits: &[Json]) -> Vec<String> {
    hits.iter()
        .map(|h| match &h["id"] {
            Json::String(s) => s.clone(),
            _ => "<foreign>".to_string(),
        })
        .collect()
}

fn keys(hit: &Json) -> Vec<String> {
    hit.as_object()
        .expect("hit is an object")
        .keys()
        .cloned()
        .collect()
}

fn score(hit: &Json) -> f64 {
    hit["score"].as_f64().expect("numeric score")
}

fn hit_with<'a>(hits: &'a [Json], id: &str) -> &'a Json {
    hits.iter()
        .find(|h| h["id"] == Json::String(id.to_string()))
        .unwrap_or_else(|| panic!("no hit {id}"))
}

const FALLBACK_NOTICE: &str = "search: using substring fallback (indexed unavailable)";

// ---------------------------------------------------------------- tag pull

#[test]
fn a_bare_search_is_a_tag_pull_over_the_default_spaces() {
    let f = seeded();
    let got = hits(&f, &["search"]);
    assert_eq!(
        ids(&got),
        [
            "m-FFFF",
            "n-AAAA",
            "n-BBBB",
            "t-CCCC",
            "t-DDDD",
            "a-GGGG",
            "<foreign>"
        ]
    );
}

#[test]
fn a_tag_pull_scores_one_and_carries_no_snippet() {
    let f = seeded();
    for hit in hits(&f, &["search"]) {
        assert_eq!(score(&hit), 1.0);
        assert!(!keys(&hit).contains(&"snippet".to_string()));
    }
}

#[test]
fn tag_pull_tags_are_anded() {
    let f = seeded();
    assert_eq!(ids(&hits(&f, &["search", "--tags", "a"])).len(), 3);
    assert_eq!(
        ids(&hits(&f, &["search", "--tags", "a", "--tags", "b"])),
        ["n-AAAA"]
    );
}

#[test]
fn tag_pull_tags_are_case_sensitive() {
    let f = seeded();
    assert!(hits(&f, &["search", "--tags", "A"]).is_empty());
}

#[test]
fn tag_pull_limit_zero_is_empty_and_negative_is_unbounded() {
    let f = seeded();
    assert!(hits(&f, &["search", "--limit", "0"]).is_empty());
    assert_eq!(hits(&f, &["search", "--limit=-1"]).len(), 7);
    assert_eq!(hits(&f, &["search", "--limit", "2"]).len(), 2);
}

#[test]
fn tag_pull_filters_by_type_owner_status_and_kind() {
    let f = seeded();
    assert_eq!(ids(&hits(&f, &["search", "--type", "log"])), ["n-BBBB"]);
    assert_eq!(
        ids(&hits(&f, &["search", "--owner", "other-agent"])),
        ["n-BBBB"]
    );
    assert_eq!(ids(&hits(&f, &["search", "--status", "done"])), ["t-DDDD"]);
    assert_eq!(ids(&hits(&f, &["search", "--kind", "fact"])), ["m-FFFF"]);
}

#[test]
fn a_global_owner_filters_the_tag_pull() {
    let f = seeded();
    let out = run(&f, &["--owner", "other-agent", "search"]);
    assert_eq!(out.status.code(), Some(0));
    let got: Json = serde_json::from_str(stdout_of(&out).trim_end()).expect("json");
    assert_eq!(got.as_array().map(Vec::len), Some(1));
}

#[test]
fn the_tag_pull_matches_the_python_golden_semantically() {
    let f = VaultFixture::from_corpus();
    let got = hits(&f, &["search", "--tags", "a"]);
    let golden: Json = serde_json::from_str(
        &std::fs::read_to_string(golden_dir().join("search_tags_a.json")).expect("golden"),
    )
    .expect("golden json");
    let expected = golden.as_array().cloned().unwrap_or_default();
    assert_eq!(got.len(), expected.len());
    for (mine, theirs) in got.iter().zip(expected.iter()) {
        for key in ["id", "type", "title", "score", "tags", "owner", "updated"] {
            assert_eq!(mine[key], theirs[key], "key {key}");
        }
        assert!(mine["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("n-6YQY.md")));
        assert!(!keys(mine).contains(&"snippet".to_string()));
    }
}

// ---------------------------------------------------------------- corpus

#[test]
fn the_corpus_reaches_notes_subfolders_and_both_task_folders() {
    let f = seeded();
    let got = ids(&hits(&f, &["search", "--limit=-1"]));
    for id in ["n-BBBB", "t-CCCC", "t-DDDD"] {
        assert!(got.contains(&id.to_string()), "{id} missing from {got:?}");
    }
}

#[test]
fn the_task_folders_are_walked_non_recursively() {
    let f = seeded();
    let got = ids(&hits(&f, &["search", "--limit=-1"]));
    assert!(!got.contains(&"t-EEEE".to_string()), "{got:?}");
}

#[test]
fn scratch_is_out_of_the_default_corpus_but_opts_in_by_flag() {
    let f = seeded();
    let default = hits(&f, &["search", "zebra", "--limit=-1", "--quiet"]);
    assert!(default
        .iter()
        .all(|h| h["type"] != Json::String("scratch".into())));
    let opted = hits(
        &f,
        &[
            "search",
            "zebra",
            "--space",
            "scratch",
            "--quiet",
            "--threshold",
            "0.1",
        ],
    );
    assert_eq!(opted.len(), 1);
    assert_eq!(opted[0]["type"], Json::String("scratch".into()));
}

#[test]
fn a_corrupt_file_is_skipped_never_an_error() {
    let f = seeded();
    let got = hits(&f, &["search", "--limit=-1"]);
    assert!(got
        .iter()
        .all(|h| h["title"] != Json::String("broken".into())));
}

#[test]
fn a_foreign_file_surfaces_with_a_null_identity() {
    let f = seeded();
    let got = hits(&f, &["search", "--limit=-1"]);
    let foreign = got
        .iter()
        .find(|h| h["id"] == Json::Null)
        .expect("foreign hit");
    assert_eq!(foreign["type"], Json::Null);
    assert_eq!(foreign["title"], Json::Null);
    assert!(foreign["path"]
        .as_str()
        .is_some_and(|p| p.ends_with("foreign.md")));
}

#[test]
fn space_narrows_the_corpus() {
    let f = seeded();
    assert_eq!(
        ids(&hits(&f, &["search", "--space", "memories"])),
        ["m-FFFF"]
    );
    assert_eq!(
        ids(&hits(&f, &["search", "--space", "tasks"])),
        ["t-CCCC", "t-DDDD"]
    );
    assert_eq!(
        ids(&hits(&f, &["search", "--space", "memories,assets"])),
        ["m-FFFF", "a-GGGG"]
    );
}

#[test]
fn an_unknown_space_is_exit_two() {
    let f = seeded();
    let out = run(&f, &["search", "--space", "nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out).trim_end(),
        "invalid space: 'nope' (use notes, tasks, memories, scratch, assets)"
    );
}

#[test]
fn a_disabled_space_is_exit_two() {
    let cfg = format!("{}\n[spaces]\nscratch = false\n", common::DEFAULT_CONFIG);
    let f = seeded_with(&cfg);
    let out = run(&f, &["search", "--space", "scratch"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out).trim_end(),
        "space 'scratch' is disabled in [spaces]"
    );
}

// ---------------------------------------------------------------- the space key

#[test]
fn a_legacy_vault_emits_the_legacy_key_set() {
    let f = legacy();
    for hit in hits(&f, &["search"]) {
        assert!(!keys(&hit).contains(&"space".to_string()), "{hit}");
    }
}

#[test]
fn a_vault_with_a_non_legacy_space_emits_the_space_key() {
    let f = seeded();
    let got = hits(&f, &["search"]);
    assert_eq!(
        hit_with(&got, "m-FFFF")["space"],
        Json::String("memories".into())
    );
    assert_eq!(
        hit_with(&got, "n-AAAA")["space"],
        Json::String("notes".into())
    );
    assert_eq!(
        hit_with(&got, "t-CCCC")["space"],
        Json::String("tasks".into())
    );
}

#[test]
fn an_explicit_space_flag_always_emits_the_space_key() {
    let f = legacy();
    let got = hits(&f, &["search", "--space", "notes"]);
    assert_eq!(got[0]["space"], Json::String("notes".into()));
}

// ---------------------------------------------------------------- the engines

#[test]
fn a_query_finds_the_body_hit_and_the_foreign_file() {
    let f = VaultFixture::from_corpus();
    let got = ids(&hits(&f, &["search", "zebra", "--quiet", "--limit=-1"]));
    assert!(got.contains(&"n-2CC3".to_string()), "{got:?}");
    assert!(got.contains(&"<foreign>".to_string()), "{got:?}");
}

#[test]
fn the_substring_engine_reproduces_the_legacy_tiers() {
    let f = seeded();
    let got = hits(
        &f,
        &[
            "search",
            "alpha note",
            "--engine",
            "substring",
            "--limit=-1",
        ],
    );
    assert_eq!(ids(&got), ["n-AAAA"]);
    assert_eq!(score(&got[0]), 1.0);
    let sub = hits(
        &f,
        &["search", "alpha", "--engine", "substring", "--limit=-1"],
    );
    assert_eq!(score(hit_with(&sub, "n-AAAA")), 0.8);
}

#[test]
fn the_substring_engine_snippet_is_the_head_of_the_body() {
    let f = seeded();
    let got = hits(&f, &["search", "zebra", "--engine", "substring"]);
    assert_eq!(
        hit_with(&got, "n-AAAA")["snippet"],
        Json::String("Alpha body mentions zebra here.".into())
    );
}

#[test]
fn the_builtin_engine_ranks_a_multi_word_query_the_substring_scan_would_miss() {
    let f = seeded();
    let legacy = hits(
        &f,
        &[
            "search",
            "zebra entry",
            "--engine",
            "substring",
            "--limit=-1",
            "--threshold",
            "0.1",
        ],
    );
    assert!(legacy.is_empty(), "{legacy:?}");
    let ranked = hits(
        &f,
        &[
            "search",
            "zebra entry",
            "--engine",
            "builtin",
            "--limit=-1",
            "--threshold",
            "0.1",
        ],
    );
    assert!(ids(&ranked).contains(&"n-BBBB".to_string()), "{ranked:?}");
}

#[test]
fn the_ranker_never_loses_a_hit_the_substring_scan_returns() {
    let f = seeded();
    let legacy = ids(&hits(
        &f,
        &["search", "zebra", "--engine", "substring", "--limit=-1"],
    ));
    let ranked = ids(&hits(
        &f,
        &["search", "zebra", "--engine", "builtin", "--limit=-1"],
    ));
    for id in &legacy {
        assert!(ranked.contains(id), "{id} lost: {ranked:?}");
    }
}

#[test]
fn an_invalid_engine_is_exit_two() {
    let f = seeded();
    let out = run(&f, &["search", "x", "--engine", "magic"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr_of(&out).trim_end(),
        "invalid engine: 'magic' (use auto, indexed, builtin, substring)"
    );
}

// ---------------------------------------------------------------- threshold

#[test]
fn an_unset_threshold_reaches_the_body_tier() {
    let f = seeded();
    let got = ids(&hits(
        &f,
        &["search", "zebra", "--engine", "substring", "--limit=-1"],
    ));
    assert!(got.contains(&"n-AAAA".to_string()), "{got:?}");
}

#[test]
fn the_threshold_flag_drops_strictly_below_and_keeps_equal() {
    let f = seeded();
    let kept = hits(
        &f,
        &[
            "search",
            "zebra",
            "--engine",
            "substring",
            "--threshold",
            "0.4",
            "--limit=-1",
        ],
    );
    assert!(!kept.is_empty());
    let dropped = hits(
        &f,
        &[
            "search",
            "zebra",
            "--engine",
            "substring",
            "--threshold",
            "0.41",
            "--limit=-1",
        ],
    );
    assert!(dropped.is_empty(), "{dropped:?}");
}

#[test]
fn an_explicit_config_threshold_applies_and_the_flag_beats_it() {
    let cfg = format!("{}\n[search]\nthreshold = 0.65\n", common::DEFAULT_CONFIG);
    let f = seeded_with(&cfg);
    assert!(hits(
        &f,
        &["search", "zebra", "--engine", "substring", "--limit=-1"]
    )
    .is_empty());
    assert!(!hits(
        &f,
        &[
            "search",
            "zebra",
            "--engine",
            "substring",
            "--threshold",
            "0.1",
            "--limit=-1"
        ]
    )
    .is_empty());
}

#[test]
fn an_explicit_threshold_advises_once_on_the_ranker_and_never_on_substring() {
    let cfg = format!("{}\n[search]\nthreshold = 0.65\n", common::DEFAULT_CONFIG);
    let f = seeded_with(&cfg);
    let advisory = "search: explicit [search].threshold applies to the ranker's normalised \
                    score (--engine substring for the legacy tiers)";
    let ranked = stderr_of(&run(&f, &["search", "zebra", "--engine", "builtin"]));
    assert_eq!(ranked.matches(advisory).count(), 1, "{ranked}");
    let legacy = stderr_of(&run(&f, &["search", "zebra", "--engine", "substring"]));
    assert!(!legacy.contains(advisory), "{legacy}");
    let quiet = stderr_of(&run(
        &f,
        &["search", "zebra", "--engine", "builtin", "--quiet"],
    ));
    assert_eq!(quiet, "");
}

#[test]
fn a_default_config_never_advises() {
    let f = seeded();
    let err = stderr_of(&run(&f, &["search", "zebra", "--engine", "builtin"]));
    assert_eq!(err, "");
}

// ---------------------------------------------------------------- snippets

#[test]
fn meta_only_drops_the_snippet_and_beats_full() {
    let f = seeded();
    for args in [
        ["search", "zebra", "--meta-only", "--quiet"].as_slice(),
        ["search", "zebra", "--meta-only", "--full", "--quiet"].as_slice(),
    ] {
        for hit in hits(&f, args) {
            assert!(!keys(&hit).contains(&"snippet".to_string()), "{hit}");
        }
    }
}

#[test]
fn full_overloads_snippet_with_the_whole_body_and_adds_no_body_key() {
    let f = seeded();
    let got = hits(&f, &["search", "zebra", "--full", "--quiet", "--limit=-1"]);
    let hit = hit_with(&got, "n-BBBB");
    assert_eq!(
        hit["snippet"],
        Json::String("# Heading\n\nLog entry about the zebra migration.".into())
    );
    assert!(!keys(hit).contains(&"body".to_string()));
}

#[test]
fn the_hit_key_order_is_fixed() {
    let f = seeded();
    let got = hits(&f, &["search", "zebra", "--quiet", "--limit=-1"]);
    assert_eq!(
        keys(hit_with(&got, "n-AAAA")),
        ["id", "type", "title", "score", "path", "tags", "owner", "updated", "snippet", "space"]
    );
}

#[test]
fn optional_keys_are_omitted_when_empty() {
    let f = seeded();
    let got = hits(&f, &["search", "zebra", "--quiet", "--limit=-1"]);
    let tagless = hit_with(&got, "m-FFFF");
    assert!(!keys(tagless).contains(&"tags".to_string()));
    assert!(!keys(tagless).contains(&"owner".to_string()));
}

// ---------------------------------------------------------------- health

#[test]
fn health_reports_no_collection_by_default() {
    let f = seeded();
    let out = run(&f, &["search", "--health"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout_of(&out);
    assert_eq!(text.lines().count(), 1);
    let json: Json = serde_json::from_str(text.trim_end()).expect("json");
    assert_eq!(
        json.as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>()),
        Some(vec![
            "mode".into(),
            "hybrid_configured".into(),
            "collection".into(),
            "daemon_up".into(),
            "indexed_binary_available".into(),
            "reason".into(),
        ])
    );
    assert_eq!(json["mode"], Json::String("fallback".into()));
    assert_eq!(json["collection"], Json::Null);
    assert_eq!(
        json["reason"],
        Json::String("no collection configured ([search].collection unset)".into())
    );
}

#[test]
fn health_reports_hybrid_disabled_first() {
    let cfg = format!(
        "{}\n[search]\nhybrid = false\ncollection = \"c\"\n",
        common::DEFAULT_CONFIG
    );
    let f = seeded_with(&cfg);
    f.fake_indexed("");
    let out = run(&f, &["search", "--health"]);
    let json: Json = serde_json::from_str(stdout_of(&out).trim_end()).expect("json");
    assert_eq!(
        json["reason"],
        Json::String("hybrid disabled ([search].hybrid = false)".into())
    );
    assert_eq!(json["collection"], Json::String("c".into()));
    assert_eq!(json["indexed_binary_available"], Json::Bool(true));
}

#[test]
fn health_reports_a_missing_binary_last() {
    let f = seeded_with(&hybrid_config());
    let out = run(&f, &["search", "--health"]);
    let json: Json = serde_json::from_str(stdout_of(&out).trim_end()).expect("json");
    assert_eq!(
        json["reason"],
        Json::String("indexed binary not found on PATH".into())
    );
    assert_eq!(json["indexed_binary_available"], Json::Bool(false));
}

#[test]
fn health_reports_indexed_when_every_gate_is_open() {
    let f = seeded_with(&hybrid_config());
    f.fake_indexed("");
    let out = run(&f, &["search", "--health"]);
    let json: Json = serde_json::from_str(stdout_of(&out).trim_end()).expect("json");
    assert_eq!(json["mode"], Json::String("indexed".into()));
    assert_eq!(json.as_object().map(|o| o.len()), Some(5));
    assert!(json.get("reason").is_none());
}

#[test]
fn health_short_circuits_a_query_and_never_shells_indexed() {
    let f = seeded_with(&hybrid_config());
    f.fake_indexed("{\"path\":\"/nope.md\",\"score\":0.9}");
    let out = run(&f, &["search", "zebra", "--health"]);
    let json: Json = serde_json::from_str(stdout_of(&out).trim_end()).expect("json");
    assert_eq!(json["mode"], Json::String("indexed".into()));
    assert!(f.indexed_argv().is_empty(), "{:?}", f.indexed_argv());
}

#[test]
fn health_reports_watcher_liveness_as_daemon_up() {
    let f = seeded();
    let out = run(&f, &["search", "--health"]);
    let json: Json = serde_json::from_str(stdout_of(&out).trim_end()).expect("json");
    assert_eq!(json["daemon_up"], Json::Bool(false));
}

// ---------------------------------------------------------------- the indexed path

fn ndjson_for(f: &VaultFixture, rows: &[(&str, f64, Option<&str>)]) -> String {
    rows.iter()
        .map(|(rel, score, snippet)| {
            let path = f.vault.join(rel);
            match snippet {
                Some(s) => format!(
                    "{{\"path\": \"{}\", \"score\": {score}, \"snippet\": \"{s}\"}}",
                    path.display()
                ),
                None => format!("{{\"path\": \"{}\", \"score\": {score}}}", path.display()),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_indexed_search_argv_is_byte_exact() {
    let f = seeded_with(&hybrid_config());
    f.fake_indexed(&ndjson_for(&f, &[("notes/n-AAAA.md", 0.9, Some("s"))]));
    let got = hits(&f, &["search", "hello world", "--limit", "5"]);
    assert_eq!(ids(&got), ["n-AAAA"]);
    assert_eq!(
        f.indexed_argv(),
        ["index search hello world --collection test-vault --json --limit 5"]
    );
}

#[test]
fn indexed_hits_carry_the_external_score_and_snippet() {
    let f = seeded_with(&hybrid_config());
    f.fake_indexed(&ndjson_for(
        &f,
        &[("notes/n-AAAA.md", 0.91, Some("ranked"))],
    ));
    let got = hits(&f, &["search", "zebra"]);
    assert_eq!(score(&got[0]), 0.91);
    assert_eq!(got[0]["snippet"], Json::String("ranked".into()));
}

#[test]
fn indexed_hits_are_re_filtered_against_the_conjunctive_filters() {
    let f = seeded_with(&hybrid_config());
    f.fake_indexed(&ndjson_for(
        &f,
        &[
            ("notes/n-AAAA.md", 0.9, None),
            ("notes/logs/n-BBBB.md", 0.8, None),
        ],
    ));
    assert_eq!(
        ids(&hits(&f, &["search", "zebra", "--tags", "b"])),
        ["n-AAAA"]
    );
    assert_eq!(
        ids(&hits(&f, &["search", "zebra", "--type", "log"])),
        ["n-BBBB"]
    );
}

#[test]
fn the_indexed_path_defaults_to_the_config_threshold_not_the_engine_floor() {
    let f = seeded_with(&hybrid_config());
    // 0.5 clears the built-in engine's 0.4 floor but not `[search].threshold`'s 0.65, which is
    // what the `indexed` path defaults to when no threshold was made explicit.
    f.fake_indexed(&ndjson_for(&f, &[("notes/n-AAAA.md", 0.5, None)]));
    assert!(hits(&f, &["search", "zebra"]).is_empty());
    assert_eq!(
        ids(&hits(&f, &["search", "zebra", "--threshold", "0.4"])),
        ["n-AAAA"]
    );
}

#[test]
fn indexed_hits_below_the_threshold_are_dropped() {
    let f = seeded_with(&hybrid_config());
    f.fake_indexed(&ndjson_for(
        &f,
        &[
            ("notes/n-AAAA.md", 0.9, None),
            ("notes/logs/n-BBBB.md", 0.1, None),
        ],
    ));
    assert_eq!(
        ids(&hits(&f, &["search", "zebra", "--threshold", "0.5"])),
        ["n-AAAA"]
    );
}

#[test]
fn an_indexed_hit_outside_the_sandbox_is_dropped() {
    let f = seeded_with(&hybrid_config());
    let outside = f.dir.path().join("outside.md");
    std::fs::write(&outside, "---\nid: n-OUT\n---\n\nx\n").expect("write outside");
    f.fake_indexed(&format!(
        "{{\"path\": \"{}\", \"score\": 0.9}}\n{}",
        outside.display(),
        ndjson_for(&f, &[("notes/n-AAAA.md", 0.9, None)])
    ));
    assert_eq!(ids(&hits(&f, &["search", "zebra"])), ["n-AAAA"]);
}

#[test]
fn an_indexed_hit_whose_file_vanished_is_dropped() {
    let f = seeded_with(&hybrid_config());
    f.fake_indexed(&ndjson_for(
        &f,
        &[("notes/gone.md", 0.9, None), ("notes/n-AAAA.md", 0.9, None)],
    ));
    assert_eq!(ids(&hits(&f, &["search", "zebra"])), ["n-AAAA"]);
}

#[test]
fn the_indexed_path_orders_by_the_epsilon_comparator() {
    let f = seeded_with(&hybrid_config());
    // n-BBBB scores lower but is inside the 0.02 band and… older, so n-AAAA still wins;
    // n-AAAA is the more recently updated of the pair.
    f.fake_indexed(&ndjson_for(
        &f,
        &[
            ("notes/logs/n-BBBB.md", 0.91, None),
            ("notes/n-AAAA.md", 0.90, None),
        ],
    ));
    assert_eq!(ids(&hits(&f, &["search", "zebra"])), ["n-AAAA", "n-BBBB"]);
}

#[test]
fn the_indexed_path_prefers_score_outside_the_band() {
    let f = seeded_with(&hybrid_config());
    f.fake_indexed(&ndjson_for(
        &f,
        &[
            ("notes/logs/n-BBBB.md", 0.95, None),
            ("notes/n-AAAA.md", 0.70, None),
        ],
    ));
    assert_eq!(ids(&hits(&f, &["search", "zebra"])), ["n-BBBB", "n-AAAA"]);
}

#[test]
fn malformed_ndjson_lines_are_skipped_and_the_query_continues() {
    let f = seeded_with(&hybrid_config());
    let good = ndjson_for(&f, &[("notes/n-AAAA.md", 0.9, None)]);
    f.fake_indexed(&format!(
        "\n{{not json}}\n{{\"path\": \"/x.md\"}}\n{{\"path\": \"/x.md\", \"score\": true}}\n{good}"
    ));
    assert_eq!(ids(&hits(&f, &["search", "zebra"])), ["n-AAAA"]);
}

#[test]
fn the_indexed_path_emits_no_degradation_notice() {
    let f = seeded_with(&hybrid_config());
    f.fake_indexed(&ndjson_for(&f, &[("notes/n-AAAA.md", 0.9, None)]));
    let out = run(&f, &["search", "zebra"]);
    assert_eq!(stderr_of(&out), "");
}

#[test]
fn a_failing_indexed_degrades_to_the_builtin_engine() {
    let f = seeded_with(&hybrid_config());
    install_fake(&f, "fail.sh");
    let out = run(&f, &["search", "zebra"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stderr_of(&out).contains(FALLBACK_NOTICE),
        "{}",
        stderr_of(&out)
    );
    let got: Json = serde_json::from_str(stdout_of(&out).trim_end()).expect("json");
    assert!(!got.as_array().map(Vec::is_empty).unwrap_or(true));
}

#[test]
fn a_hanging_indexed_times_out_and_degrades() {
    let f = seeded_with(&hybrid_config());
    install_fake(&f, "hang.sh");
    let started = std::time::Instant::now();
    let out = f
        .cmd()
        .env("MESH_INDEXED_TIMEOUT_MS", "250")
        .args(["search", "zebra"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(0));
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    assert!(stderr_of(&out).contains(FALLBACK_NOTICE));
    let got: Json = serde_json::from_str(stdout_of(&out).trim_end()).expect("json");
    assert!(!got.as_array().map(Vec::is_empty).unwrap_or(true));
}

#[test]
fn engine_builtin_never_shells_indexed() {
    let f = seeded_with(&hybrid_config());
    f.fake_indexed(&ndjson_for(&f, &[("notes/n-AAAA.md", 0.9, None)]));
    let _ = hits(&f, &["search", "zebra", "--engine", "builtin"]);
    assert!(f.indexed_argv().is_empty());
}

#[test]
fn engine_indexed_shells_indexed_even_with_hybrid_off() {
    let cfg = format!(
        "{}\n[search]\ncollection = \"test-vault\"\nhybrid = false\n",
        common::DEFAULT_CONFIG
    );
    let f = seeded_with(&cfg);
    f.fake_indexed(&ndjson_for(&f, &[("notes/n-AAAA.md", 0.9, None)]));
    assert_eq!(
        ids(&hits(&f, &["search", "zebra", "--engine", "indexed"])),
        ["n-AAAA"]
    );
    assert_eq!(f.indexed_argv().len(), 1);
}

// ---------------------------------------------------------------- notices and output class

#[test]
fn the_degradation_notice_is_emitted_once_even_with_zero_hits() {
    let f = seeded();
    let out = run(&f, &["search", "nothing-matches-this-at-all"]);
    assert_eq!(stdout_of(&out), "[]\n");
    assert_eq!(stderr_of(&out).matches(FALLBACK_NOTICE).count(), 1);
}

#[test]
fn quiet_suppresses_only_the_notice_never_the_payload() {
    let f = seeded();
    let loud = run(&f, &["search", "zebra"]);
    let quiet = run(&f, &["search", "zebra", "--quiet"]);
    assert_eq!(stdout_of(&loud), stdout_of(&quiet));
    assert!(stderr_of(&loud).contains(FALLBACK_NOTICE));
    assert_eq!(stderr_of(&quiet), "");
}

#[test]
fn an_explicit_builtin_engine_is_not_a_degradation() {
    let f = seeded();
    for engine in ["builtin", "substring"] {
        let out = run(&f, &["search", "zebra", "--engine", engine]);
        assert_eq!(stderr_of(&out), "", "engine {engine}");
    }
}

#[test]
fn a_tag_pull_never_emits_the_degradation_notice() {
    let f = seeded();
    assert_eq!(stderr_of(&run(&f, &["search", "--tags", "a"])), "");
}

#[test]
fn json_is_inert_and_accepted_on_either_side_of_the_command_name() {
    let f = seeded();
    let left = run(&f, &["--json", "search", "zebra", "--quiet"]);
    let right = run(&f, &["search", "zebra", "--json", "--quiet"]);
    let plain = run(&f, &["search", "zebra", "--quiet"]);
    assert_eq!(stdout_of(&left), stdout_of(&right));
    assert_eq!(stdout_of(&left), stdout_of(&plain));
}

#[test]
fn quiet_is_accepted_on_either_side_of_the_command_name() {
    let f = seeded();
    let left = run(&f, &["--quiet", "search", "zebra"]);
    let right = run(&f, &["search", "zebra", "--quiet"]);
    assert_eq!(stdout_of(&left), stdout_of(&right));
    assert_eq!(stderr_of(&left), "");
    assert_eq!(stderr_of(&right), "");
}

#[test]
fn owner_is_accepted_on_either_side_of_the_command_name() {
    let f = seeded();
    let left = run(&f, &["--owner", "other-agent", "search"]);
    let right = run(&f, &["search", "--owner", "other-agent"]);
    assert_eq!(stdout_of(&left), stdout_of(&right));
}

#[test]
fn zero_hits_is_an_empty_array_and_exit_zero() {
    let f = seeded();
    let out = run(&f, &["search", "--tags", "no-such-tag"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out), "[]\n");
}

#[test]
fn a_missing_config_is_exit_two_with_the_three_line_message() {
    let f = seeded();
    let out = f
        .bare_cmd()
        .args(["search", "zebra"])
        .output()
        .expect("run mesh");
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr_of(&out).starts_with("mesh: no config found at "));
}

#[test]
fn search_help_lists_the_whole_flag_surface() {
    let f = seeded();
    let out = run(&f, &["search", "--help"]);
    let text = stdout_of(&out);
    for flag in [
        "--type",
        "--tags",
        "--owner",
        "--status",
        "--kind",
        "--space",
        "--engine",
        "--limit",
        "--threshold",
        "--meta-only",
        "--full",
        "--health",
    ] {
        assert!(text.contains(flag), "{flag} missing from help");
    }
}

#[test]
fn a_repeated_space_flag_value_is_deduped() {
    let f = seeded();
    assert_eq!(
        ids(&hits(&f, &["search", "--space", "memories,memories"])),
        ["m-FFFF"]
    );
}

#[test]
fn the_payload_path_is_the_file_on_disk() {
    let f = seeded();
    let got = hits(&f, &["search", "--space", "memories"]);
    let path = PathBuf::from(got[0]["path"].as_str().expect("path string"));
    assert!(path.is_file(), "{path:?}");
}
