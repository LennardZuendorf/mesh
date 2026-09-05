//! Search: engine routing, the built-in ranker, and the `indexed` wrapper.
//!
//! One entry point per shape — `query` for a ranked search, `tag_pull` for a metadata pull,
//! `health` for the gate report. Routing is decided here and the reported `Mode` is always
//! the branch **actually taken**, never predicted from the gates: gates can all read healthy
//! while `indexed` exits non-zero for a runtime reason, and a gate-derived mode would
//! confidently mislabel a substring hit as ranked recall.

pub mod builtin;
pub mod corpus;
pub mod health;
pub mod indexed;
pub mod tagpull;
pub mod tokenize;

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::config::Config;
use crate::domain::select::parse_csv;
use crate::error::{MeshError, Result};
use crate::fm::{read_body, Row};
use crate::spaces::Space;

/// The stderr line the built-in engine emits when it stands in for `indexed`.
pub const FALLBACK_NOTICE: &str = "search: using substring fallback (indexed unavailable)";

/// The stderr line a ranked search emits when `[search].threshold` was written for the
/// legacy tiers and now also applies to the ranker's normalised score.
pub const THRESHOLD_ADVISORY: &str = "search: explicit [search].threshold applies to the ranker's normalised score (--engine substring for the legacy tiers)";

/// The branch a query actually took. Never predicted from the gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Indexed,
    Builtin,
}

impl Mode {
    /// The `mode` value in the `--health` payload.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Indexed => "indexed",
            Mode::Builtin => "fallback",
        }
    }
}

/// What `--engine` asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Engine {
    #[default]
    Auto,
    Indexed,
    Builtin,
    Substring,
}

impl Engine {
    /// Parse an `--engine` value.
    pub fn parse(value: &str) -> Result<Engine> {
        match value {
            "auto" => Ok(Engine::Auto),
            "indexed" => Ok(Engine::Indexed),
            "builtin" => Ok(Engine::Builtin),
            "substring" => Ok(Engine::Substring),
            other => Err(MeshError::Validation(format!(
                "invalid engine: '{other}' (use auto, indexed, builtin, substring)"
            ))),
        }
    }

    /// Whether this engine bypasses the BM25 term and scores the compat tiers alone.
    pub fn is_substring(self) -> bool {
        self == Engine::Substring
    }

    /// Whether the caller pinned the built-in engine, so a fall back to it is not a degradation.
    pub fn is_explicit_builtin(self) -> bool {
        matches!(self, Engine::Builtin | Engine::Substring)
    }
}

/// The conjunctive filter a search applies, plus its routing switches.
#[derive(Clone, Debug)]
pub struct SearchFilter {
    pub spaces: Vec<Space>,
    pub type_filter: Option<String>,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub status: Option<String>,
    pub kind: Option<String>,
    pub limit: i64,
    pub threshold: Option<f64>,
    pub engine: Engine,
    pub quiet: bool,
}

impl Default for SearchFilter {
    fn default() -> Self {
        SearchFilter {
            spaces: vec![Space::Notes, Space::Tasks, Space::Memories, Space::Assets],
            type_filter: None,
            tags: Vec::new(),
            owner: None,
            status: None,
            kind: None,
            limit: 10,
            threshold: None,
            engine: Engine::Auto,
            quiet: false,
        }
    }
}

/// One search hit. `id`/`type`/`title` are null for foreign Markdown.
#[derive(Clone, Debug)]
pub struct Hit {
    pub id: Option<String>,
    pub r#type: Option<String>,
    pub title: Option<String>,
    pub score: f64,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub updated: Option<DateTime<Utc>>,
    pub snippet: Option<String>,
    pub path: PathBuf,
    pub space: Space,
}

/// What an `indexed index create`/`update` attempt did. Never an error on any surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexOutcome {
    /// `[search].collection` is unset — nothing to update, nothing went wrong.
    NoCollection,
    /// `indexed` ran and exited zero.
    Ran,
    /// `indexed` was missing, exited non-zero, or timed out.
    Failed(indexed::Failure),
}

impl IndexOutcome {
    /// Whether the caller should print the `search index unavailable …` notice.
    pub fn degraded(self) -> bool {
        matches!(self, IndexOutcome::Failed(_))
    }
}

