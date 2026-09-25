//! The `indexed` subprocess wrapper: three argv forms, one JSON envelope in, hits out.
//!
//! Agent content is data, never shell input — every invocation is a direct `execve` with an
//! argument vector, never a shell string. Machine output is requested through the
//! `INDEXED_SIMPLE_OUTPUT` environment variable on the child (the CLI's `--simple-output`
//! flag is global-only), so the argv never carries an output flag. A missing binary, a
//! non-zero exit or a hung child degrades to the built-in engine with the standard notice;
//! none of them is an error.

use std::cmp::Ordering;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value as Json;

use crate::config::{Config, ENV_INDEXED_BIN};
use crate::domain::select::matches_filters;
use crate::fm::read_doc;
use crate::model::common::{meta_str, meta_strings, meta_time};
use crate::search::corpus::{base_filter, space_of};
use crate::search::{Hit, SearchFilter};
use crate::spaces::Space;
use crate::storage::safe_resolve;

/// The binary looked up on `PATH` when `$MESH_INDEXED_BIN` is unset.
pub const INDEXED_BIN: &str = "indexed";

/// The wall clock allowed per **search** invocation (deviation 12). Overridable for tests
/// through `$MESH_INDEXED_TIMEOUT_MS`; a value of `0` or an unparsable one falls back to this.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// The wall clock allowed per **ingest** invocation — `index create`/`index update`, which
/// embed and index a whole vault and are far slower than a query. `$MESH_INDEXED_TIMEOUT_MS`
/// still overrides it.
pub const INGEST_TIMEOUT_MS: u64 = 600_000;

/// The environment variable that overrides both wall clocks, in milliseconds.
pub const ENV_TIMEOUT_MS: &str = "MESH_INDEXED_TIMEOUT_MS";

/// The environment variable that puts the `indexed` CLI in machine-readable mode — its
/// `--simple-output` flag is global-only, so the child is configured through the environment
/// and the argv stays free of output flags.
pub const ENV_SIMPLE_OUTPUT: &str = "INDEXED_SIMPLE_OUTPUT";

/// How often the parent checks on a running child.
const POLL: Duration = Duration::from_millis(2);

/// Scores within this band are ties, and the more recently updated file wins.
pub const TIEBREAK_EPSILON: f64 = 0.02;

/// Why an `indexed` invocation did not produce output. Every variant degrades, never errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// No binary on `PATH` and no `$MESH_INDEXED_BIN`.
    Missing,
    /// Spawned but exited non-zero, or could not be spawned.
    Failed,
    /// Still running when the wall clock ran out; the child was killed.
    Timeout,
    /// `index update <C>` reported the collection absent. The only existence probe the
    /// shipped CLI offers, so it is what routes `mesh reindex` to `index create`.
    MissingCollection,
}

/// One decoded NDJSON line.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexedHit {
    pub path: String,
    pub score: f64,
    pub snippet: Option<String>,
}

/// The `indexed` binary to run: `$MESH_INDEXED_BIN` when non-empty, else a `PATH` lookup.
///
/// This never executes the binary — it is the only "is indexed installed" detection there is.
pub fn binary() -> Option<PathBuf> {
    if let Ok(value) = std::env::var(ENV_INDEXED_BIN) {
        if !value.is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    which::which(INDEXED_BIN).ok()
}

/// Whether an `indexed` binary is reachable. A pure lookup.
pub fn available() -> bool {
    binary().is_some()
}

/// The wall clock for a search invocation, `$MESH_INDEXED_TIMEOUT_MS` or 30 s.
pub fn timeout() -> Duration {
    resolve_timeout(DEFAULT_TIMEOUT_MS)
}

/// The wall clock for an ingest invocation (`index create`/`index update`), the same
/// environment override or 600 s.
pub fn ingest_timeout() -> Duration {
    resolve_timeout(INGEST_TIMEOUT_MS)
}

/// `$MESH_INDEXED_TIMEOUT_MS` when set to a positive integer, else `default_ms`.
fn resolve_timeout(default_ms: u64) -> Duration {
    let ms = std::env::var(ENV_TIMEOUT_MS)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default_ms);
    Duration::from_millis(ms)
}

