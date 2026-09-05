//! The `indexed` subprocess wrapper: three argv forms, NDJSON in, hits out.
//!
//! Agent content is data, never shell input — every invocation is a direct `execve` with an
//! argument vector, never a shell string. A missing binary, a non-zero exit or a hung child
//! degrades to the built-in engine with the standard notice; none of them is an error.

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

/// The wall clock allowed per invocation (deviation 12). Overridable for tests through
/// `$MESH_INDEXED_TIMEOUT_MS`; a value of `0` or an unparsable one falls back to this.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// The environment variable that overrides `DEFAULT_TIMEOUT_MS`, in milliseconds.
pub const ENV_TIMEOUT_MS: &str = "MESH_INDEXED_TIMEOUT_MS";

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

/// The per-invocation wall clock.
pub fn timeout() -> Duration {
    let ms = std::env::var(ENV_TIMEOUT_MS)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    Duration::from_millis(ms)
}

/// `indexed index search <query> --collection <C> --json --limit <N>`.
pub fn search_argv(query: &str, collection: &str, limit: i64) -> Vec<String> {
    vec![
        "index".to_string(),
        "search".to_string(),
        query.to_string(),
        "--collection".to_string(),
        collection.to_string(),
        "--json".to_string(),
        "--limit".to_string(),
        limit.to_string(),
    ]
}

/// `indexed index update <path> --collection <C>`.
pub fn update_argv(path: &Path, collection: &str) -> Vec<String> {
    vec![
        "index".to_string(),
        "update".to_string(),
        path.display().to_string(),
        "--collection".to_string(),
        collection.to_string(),
    ]
}

/// `indexed index create <root> --collection <C>`.
pub fn create_argv(root: &Path, collection: &str) -> Vec<String> {
    vec![
        "index".to_string(),
        "create".to_string(),
        root.display().to_string(),
        "--collection".to_string(),
        collection.to_string(),
    ]
}

/// Run `indexed` with `argv` and return its stdout.
///
/// stdout is drained by a helper thread so a child that outruns the pipe buffer cannot
/// deadlock the parent; stderr is discarded, as the Python client discarded it.
pub fn run(argv: &[String]) -> Result<String, Failure> {
    let Some(bin) = binary() else {
        return Err(Failure::Missing);
    };
    let mut child = Command::new(&bin)
        .args(argv)
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
    let deadline = Instant::now() + timeout();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = reader.join().unwrap_or_default();
                return if status.success() {
                    Ok(text)
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

/// Decode NDJSON: one object per line, tolerantly.
///
/// Blank lines are skipped; a malformed line is skipped and decoding continues; a missing or
/// wrong-typed `path`/`score` skips the line; a **boolean `score` is rejected** (`true` must
/// not become `1.0`); an integer `score` coerces to float; unknown keys are ignored; an absent
/// or null `snippet` is `None`.
pub fn parse_ndjson(text: &str) -> Vec<IndexedHit> {
    let mut out: Vec<IndexedHit> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(Json::Object(obj)) = serde_json::from_str::<Json>(trimmed) else {
            continue;
        };
        let Some(Json::String(path)) = obj.get("path") else {
            continue;
        };
        let Some(Json::Number(score)) = obj.get("score") else {
            continue;
        };
        let Some(score) = score.as_f64() else {
            continue;
        };
        let snippet = match obj.get("snippet") {
            None | Some(Json::Null) => None,
            Some(Json::String(s)) => Some(s.clone()),
            Some(_) => continue,
        };
        out.push(IndexedHit {
            path: path.clone(),
            score,
            snippet,
        });
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
        return Ordering::Equal;
    }
    if a.score > b.score {
        Ordering::Less
    } else {
        Ordering::Greater
    }
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
    let raw = run(&search_argv(query, collection, f.limit))?;
    let filter = base_filter(f);
    let fallback = f.spaces.first().copied().unwrap_or(Space::Notes);
    let mut hits: Vec<Hit> = Vec::new();
    for hit in parse_ndjson(&raw) {
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
                "--json",
                "--limit",
                "5"
            ]
        );
    }

    #[test]
    fn update_and_create_argv_are_byte_exact() {
        assert_eq!(
            update_argv(Path::new("/v/notes/n-1.md"), "c"),
            ["index", "update", "/v/notes/n-1.md", "--collection", "c"]
        );
        assert_eq!(
            create_argv(Path::new("/v"), "c"),
            ["index", "create", "/v", "--collection", "c"]
        );
    }

    #[test]
    fn ndjson_decodes_one_object_per_line() {
        let hits = parse_ndjson("{\"path\":\"/a.md\",\"score\":0.9}\n{\"path\":\"/b.md\",\"score\":0.5,\"snippet\":\"s\"}\n");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "/a.md");
        assert_eq!(hits[0].snippet, None);
        assert_eq!(hits[1].snippet.as_deref(), Some("s"));
    }

    #[test]
    fn ndjson_skips_blank_and_malformed_lines_and_continues() {
        let hits = parse_ndjson(
            "\n   \n{not json}\n{\"path\":\"/a.md\",\"score\":0.9}\nnope\n{\"path\":\"/b.md\",\"score\":0.1}\n",
        );
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn ndjson_requires_a_string_path() {
        assert!(parse_ndjson("{\"score\":0.9}").is_empty());
        assert!(parse_ndjson("{\"path\":7,\"score\":0.9}").is_empty());
        assert!(parse_ndjson("{\"path\":null,\"score\":0.9}").is_empty());
    }

    #[test]
    fn ndjson_requires_a_numeric_score() {
        assert!(parse_ndjson("{\"path\":\"/a.md\"}").is_empty());
        assert!(parse_ndjson("{\"path\":\"/a.md\",\"score\":\"0.9\"}").is_empty());
        assert!(parse_ndjson("{\"path\":\"/a.md\",\"score\":null}").is_empty());
    }

    #[test]
    fn ndjson_rejects_a_boolean_score() {
        assert!(parse_ndjson("{\"path\":\"/a.md\",\"score\":true}").is_empty());
        assert!(parse_ndjson("{\"path\":\"/a.md\",\"score\":false}").is_empty());
    }

    #[test]
    fn ndjson_coerces_an_integer_score_to_float() {
        let hits = parse_ndjson("{\"path\":\"/a.md\",\"score\":1}");
        assert_eq!(hits[0].score, 1.0);
    }

    #[test]
    fn ndjson_ignores_unknown_keys_and_a_json_array_line() {
        let hits = parse_ndjson("{\"path\":\"/a.md\",\"score\":0.5,\"future\":{\"k\":1}}\n[1,2]\n");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].score, 0.5);
    }

    #[test]
    fn ndjson_skips_a_wrong_typed_snippet() {
        assert!(parse_ndjson("{\"path\":\"/a.md\",\"score\":0.5,\"snippet\":7}").is_empty());
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
}