/// A ranked search. Returns the hits and the branch that produced them.
pub fn query(cfg: &Config, q: &str, f: &SearchFilter) -> Result<(Vec<Hit>, Mode)> {
    if wants_indexed(cfg, f.engine) {
        if let Some(collection) = cfg.search.collection.clone() {
            if indexed::available() {
                let threshold = f.threshold.unwrap_or(cfg.search.threshold);
                if let Ok(mut hits) = indexed::search(cfg, &collection, q, f, threshold) {
                    apply_limit(&mut hits, f.limit);
                    return Ok((hits, Mode::Indexed));
                }
            }
        }
    }

    // The built-in engine. Both notices are emitted before any work, once per call.
    if !f.engine.is_explicit_builtin() {
        notice(f.quiet, FALLBACK_NOTICE);
    }
    if cfg.search.threshold_explicit && !f.engine.is_substring() {
        notice(f.quiet, THRESHOLD_ADVISORY);
    }

    let threshold = f.threshold.unwrap_or(builtin::DEFAULT_THRESHOLD_FLOOR);
    let docs = corpus::docs(cfg, &f.spaces);
    let mut hits = builtin::search(&docs, q, f, threshold, f.engine.is_substring());
    apply_limit(&mut hits, f.limit);
    Ok((hits, Mode::Builtin))
}

/// A tag pull: metadata only, `score = 1.0`, no snippet.
pub fn tag_pull(cfg: &Config, f: &SearchFilter) -> Result<Vec<Hit>> {
    tagpull::tag_pull(cfg, f)
}

/// The `--health` payload.
pub fn health(cfg: &Config) -> serde_json::Value {
    health::payload(cfg)
}

/// The mode the gates *predict*. `query` reports the branch actually taken instead.
pub fn route(cfg: &Config, e: Engine) -> Mode {
    if wants_indexed(cfg, e) && cfg.search.collection.is_some() && indexed::available() {
        Mode::Indexed
    } else {
        Mode::Builtin
    }
}

/// `--threshold` wins; else `[search].threshold`, but only when it was physically present in
/// the TOML; else `None`, so the engine applies its own floor and every tier stays reachable.
pub fn resolve_effective_threshold(flag: Option<f64>, cfg: &Config) -> Option<f64> {
    match flag {
        Some(t) => Some(t),
        None if cfg.search.threshold_explicit => Some(cfg.search.threshold),
        None => None,
    }
}

/// Every corpus file as a frontmatter-only `Row`, in space order.
pub fn corpus_rows(cfg: &Config, spaces: &[Space]) -> Vec<Row> {
    corpus::rows(cfg, spaces)
}

/// Whether an `indexed` binary is reachable. A pure `PATH` / `$MESH_INDEXED_BIN` lookup that
/// never executes the binary.
pub fn indexed_available(cfg: &Config) -> bool {
    let _ = cfg;
    indexed::available()
}

/// Rebuild the index for each root. **Never fails the process**: an unreachable or failing
/// `indexed` is a degradation the caller reports, not an error.
pub fn reindex(cfg: &Config, roots: &[PathBuf]) -> Result<()> {
    reindex_status(cfg, roots);
    Ok(())
}

/// `reindex`, with the outcome the caller needs to decide whether to print the
/// `search index unavailable (indexed binary missing or failed)` notice.
pub fn reindex_status(cfg: &Config, roots: &[PathBuf]) -> IndexOutcome {
    let Some(collection) = cfg.search.collection.as_deref() else {
        return IndexOutcome::NoCollection;
    };
    let mut outcome = IndexOutcome::Ran;
    for root in roots {
        if let Err(failure) = indexed::run(&indexed::create_argv(root, collection)) {
            outcome = IndexOutcome::Failed(failure);
        }
    }
    outcome
}

/// Refresh one path in the index. Never fails the process.
pub fn index_update(cfg: &Config, path: &Path) -> Result<()> {
    index_update_status(cfg, path);
    Ok(())
}

/// `index_update`, with the outcome.
pub fn index_update_status(cfg: &Config, path: &Path) -> IndexOutcome {
    let Some(collection) = cfg.search.collection.as_deref() else {
        return IndexOutcome::NoCollection;
    };
    match indexed::run(&indexed::update_argv(path, collection)) {
        Ok(_) => IndexOutcome::Ran,
        Err(failure) => IndexOutcome::Failed(failure),
    }
}

/// The spaces a search reads: `--space` when given, else `[search].spaces`.
///
/// An unknown name in `--space` is exit 2; a disabled one is exit 2 with the standard
/// `space '{name}' is disabled in [spaces]`. Names coming from the config file are filtered
/// instead of fatal, so a disabled space never breaks every search in the vault.
pub fn resolve_spaces(cfg: &Config, csv: Option<&str>) -> Result<Vec<Space>> {
    match csv {
        Some(value) => {
            let mut out: Vec<Space> = Vec::new();
            for name in parse_csv(value) {
                let Some(space) = Space::from_name(&name) else {
                    return Err(MeshError::Validation(format!(
                        "invalid space: '{name}' (use notes, tasks, memories, scratch, assets)"
                    )));
                };
                cfg.root(space)?;
                if !out.contains(&space) {
                    out.push(space);
                }
            }
            Ok(out)
        }
        None => Ok(configured_spaces(cfg)),
    }
}