/// `indexed index search <query> --collection <C> --limit <N>`.
pub fn search_argv(query: &str, collection: &str, limit: i64) -> Vec<String> {
    vec![
        "index".to_string(),
        "search".to_string(),
        query.to_string(),
        "--collection".to_string(),
        collection.to_string(),
        "--limit".to_string(),
        limit.to_string(),
    ]
}

/// `indexed index update <C>` — a refresh is per collection; the CLI has no path-level update.
pub fn update_argv(collection: &str) -> Vec<String> {
    vec![
        "index".to_string(),
        "update".to_string(),
        collection.to_string(),
    ]
}

/// `indexed index create files --path <root> --collection <C>`.
pub fn create_argv(root: &Path, collection: &str) -> Vec<String> {
    vec![
        "index".to_string(),
        "create".to_string(),
        "files".to_string(),
        "--path".to_string(),
        root.display().to_string(),
        "--collection".to_string(),
        collection.to_string(),
    ]
}

/// Run `indexed` with `argv` on the **search** wall clock and return its stdout.
///
/// stdout is drained by a helper thread so a child that outruns the pipe buffer cannot
/// deadlock the parent; stderr is discarded, as the Python client discarded it.
pub fn run(argv: &[String]) -> Result<String, Failure> {
    run_with_timeout(argv, timeout())
}

/// Run an **ingest** invocation (`index create`/`index update`) on the ingest wall clock.
pub fn run_ingest(argv: &[String]) -> Result<String, Failure> {
    run_with_timeout(argv, ingest_timeout())
}

/// Whether a failed child's output is the shipped CLI's missing-collection error.
///
/// `index update <C>` on an absent collection is the only existence probe the CLI offers
/// (there is no "does this collection exist" query); it exits non-zero with the machine-output
/// envelope `{"status":"error","error":"Collection 'X' not found"}` on stdout.
fn is_missing_collection(text: &str) -> bool {
    matches!(
        serde_json::from_str::<Json>(text),
        Ok(Json::Object(obj))
            if matches!(obj.get("error"), Some(Json::String(e)) if e.contains("not found"))
    )
}

/// Run `indexed` with `argv` under `budget` and return its stdout.
fn run_with_timeout(argv: &[String], budget: Duration) -> Result<String, Failure> {
    let Some(bin) = binary() else {
        return Err(Failure::Missing);
    };
    let mut child = Command::new(&bin)
        .args(argv)
        .env(ENV_SIMPLE_OUTPUT, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Failure::Missing
            } else {
                Failure::Failed
            }
        })?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut handle) = stdout {
            let _ = handle.read_to_string(&mut buf);
        }
        buf
    });
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = reader.join().unwrap_or_default();
                return if status.success() {
                    Ok(text)
                } else if is_missing_collection(&text) {
                    Err(Failure::MissingCollection)
                } else {
                    Err(Failure::Failed)
                };
            }
            Ok(None) => {}
            Err(_) => return Err(Failure::Failed),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::Timeout);
        }
        std::thread::sleep(POLL);
    }
}

/// Decode the `--simple-output` envelope: one JSON document, chunk-level `results`.
///
/// Tolerantly: a payload that is not a JSON object, or whose `results` is not an array, is
/// no hits; an entry without a string `document_url` or a numeric score is skipped and
/// decoding continues; a **boolean score is rejected** (`true` must not become `1.0`); an
/// integer score coerces to float; unknown keys are ignored; an absent or null `text` is
/// `None` and a wrong-typed one skips the entry. Scores are normalised to higher-is-better:
/// a v2 entry's `relevance` is used as-is, a v1 entry carries only `relevance_score`
/// (squared L2, lower is better) and is mapped through `1/(1+s)` onto the same scale.
/// Chunks of one document are deduplicated to the best chunk, so one document is one hit.
pub fn parse_envelope(text: &str) -> Vec<IndexedHit> {
    let Ok(Json::Object(obj)) = serde_json::from_str::<Json>(text) else {
        return Vec::new();
    };
    let Some(Json::Array(entries)) = obj.get("results") else {
        return Vec::new();
    };
    let mut out: Vec<IndexedHit> = Vec::new();
    for entry in entries {
        let Json::Object(entry) = entry else {
            continue;
        };
        let Some(Json::String(url)) = entry.get("document_url") else {
            continue;
        };
        let path = url
            .strip_prefix("file://")
            .map(str::to_string)
            .unwrap_or_else(|| url.clone());
        let score = match entry.get("relevance").and_then(Json::as_f64) {
            Some(relevance) => relevance,
            None => {
                let Some(Json::Number(raw)) = entry.get("relevance_score") else {
                    continue;
                };
                let Some(raw) = raw.as_f64() else {
                    continue;
                };
                1.0 / (1.0 + raw.max(0.0))
            }
        };
        let snippet = match entry.get("text") {
            None | Some(Json::Null) => None,
            Some(Json::String(s)) => Some(s.clone()),
            Some(_) => continue,
        };
        match out.iter_mut().find(|hit| hit.path == path) {
            // A document may match through several chunks: keep the best one, so one
            // document is one hit with its strongest score and that chunk's text.
            Some(existing) => {
                if score > existing.score {
                    existing.score = score;
                    existing.snippet = snippet;
                }
            }
            None => out.push(IndexedHit {
                path,
                score,
                snippet,
            }),
        }
    }
    out
}

