//! The built-in engine: a BM25-lite ranker with the four Python-era tiers as floors.
//!
//! `score = max(bm25, tier)` (final.md §7.2). The tiers stay exactly reachable, so a better
//! ranker provably cannot lose a hit the old substring scan would have returned, while a
//! multi-word query that no longer matches as one literal substring still ranks sensibly.
//! `--engine substring` disables the BM25 term entirely, restoring legacy scoring and
//! head-of-body snippets.

use crate::domain::select::matches_filters;
use crate::model::common::{meta_str, meta_strings, meta_time};
use crate::search::corpus::{base_filter, CorpusDoc};
use crate::search::tokenize::{
    find_chars, headings, lower_chars, term_frequency, token_set, tokenize,
};
use crate::search::{Hit, SearchFilter};
use crate::text::{snippet_around, snippet_head};

/// `lower(title) == lower(query)`.
pub const TIER_TITLE_EXACT: f64 = 1.0;
/// `lower(query)` is a substring of `lower(title)`.
pub const TIER_TITLE_SUBSTRING: f64 = 0.8;
/// `lower(query)` is a substring of some `lower(tag)`.
pub const TIER_TAG_CONTAINS: f64 = 0.6;
/// `lower(query)` is a substring of `lower(body)`.
pub const TIER_BODY_SUBSTRING: f64 = 0.4;
/// The engine's own floor when no threshold was made explicit — the body tier, so every tier
/// is reachable at default configuration.
pub const DEFAULT_THRESHOLD_FLOOR: f64 = TIER_BODY_SUBSTRING;

/// Field weights, title heaviest.
const W_TITLE: f64 = 3.0;
const W_TAGS: f64 = 2.0;
const W_HEADINGS: f64 = 1.5;
const W_BODY: f64 = 1.0;
/// The saturation constant in `tf / (tf + k1)`.
const K1: f64 = 1.2;

/// One document's four token fields.
#[derive(Clone, Debug, Default)]
struct Fields {
    title: Vec<String>,
    tags: Vec<String>,
    headings: Vec<String>,
    body: Vec<String>,
}

impl Fields {
    fn of(doc: &CorpusDoc) -> Fields {
        Fields {
            title: tokenize(meta_str(&doc.meta, "title").unwrap_or_default()),
            tags: tokenize(&meta_strings(&doc.meta, "tags").join(" ")),
            headings: tokenize(&headings(&doc.body)),
            body: tokenize(&doc.body),
        }
    }

    fn contains(&self, token: &str) -> bool {
        [&self.title, &self.tags, &self.headings, &self.body]
            .into_iter()
            .any(|f| f.iter().any(|t| t == token))
    }
}

/// `tf / (tf + 1.2)`.
fn tf_sat(tokens: &[String], token: &str) -> f64 {
    let tf = term_frequency(tokens, token) as f64;
    if tf == 0.0 {
        0.0
    } else {
        tf / (tf + K1)
    }
}

/// `ln(1 + (N - df + 0.5) / (df + 0.5))`.
pub fn idf(total: usize, df: usize) -> f64 {
    let n = total as f64;
    let d = df as f64;
    (1.0 + (n - d + 0.5) / (d + 0.5)).ln()
}

/// The compat floor: the highest matching Python-era tier, or `0.0`.
///
/// The whole query is one literal substring — no tokenisation, no stemming. A tag match is
/// `query ⊂ tag`, not the reverse.
pub fn tier(query_lower: &str, title: Option<&str>, tags: &[String], body: &str) -> f64 {
    let title_lower = title.unwrap_or_default().to_lowercase();
    if title_lower == query_lower {
        return TIER_TITLE_EXACT;
    }
    if title_lower.contains(query_lower) {
        return TIER_TITLE_SUBSTRING;
    }
    if tags.iter().any(|t| t.to_lowercase().contains(query_lower)) {
        return TIER_TAG_CONTAINS;
    }
    if body.to_lowercase().contains(query_lower) {
        return TIER_BODY_SUBSTRING;
    }
    0.0
}