/// `[search].spaces`, with unknown and disabled names dropped.
pub fn configured_spaces(cfg: &Config) -> Vec<Space> {
    let mut out: Vec<Space> = Vec::new();
    for name in &cfg.search.spaces {
        let Some(space) = Space::from_name(name) else {
            continue;
        };
        if cfg.root(space).is_ok() && !out.contains(&space) {
            out.push(space);
        }
    }
    out
}

/// Whether hits should carry the `space` key (corpus §"legacy key set").
pub fn emit_space_key(cfg: &Config, spaces: &[Space], explicit: bool) -> bool {
    corpus::emit_space_key(cfg, spaces, explicit)
}

/// Replace every snippet with the complete Markdown body, re-read from disk — what `--full`
/// means. An unreadable file yields an empty body, never an error.
pub fn fill_full_bodies(hits: &mut [Hit]) {
    for hit in hits.iter_mut() {
        hit.snippet = Some(read_body(&hit.path));
    }
}

/// `>= 0` slices, negative is unbounded — never `> 0`.
fn apply_limit(hits: &mut Vec<Hit>, limit: i64) {
    if limit >= 0 {
        hits.truncate(usize::try_from(limit).unwrap_or(0));
    }
}

/// Whether this engine setting asks for the `indexed` branch at all.
fn wants_indexed(cfg: &Config, engine: Engine) -> bool {
    match engine {
        Engine::Builtin | Engine::Substring => false,
        Engine::Indexed => true,
        Engine::Auto => cfg.search.hybrid,
    }
}