/// The epsilon comparator, reproduced literally.
///
/// It is **not** a total order (it is not transitive across chained epsilon bands). That is
/// the Python behaviour and we do not "fix" it into a derived `Ord`; a stable sort makes the
/// result deterministic for any given input order.
pub fn compare(a: &Hit, b: &Hit) -> Ordering {
    if (a.score - b.score).abs() <= TIEBREAK_EPSILON {
        if a.updated != b.updated {
            return if a.updated > b.updated {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        if a.score != b.score {
            return if a.score > b.score {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        // `tech.md`: ordering is score desc, updated desc, **path asc**. The path arm was
        // missing, so two hits with the same score and the same `updated` kept whatever
        // order `indexed` emitted them in. It only refines ties; no other result moves.
        return a.path.cmp(&b.path);
    }
    if a.score > b.score {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

/// Whether any post-fetch filter is active, so the fetch must not be pre-truncated.
fn filters_any(f: &SearchFilter) -> bool {
    !f.tags.is_empty()
        || f.type_filter.is_some()
        || f.status.is_some()
        || f.kind.is_some()
        || f.owner.is_some()
}

/// Query `indexed`, then sandbox-check, re-read and filter every hit it returned.
///
/// The emitted `path` is the sandbox-resolved realpath, the `score` is `indexed`'s own rank
/// and the `snippet` is `indexed`'s snippet (which may be absent).
pub fn search(
    cfg: &Config,
    collection: &str,
    query: &str,
    f: &SearchFilter,
    threshold: f64,
) -> Result<Vec<Hit>, Failure> {
    // Every hit `indexed` returns is re-read and re-filtered below, so asking it for exactly
    // `--limit N` under an active filter under-returns: the rows that would have filled the
    // page were never fetched. An active filter forces an unbounded fetch and the limit is
    // applied afterwards as a display cap — the same shape `recent_activity_in` uses.
    let fetch_limit = if filters_any(f) { -1 } else { f.limit };
    let raw = run(&search_argv(query, collection, fetch_limit))?;
    let filter = base_filter(f);
    let fallback = f.spaces.first().copied().unwrap_or(Space::Notes);
    let mut hits: Vec<Hit> = Vec::new();
    for hit in parse_envelope(&raw) {
        if hit.score < threshold {
            continue;
        }
        let Ok(path) = safe_resolve(&cfg.spaces, Path::new(&hit.path)) else {
            continue;
        };
        let Some(doc) = read_doc(&path) else {
            continue;
        };
        if !matches_filters(&doc.meta, &filter) {
            continue;
        }
        let space = space_of(cfg, &path).unwrap_or(fallback);
        hits.push(Hit {
            id: meta_str(&doc.meta, "id").map(str::to_string),
            r#type: meta_str(&doc.meta, "type").map(str::to_string),
            title: meta_str(&doc.meta, "title").map(str::to_string),
            score: hit.score,
            tags: meta_strings(&doc.meta, "tags"),
            owner: meta_str(&doc.meta, "owner").map(str::to_string),
            updated: meta_time(&doc.meta, "updated"),
            snippet: hit.snippet,
            path,
            space,
        });
    }
    hits.sort_by(compare);
    Ok(hits)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use std::path::PathBuf;

    fn hit(score: f64, updated: Option<&str>) -> Hit {
        Hit {
            id: None,
            r#type: None,
            title: None,
            score,
            tags: Vec::new(),
            owner: None,
            updated: updated.map(|t| t.parse::<DateTime<Utc>>().unwrap()),
            snippet: None,
            path: PathBuf::from("/v/x.md"),
            space: Space::Notes,
        }
    }

    #[test]
    fn search_argv_is_byte_exact() {
        assert_eq!(
            search_argv("hello world", "test-vault", 5),
            [
                "index",
                "search",
                "hello world",
                "--collection",
                "test-vault",
                "--limit",
                "5"
            ]
        );
    }

    #[test]
    fn update_and_create_argv_are_byte_exact() {
        assert_eq!(update_argv("c"), ["index", "update", "c"]);
        assert_eq!(
            create_argv(Path::new("/v"), "c"),
            [
                "index",
                "create",
                "files",
                "--path",
                "/v",
                "--collection",
                "c"
            ]
        );
    }

    /// One chunk entry in `results`, as `--simple-output` prints it.
    fn envelope(entries: &str) -> String {
        format!(
            "{{\"query\":\"q\",\"total_collections_searched\":1,\
              \"total_documents_found\":1,\"total_chunks_found\":1,\
              \"results\":[{entries}],\"collection_errors\":[]}}"
        )
    }

    #[test]
    fn the_envelope_decodes_chunk_entries_to_document_hits() {
        let hits = parse_envelope(&envelope(
            "{\"rank\":1,\"relevance\":0.9,\"relevance_score\":0.9,\"collection\":\"c\",\
              \"document_id\":\"x\",\"document_url\":\"file:///v/a.md\",\
              \"chunk_number\":1,\"text\":\"s\"}",
        ));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "/v/a.md", "file:// is stripped");
        assert_eq!(hits[0].score, 0.9, "v2 relevance is used as-is");
        assert_eq!(hits[0].snippet.as_deref(), Some("s"));
    }

    #[test]
    fn the_envelope_normalises_v1_scores_to_higher_is_better() {
        // v1 carries no `relevance`; `relevance_score` is squared L2 (lower is better), so it
        // is normalised into the same higher-is-better scale the comparator expects.
        let hits = parse_envelope(&envelope(
            "{\"rank\":1,\"relevance_score\":0.25,\"document_url\":\"/v/a.md\",\"text\":\"t\"}",
        ));
        assert_eq!(hits.len(), 1);
        assert!((hits[0].score - 1.0 / 1.25).abs() < 1e-12);
    }

    #[test]
    fn the_envelope_dedupes_documents_keeping_the_best_chunk() {
        let hits = parse_envelope(&envelope(
            "{\"rank\":1,\"relevance\":0.9,\"document_url\":\"/v/a.md\",\"text\":\"best\"},\
             {\"rank\":2,\"relevance\":0.5,\"document_url\":\"/v/a.md\",\"text\":\"worse\"}",
        ));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].score, 0.9);
        assert_eq!(hits[0].snippet.as_deref(), Some("best"));
    }

    #[test]
    fn the_envelope_skips_entries_with_a_missing_or_wrong_typed_url_or_score() {
        for entries in [
            "{\"relevance\":0.9}",                                // no url
            "{\"document_url\":7,\"relevance\":0.9}",             // url not a string
            "{\"document_url\":\"/v/a.md\"}",                     // no score at all
            "{\"document_url\":\"/v/a.md\",\"relevance\":\"x\"}", // score not a number
            "{\"document_url\":\"/v/a.md\",\"relevance\":true}",  // boolean is rejected
        ] {
            assert!(parse_envelope(&envelope(entries)).is_empty(), "{entries}");
        }
    }

    #[test]
    fn the_envelope_skips_a_wrong_typed_text_and_tolerates_its_absence() {
        assert!(parse_envelope(&envelope(
            "{\"relevance\":0.9,\"document_url\":\"/v/a.md\",\"text\":7}"
        ))
        .is_empty());
        let hits = parse_envelope(&envelope(
            "{\"relevance\":0.9,\"document_url\":\"/v/a.md\"}",
        ));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].snippet, None);
    }

    #[test]
    fn the_envelope_coerces_an_integer_score_and_ignores_unknown_keys() {
        let hits = parse_envelope(&envelope(
            "{\"rank\":1,\"relevance\":1,\"document_url\":\"/v/a.md\",\"future\":{\"k\":1}}",
        ));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].score, 1.0);
    }

    #[test]
    fn the_envelope_degrades_to_nothing_on_a_non_json_or_shapeless_payload() {
        assert!(parse_envelope("").is_empty());
        assert!(parse_envelope("not json").is_empty());
        assert!(parse_envelope("[1,2]").is_empty());
        assert!(parse_envelope("{\"results\":7}").is_empty());
        assert!(parse_envelope("{}").is_empty());
    }

    #[test]
    fn collection_errors_never_become_hits() {
        let hits = parse_envelope(
            "{\"query\":\"q\",\"results\":[],\"collection_errors\":[{\"collection\":\"c\",\"error\":\"boom\"}]}",
        );
        assert!(hits.is_empty());
    }

    #[test]
    fn comparator_prefers_recency_inside_the_epsilon_band() {
        let recent = hit(0.90, Some("2026-06-01T00:00:00Z"));
        let older = hit(0.91, Some("2026-01-01T00:00:00Z"));
        assert_eq!(compare(&recent, &older), Ordering::Less);
        assert_eq!(compare(&older, &recent), Ordering::Greater);
    }

    #[test]
    fn comparator_prefers_score_outside_the_band() {
        let high = hit(0.95, Some("2026-01-01T00:00:00Z"));
        let low = hit(0.70, Some("2026-06-01T00:00:00Z"));
        assert_eq!(compare(&high, &low), Ordering::Less);
        assert_eq!(compare(&low, &high), Ordering::Greater);
    }

    #[test]
    fn comparator_treats_an_undated_hit_as_oldest() {
        let dated = hit(0.90, Some("2026-01-01T00:00:00Z"));
        let undated = hit(0.90, None);
        assert_eq!(compare(&dated, &undated), Ordering::Less);
    }

    #[test]
    fn comparator_is_equal_on_identical_score_and_date() {
        let a = hit(0.5, Some("2026-01-01T00:00:00Z"));
        let b = hit(0.5, Some("2026-01-01T00:00:00Z"));
        assert_eq!(compare(&a, &b), Ordering::Equal);
    }

    #[test]
    fn comparator_breaks_an_in_band_date_tie_by_score() {
        let a = hit(0.51, Some("2026-01-01T00:00:00Z"));
        let b = hit(0.50, Some("2026-01-01T00:00:00Z"));
        assert_eq!(compare(&a, &b), Ordering::Less);
    }

    #[test]
    fn the_band_edge_is_inclusive() {
        // Exactly 0.02 apart: still a tie, so the more recently updated hit wins even though
        // its score is lower.
        let a = hit(0.02, None);
        let b = hit(0.00, Some("2026-01-01T00:00:00Z"));
        assert_eq!((a.score - b.score).abs(), TIEBREAK_EPSILON);
        assert_eq!(compare(&a, &b), Ordering::Greater);
    }

    #[test]
    fn just_outside_the_band_the_higher_score_wins() {
        let a = hit(0.5, None);
        let b = hit(0.4, Some("2026-01-01T00:00:00Z"));
        assert_eq!(compare(&a, &b), Ordering::Less);
    }

    #[test]
    fn timeout_reads_the_environment_override() {
        assert!(timeout().as_millis() > 0);
    }

    #[test]
    fn the_search_and_ingest_clocks_have_distinct_defaults() {
        assert_eq!(DEFAULT_TIMEOUT_MS, 30_000, "search keeps its 30 s clock");
        assert_eq!(INGEST_TIMEOUT_MS, 600_000, "the ingest path gets 600 s");
    }

    #[test]
    fn only_the_shipped_not_found_envelope_reads_as_a_missing_collection() {
        // The shipped CLI's `index update <absent>` machine output.
        assert!(is_missing_collection(
            "{\n  \"status\": \"error\",\n  \"error\": \"Collection 'c' not found\"\n}\n"
        ));
        // Anything else is an ordinary failure, and the create fallback is not taken.
        assert!(!is_missing_collection(""));
        assert!(!is_missing_collection("not json"));
        assert!(!is_missing_collection(
            "{\"status\":\"error\",\"error\":\"disk full\"}"
        ));
        assert!(!is_missing_collection("{\"results\":[]}"));
    }
}