/// Run the built-in engine over `docs`.
///
/// `substring_only` is `--engine substring`: `score = tier` and the snippet is always the head
/// of the body, byte-identical to the legacy scan.
pub fn search(
    docs: &[CorpusDoc],
    query: &str,
    f: &SearchFilter,
    threshold: f64,
    substring_only: bool,
) -> Vec<Hit> {
    let filter = base_filter(f);
    let candidates: Vec<&CorpusDoc> = docs
        .iter()
        .filter(|d| matches_filters(&d.meta, &filter))
        .collect();

    let query_lower = query.to_lowercase();
    let tokens = token_set(query);
    let fields: Vec<Fields> = candidates.iter().map(|d| Fields::of(d)).collect();
    let total = candidates.len();
    let idfs: Vec<f64> = tokens
        .iter()
        .map(|t| {
            let df = fields.iter().filter(|f| f.contains(t)).count();
            idf(total, df)
        })
        .collect();
    let denom: f64 = idfs.iter().map(|i| i * W_TITLE).sum();

    let mut hits: Vec<Hit> = Vec::new();
    for (doc, field) in candidates.iter().zip(fields.iter()) {
        let bm25 = if substring_only || denom <= 0.0 {
            0.0
        } else {
            let raw: f64 = tokens
                .iter()
                .zip(idfs.iter())
                .map(|(t, i)| {
                    i * (W_TITLE * tf_sat(&field.title, t)
                        + W_TAGS * tf_sat(&field.tags, t)
                        + W_HEADINGS * tf_sat(&field.headings, t)
                        + W_BODY * tf_sat(&field.body, t))
                })
                .sum();
            raw / denom
        };
        let tags = meta_strings(&doc.meta, "tags");
        let floor = tier(&query_lower, meta_str(&doc.meta, "title"), &tags, &doc.body);
        let score = if substring_only {
            floor
        } else {
            bm25.max(floor)
        };
        if score < threshold {
            continue;
        }
        let snippet = if substring_only || floor >= bm25 {
            snippet_head(&doc.body)
        } else {
            centred_snippet(&doc.body, &tokens, &idfs).or_else(|| snippet_head(&doc.body))
        };
        hits.push(Hit {
            id: meta_str(&doc.meta, "id").map(str::to_string),
            r#type: meta_str(&doc.meta, "type").map(str::to_string),
            title: meta_str(&doc.meta, "title").map(str::to_string),
            score,
            tags,
            owner: meta_str(&doc.meta, "owner").map(str::to_string),
            updated: meta_time(&doc.meta, "updated"),
            snippet,
            path: doc.path.clone(),
            space: doc.space,
        });
    }
    sort_hits(&mut hits);
    hits
}

/// A window centred on the first occurrence of the best-scoring query token.
fn centred_snippet(body: &str, tokens: &[String], idfs: &[f64]) -> Option<String> {
    let haystack = lower_chars(body);
    let mut best: Option<(f64, usize)> = None;
    for (token, weight) in tokens.iter().zip(idfs.iter()) {
        let needle = lower_chars(token);
        let Some(at) = find_chars(&haystack, &needle) else {
            continue;
        };
        if best.is_none_or(|(w, _)| *weight > w) {
            best = Some((*weight, at));
        }
    }
    let (_, at) = best?;
    snippet_around(body, at)
}