/// One stderr line, suppressed by `--quiet`, never inside a payload.
fn notice(quiet: bool, text: &str) {
    if quiet {
        return;
    }
    let stderr = std::io::stderr();
    let mut handle = stderr.lock();
    let _ = handle.write_all(format!("{text}\n").as_bytes());
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
    use crate::config::test_support::config_for;

    fn seeded() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(
            notes.join("n-1.md"),
            "---\nid: n-1\ntitle: Zebra\nupdated: 2026-06-01T00:00:00Z\n---\n\nzebra body\n",
        )
        .unwrap();
        std::fs::write(
            notes.join("n-2.md"),
            "---\nid: n-2\ntitle: Other\nupdated: 2026-05-01T00:00:00Z\n---\n\nmentions zebra\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn engine_parse_accepts_the_four_values_and_rejects_the_rest() {
        assert_eq!(Engine::parse("auto").unwrap(), Engine::Auto);
        assert_eq!(Engine::parse("indexed").unwrap(), Engine::Indexed);
        assert_eq!(Engine::parse("builtin").unwrap(), Engine::Builtin);
        assert_eq!(Engine::parse("substring").unwrap(), Engine::Substring);
        let err = Engine::parse("magic").unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid engine: 'magic' (use auto, indexed, builtin, substring)"
        );
        assert_eq!(err.code(), 2);
    }

    #[test]
    fn mode_names_match_the_health_payload() {
        assert_eq!(Mode::Indexed.name(), "indexed");
        assert_eq!(Mode::Builtin.name(), "fallback");
    }

    #[test]
    fn threshold_resolution_is_three_way() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        assert_eq!(resolve_effective_threshold(Some(0.1), &cfg), Some(0.1));
        assert_eq!(resolve_effective_threshold(None, &cfg), None);
        cfg.search.threshold_explicit = true;
        cfg.search.threshold = 0.65;
        assert_eq!(resolve_effective_threshold(None, &cfg), Some(0.65));
        assert_eq!(resolve_effective_threshold(Some(0.2), &cfg), Some(0.2));
    }

    #[test]
    fn an_unset_threshold_reaches_the_body_tier() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let f = SearchFilter {
            spaces: vec![Space::Notes],
            engine: Engine::Substring,
            quiet: true,
            limit: -1,
            threshold: resolve_effective_threshold(None, &cfg),
            ..SearchFilter::default()
        };
        let (hits, mode) = query(&cfg, "zebra", &f).unwrap();
        assert_eq!(mode, Mode::Builtin);
        let ids: Vec<&str> = hits.iter().filter_map(|h| h.id.as_deref()).collect();
        assert_eq!(ids, ["n-1", "n-2"]);
    }

    #[test]
    fn an_explicit_config_threshold_excludes_the_body_tier() {
        let dir = seeded();
        let mut cfg = config_for(dir.path());
        cfg.search.threshold = 0.65;
        cfg.search.threshold_explicit = true;
        let f = SearchFilter {
            spaces: vec![Space::Notes],
            engine: Engine::Substring,
            quiet: true,
            limit: -1,
            threshold: resolve_effective_threshold(None, &cfg),
            ..SearchFilter::default()
        };
        let (hits, _) = query(&cfg, "zebra", &f).unwrap();
        let ids: Vec<&str> = hits.iter().filter_map(|h| h.id.as_deref()).collect();
        assert_eq!(ids, ["n-1"]);
    }

    #[test]
    fn routing_stays_builtin_without_a_collection() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        assert_eq!(route(&cfg, Engine::Auto), Mode::Builtin);
        assert_eq!(route(&cfg, Engine::Indexed), Mode::Builtin);
        assert_eq!(route(&cfg, Engine::Builtin), Mode::Builtin);
        assert_eq!(route(&cfg, Engine::Substring), Mode::Builtin);
    }

    #[test]
    fn hybrid_off_never_asks_for_indexed_but_engine_indexed_does() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.search.hybrid = false;
        assert!(!wants_indexed(&cfg, Engine::Auto));
        assert!(wants_indexed(&cfg, Engine::Indexed));
        assert!(!wants_indexed(&cfg, Engine::Builtin));
    }

    #[test]
    fn limit_is_applied_after_ranking() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let f = SearchFilter {
            spaces: vec![Space::Notes],
            engine: Engine::Substring,
            quiet: true,
            limit: 1,
            ..SearchFilter::default()
        };
        let (hits, _) = query(&cfg, "zebra", &f).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id.as_deref(), Some("n-1"));
    }

    #[test]
    fn limit_zero_and_negative_behave_like_every_other_listing() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let base = SearchFilter {
            spaces: vec![Space::Notes],
            engine: Engine::Substring,
            quiet: true,
            ..SearchFilter::default()
        };
        let (none, _) = query(
            &cfg,
            "zebra",
            &SearchFilter {
                limit: 0,
                ..base.clone()
            },
        )
        .unwrap();
        assert!(none.is_empty());
        let (all, _) = query(&cfg, "zebra", &SearchFilter { limit: -1, ..base }).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn configured_spaces_drops_unknown_and_disabled_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.search.spaces = vec!["notes".into(), "nope".into(), "memories".into()];
        assert_eq!(configured_spaces(&cfg), [Space::Notes, Space::Memories]);
        cfg.spaces = crate::spaces::Spaces::resolve(
            dir.path(),
            &[(Space::Memories, crate::spaces::SpaceSetting::Disabled)],
        )
        .unwrap();
        assert_eq!(configured_spaces(&cfg), [Space::Notes]);
    }

    #[test]
    fn resolve_spaces_rejects_an_unknown_name() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let err = resolve_spaces(&cfg, Some("notes,nope")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid space: 'nope' (use notes, tasks, memories, scratch, assets)"
        );
        assert_eq!(err.code(), 2);
    }

    #[test]
    fn resolve_spaces_rejects_a_disabled_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.spaces = crate::spaces::Spaces::resolve(
            dir.path(),
            &[(Space::Scratch, crate::spaces::SpaceSetting::Disabled)],
        )
        .unwrap();
        let err = resolve_spaces(&cfg, Some("scratch")).unwrap_err();
        assert_eq!(err.to_string(), "space 'scratch' is disabled in [spaces]");
    }

    #[test]
    fn resolve_spaces_dedupes_and_preserves_order() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        assert_eq!(
            resolve_spaces(&cfg, Some("tasks, notes ,tasks")).unwrap(),
            [Space::Tasks, Space::Notes]
        );
    }

    #[test]
    fn reindex_and_index_update_never_fail_the_process() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        assert!(reindex(&cfg, &[dir.path().to_path_buf()]).is_ok());
        assert_eq!(
            reindex_status(&cfg, &[dir.path().to_path_buf()]),
            IndexOutcome::NoCollection
        );
        assert!(index_update(&cfg, &dir.path().join("notes/n-1.md")).is_ok());
        cfg.search.collection = Some("c".into());
        assert!(reindex(&cfg, &[dir.path().to_path_buf()]).is_ok());
        assert!(index_update(&cfg, &dir.path().join("notes/n-1.md")).is_ok());
    }

    #[test]
    fn index_outcome_reports_degradation() {
        assert!(!IndexOutcome::NoCollection.degraded());
        assert!(!IndexOutcome::Ran.degraded());
        assert!(IndexOutcome::Failed(indexed::Failure::Missing).degraded());
    }

    #[test]
    fn corpus_rows_matches_the_walked_corpus() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        assert_eq!(corpus_rows(&cfg, &[Space::Notes]).len(), 2);
    }

    #[test]
    fn fill_full_bodies_replaces_every_snippet() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let f = SearchFilter {
            spaces: vec![Space::Notes],
            engine: Engine::Substring,
            quiet: true,
            limit: -1,
            ..SearchFilter::default()
        };
        let (mut hits, _) = query(&cfg, "zebra", &f).unwrap();
        fill_full_bodies(&mut hits);
        assert!(hits.iter().all(|h| h.snippet.is_some()));
        assert_eq!(hits[0].snippet.as_deref(), Some("zebra body"));
    }
}