/// `score` descending, then `updated` descending (undated last), then `path` ascending.
///
/// A stable composition: weakest key first, strongest last. No epsilon band — that belongs to
/// the `indexed` path alone.
pub fn sort_hits(hits: &mut [Hit]) {
    hits.sort_by(|a, b| a.path.cmp(&b.path));
    hits.sort_by_key(|h| std::cmp::Reverse(h.updated));
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
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
    use crate::fm::parse_meta;
    use crate::spaces::Space;
    use std::path::PathBuf;

    fn doc(name: &str, meta: &str, body: &str) -> CorpusDoc {
        CorpusDoc {
            path: PathBuf::from(format!("/v/notes/{name}.md")),
            meta: parse_meta(meta).unwrap_or_default(),
            body: body.to_string(),
            space: Space::Notes,
        }
    }

    fn scores(hits: &[Hit]) -> Vec<(String, f64)> {
        hits.iter()
            .map(|h| (h.id.clone().unwrap_or_default(), h.score))
            .collect()
    }

    fn tier_corpus() -> Vec<CorpusDoc> {
        vec![
            doc("n-exact", "id: n-exact\ntitle: matrix probe\n", "nothing"),
            doc(
                "n-sub",
                "id: n-sub\ntitle: the matrix probe rig\n",
                "nothing",
            ),
            doc(
                "n-tag",
                "id: n-tag\ntitle: Unrelated\ntags:\n  - matrix probe\n",
                "nothing",
            ),
            doc(
                "n-body",
                "id: n-body\ntitle: Unrelated\n",
                "somewhere matrix probe appears",
            ),
            doc("n-none", "id: n-none\ntitle: Unrelated\n", "nothing"),
        ]
    }

    #[test]
    fn the_four_tiers_are_exactly_reachable() {
        let docs = tier_corpus();
        let f = SearchFilter {
            limit: -1,
            ..SearchFilter::default()
        };
        let hits = search(&docs, "matrix probe", &f, 0.0, true);
        let mut got = scores(&hits);
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            got,
            [
                ("n-body".to_string(), 0.4),
                ("n-exact".to_string(), 1.0),
                ("n-none".to_string(), 0.0),
                ("n-sub".to_string(), 0.8),
                ("n-tag".to_string(), 0.6),
            ]
        );
    }

    #[test]
    fn tier_matches_the_query_inside_a_tag_not_the_reverse() {
        assert_eq!(
            tier("matrix probe", Some("x"), &["matrix probe".into()], ""),
            TIER_TAG_CONTAINS
        );
        assert_eq!(
            tier("matrix probe rig", Some("x"), &["matrix".into()], ""),
            0.0
        );
    }

    #[test]
    fn tier_treats_a_missing_title_as_empty() {
        assert_eq!(tier("", None, &[], ""), TIER_TITLE_EXACT);
        assert_eq!(tier("x", None, &[], "has x"), TIER_BODY_SUBSTRING);
    }

    #[test]
    fn the_floor_is_never_lost_to_the_ranker() {
        let docs = tier_corpus();
        let f = SearchFilter {
            limit: -1,
            ..SearchFilter::default()
        };
        let ranked = search(&docs, "matrix probe", &f, 0.4, false);
        let legacy = search(&docs, "matrix probe", &f, 0.4, true);
        for hit in &legacy {
            assert!(
                ranked.iter().any(|h| h.id == hit.id),
                "ranker lost {:?}",
                hit.id
            );
        }
        for hit in &ranked {
            let floor = legacy
                .iter()
                .find(|h| h.id == hit.id)
                .map(|h| h.score)
                .unwrap_or(0.0);
            assert!(hit.score >= floor - 1e-9);
        }
    }

    #[test]
    fn a_multi_word_query_ranks_when_no_literal_substring_matches() {
        let docs = vec![
            doc(
                "n-1",
                "id: n-1\ntitle: Zebra migration notes\n",
                "the herd crossed the river",
            ),
            doc("n-2", "id: n-2\ntitle: Unrelated\n", "nothing at all"),
        ];
        let f = SearchFilter {
            limit: -1,
            ..SearchFilter::default()
        };
        let legacy = search(&docs, "zebra herd", &f, 0.0, true);
        assert!(legacy.iter().all(|h| h.score == 0.0));
        let ranked = search(&docs, "zebra herd", &f, 0.1, false);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].id.as_deref(), Some("n-1"));
    }

    #[test]
    fn substring_engine_scores_only_the_tier() {
        let docs = vec![doc(
            "n-1",
            "id: n-1\ntitle: Zebra\n",
            "zebra zebra zebra zebra",
        )];
        let f = SearchFilter::default();
        let legacy = search(&docs, "zebra", &f, 0.0, true);
        assert_eq!(legacy[0].score, TIER_TITLE_EXACT);
        let ranked = search(&docs, "zebra", &f, 0.0, false);
        assert!(ranked[0].score >= TIER_TITLE_EXACT);
    }

    #[test]
    fn threshold_drops_strictly_below_and_keeps_equal() {
        let docs = tier_corpus();
        let f = SearchFilter {
            limit: -1,
            ..SearchFilter::default()
        };
        let kept = search(&docs, "matrix probe", &f, 0.6, true);
        let ids: Vec<String> = kept.iter().filter_map(|h| h.id.clone()).collect();
        assert!(ids.contains(&"n-tag".to_string()));
        assert!(!ids.contains(&"n-body".to_string()));
    }

    #[test]
    fn substring_snippet_is_the_head_of_the_body() {
        let docs = vec![doc(
            "n-1",
            "id: n-1\ntitle: T\n",
            "  head of the body mentions zebra late  ",
        )];
        let hits = search(&docs, "zebra", &SearchFilter::default(), 0.0, true);
        assert_eq!(
            hits[0].snippet.as_deref(),
            Some("head of the body mentions zebra late")
        );
    }

    #[test]
    fn an_empty_body_yields_no_snippet() {
        let docs = vec![doc("n-1", "id: n-1\ntitle: zebra\n", "   ")];
        let hits = search(&docs, "zebra", &SearchFilter::default(), 0.0, true);
        assert!(hits[0].snippet.is_none());
    }

    #[test]
    fn a_ranked_hit_gets_a_centred_snippet() {
        let filler = "lorem ipsum ".repeat(40);
        let body = format!("{filler}zebra {filler}herd");
        let docs = vec![doc("n-1", "id: n-1\ntitle: Unrelated title\n", &body)];
        let f = SearchFilter {
            limit: -1,
            ..SearchFilter::default()
        };
        // "zebra herd" is no literal substring of anything, so the tier is 0 and only the
        // ranker can produce this hit — which is exactly when the snippet is centred.
        let hits = search(&docs, "zebra herd", &f, 0.01, false);
        assert_eq!(hits.len(), 1);
        let snippet = hits[0].snippet.clone().unwrap();
        assert!(snippet.contains("zebra"));
        assert!(!snippet_head(&body).unwrap().contains("zebra"));
    }

    #[test]
    fn a_floor_hit_keeps_the_head_snippet_even_under_the_ranker() {
        let filler = "lorem ipsum ".repeat(40);
        let body = format!("{filler}zebra");
        let docs = vec![doc("n-1", "id: n-1\ntitle: Unrelated title\n", &body)];
        let hits = search(&docs, "zebra", &SearchFilter::default(), 0.0, false);
        assert_eq!(hits[0].score, TIER_BODY_SUBSTRING);
        assert_eq!(hits[0].snippet, snippet_head(&body));
    }

    #[test]
    fn ordering_is_score_then_updated_then_path() {
        let docs = vec![
            doc(
                "n-old",
                "id: n-old\ntitle: zebra\nupdated: 2026-01-01T00:00:00Z\n",
                "x",
            ),
            doc(
                "n-new",
                "id: n-new\ntitle: zebra\nupdated: 2026-06-01T00:00:00Z\n",
                "x",
            ),
            doc("n-none", "id: n-none\ntitle: zebra\n", "x"),
        ];
        let f = SearchFilter {
            limit: -1,
            ..SearchFilter::default()
        };
        let hits = search(&docs, "zebra", &f, 0.0, true);
        let ids: Vec<&str> = hits.iter().filter_map(|h| h.id.as_deref()).collect();
        assert_eq!(ids, ["n-new", "n-old", "n-none"]);
    }

    #[test]
    fn equal_scores_and_no_dates_fall_back_to_path_order() {
        let docs = vec![
            doc("n-b", "id: n-b\ntitle: zebra\n", "x"),
            doc("n-a", "id: n-a\ntitle: zebra\n", "x"),
        ];
        let f = SearchFilter {
            limit: -1,
            ..SearchFilter::default()
        };
        let hits = search(&docs, "zebra", &f, 0.0, true);
        let ids: Vec<&str> = hits.iter().filter_map(|h| h.id.as_deref()).collect();
        assert_eq!(ids, ["n-a", "n-b"]);
    }

    #[test]
    fn filters_are_conjunctive() {
        let docs = vec![
            doc(
                "n-1",
                "id: n-1\ntitle: zebra\ntype: log\nowner: me\ntags:\n  - a\n  - b\n",
                "x",
            ),
            doc("n-2", "id: n-2\ntitle: zebra\ntype: note\nowner: me\n", "x"),
        ];
        let f = SearchFilter {
            type_filter: Some("log".into()),
            tags: vec!["a".into(), "b".into()],
            owner: Some("me".into()),
            limit: -1,
            ..SearchFilter::default()
        };
        let hits = search(&docs, "zebra", &f, 0.0, true);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id.as_deref(), Some("n-1"));
    }

    #[test]
    fn a_foreign_file_hits_with_null_identity() {
        let docs = vec![doc("foreign", "", "a foreign file mentioning zebra")];
        let hits = search(&docs, "zebra", &SearchFilter::default(), 0.0, true);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].id.is_none());
        assert!(hits[0].r#type.is_none());
        assert!(hits[0].title.is_none());
    }

    #[test]
    fn idf_falls_as_the_document_frequency_rises() {
        assert!(idf(10, 1) > idf(10, 5));
        assert!(idf(10, 10) >= 0.0);
    }

    #[test]
    fn an_empty_query_matches_every_document_mechanically() {
        let docs = tier_corpus();
        let f = SearchFilter {
            limit: -1,
            ..SearchFilter::default()
        };
        let hits = search(&docs, "", &f, 0.0, true);
        assert_eq!(hits.len(), docs.len());
        assert!(hits.iter().all(|h| h.score >= TIER_TITLE_SUBSTRING));
    }
}
