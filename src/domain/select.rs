//! The one filter/sort/limit engine. Every listing in every space routes through `select`.

use std::cmp::Reverse;

use chrono::{DateTime, Utc};

use crate::error::{MeshError, Result};
use crate::fm::{Meta, Row, View};
use crate::model::common::{meta_str, meta_strings, meta_text, meta_time};

/// The sort keys any space may ask for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Updated,
    Created,
    Title,
    Priority,
    Importance,
    Bytes,
}

impl SortKey {
    /// The flag value that selects this key.
    pub fn name(self) -> &'static str {
        match self {
            SortKey::Updated => "updated",
            SortKey::Created => "created",
            SortKey::Title => "title",
            SortKey::Priority => "priority",
            SortKey::Importance => "importance",
            SortKey::Bytes => "bytes",
        }
    }

    /// Parse a `--sort` value against the keys this command allows.
    ///
    /// The rejection text is `invalid sort field: 'x' (use updated, created, title)`.
    pub fn parse(value: &str, allowed: &[SortKey]) -> Result<SortKey> {
        match allowed.iter().find(|k| k.name() == value) {
            Some(key) => Ok(*key),
            None => {
                let names: Vec<&str> = allowed.iter().map(|k| k.name()).collect();
                Err(MeshError::Validation(format!(
                    "invalid sort field: '{value}' (use {})",
                    names.join(", ")
                )))
            }
        }
    }
}

/// What a view sorts by under one key.
#[derive(Clone, Debug, PartialEq)]
pub enum SortValue {
    /// A timestamp; `None` sorts last under a descending key.
    Time(Option<DateTime<Utc>>),
    /// Case-insensitive ascending text.
    Text(String),
    /// An ascending rank, unranked last.
    Rank(i64),
    /// A descending number.
    Num(i64),
}

/// The conjunctive filter every listing shares.
#[derive(Clone, Debug)]
pub struct Filter {
    pub tags: Option<Vec<String>>,
    pub any_tag: bool,
    pub owner: Option<String>,
    pub mine: bool,
    pub me: Option<String>,
    pub cutoff: Option<DateTime<Utc>>,
    pub stale_cutoff: Option<DateTime<Utc>>,
    pub sort: SortKey,
    pub limit: Option<i64>,
    /// Exact string equality against raw frontmatter, e.g. `("type", "log")`.
    pub extra: Vec<(String, String)>,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            tags: None,
            any_tag: false,
            owner: None,
            mine: false,
            me: None,
            cutoff: None,
            stale_cutoff: None,
            sort: SortKey::Updated,
            limit: Some(20),
            extra: Vec::new(),
        }
    }
}

impl Filter {
    /// A filter with no predicates and no limit.
    pub fn unbounded() -> Filter {
        Filter {
            limit: None,
            ..Filter::default()
        }
    }

    /// Add an exact-match predicate against a raw frontmatter key.
    pub fn with_extra(mut self, key: &str, value: Option<&str>) -> Filter {
        if let Some(v) = value {
            self.extra.push((key.to_string(), v.to_string()));
        }
        self
    }
}

/// A typed view built from validated frontmatter.
pub trait FromMeta: Sized {
    /// Build the view, or `None` when the frontmatter does not validate (the row is skipped).
    fn from_meta(meta: &Meta) -> Option<Self>;
}

/// How a view answers a sort key.
pub trait Sortable {
    /// The value this view sorts by under `key`.
    fn sort_value(&self, key: SortKey) -> SortValue;
}

/// AND by default, OR under `--any-tag`.
pub fn matches_tags(have: &[String], want: &[String], any_tag: bool) -> bool {
    if want.is_empty() {
        return true;
    }
    if any_tag {
        want.iter().any(|w| have.contains(w))
    } else {
        want.iter().all(|w| have.contains(w))
    }
}

/// Split a comma-separated value: trimmed, empties dropped, order-preserving dedupe.
pub fn parse_csv(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in value.split(',') {
        let t = token.trim();
        if t.is_empty() {
            continue;
        }
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// Whether a row's frontmatter passes every filter predicate.
pub fn matches_filters(meta: &Meta, f: &Filter) -> bool {
    if let Some(owner) = &f.owner {
        if meta_str(meta, "owner") != Some(owner.as_str()) {
            return false;
        }
    }
    if f.mine {
        match &f.me {
            None => return false,
            Some(me) => {
                let owned = meta_str(meta, "owner") == Some(me.as_str());
                let claimed = meta_str(meta, "claimed_by") == Some(me.as_str());
                if !owned && !claimed {
                    return false;
                }
            }
        }
    }
    if let Some(tags) = &f.tags {
        if !matches_tags(&meta_strings(meta, "tags"), tags, f.any_tag) {
            return false;
        }
    }
    if let Some(cutoff) = f.cutoff {
        match meta_time(meta, "updated") {
            Some(updated) if updated >= cutoff => {}
            _ => return false,
        }
    }
    if let Some(stale) = f.stale_cutoff {
        match meta_time(meta, "updated") {
            Some(updated) if updated < stale => {}
            _ => return false,
        }
    }
    for (key, want) in &f.extra {
        if meta_text(meta, key).as_deref() != Some(want.as_str()) {
            return false;
        }
    }
    true
}

/// Filter, validate, sort and limit a scan. The only listing implementation in the tree.
pub fn select<V: FromMeta + Sortable>(rows: Vec<Row>, f: &Filter) -> Vec<View<V>> {
    let mut views: Vec<View<V>> = Vec::new();
    for row in rows {
        if !matches_filters(&row.meta, f) {
            continue;
        }
        let Some(item) = V::from_meta(&row.meta) else {
            continue;
        };
        views.push(View {
            item,
            body: String::new(),
            path: row.path,
        });
    }
    sort_views(&mut views, f.sort);
    match f.limit {
        Some(n) if n >= 0 => {
            let n = usize::try_from(n).unwrap_or(0);
            views.truncate(n);
            views
        }
        _ => views,
    }
}

/// The stable sort composition: path ascending underneath, then the requested key.
pub fn sort_views<V: Sortable>(views: &mut [View<V>], key: SortKey) {
    views.sort_by(|a, b| a.path.to_string_lossy().cmp(&b.path.to_string_lossy()));
    match key {
        SortKey::Title => views.sort_by_key(|v| text_key(v, key)),
        SortKey::Priority => {
            views.sort_by_key(|v| time_key(v, SortKey::Created));
            views.sort_by_key(|v| rank_key(v, key));
        }
        SortKey::Importance => {
            views.sort_by_key(|v| time_key(v, SortKey::Created));
            views.sort_by_key(|v| Reverse(num_key(v, key)));
        }
        SortKey::Bytes => views.sort_by_key(|v| Reverse(num_key(v, key))),
        SortKey::Updated | SortKey::Created => {
            views.sort_by_key(|v| Reverse(time_key(v, key)));
        }
    }
}

fn text_key<V: Sortable>(view: &View<V>, key: SortKey) -> String {
    match view.item.sort_value(key) {
        SortValue::Text(t) => t.to_lowercase(),
        other => format!("{other:?}"),
    }
}

fn time_key<V: Sortable>(view: &View<V>, key: SortKey) -> Option<DateTime<Utc>> {
    match view.item.sort_value(key) {
        SortValue::Time(t) => t,
        _ => None,
    }
}

fn rank_key<V: Sortable>(view: &View<V>, key: SortKey) -> i64 {
    match view.item.sort_value(key) {
        SortValue::Rank(r) => r,
        _ => i64::MAX,
    }
}

fn num_key<V: Sortable>(view: &View<V>, key: SortKey) -> i64 {
    match view.item.sort_value(key) {
        SortValue::Num(n) => n,
        SortValue::Rank(r) => r,
        _ => 0,
    }
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
    use std::path::PathBuf;

    #[derive(Debug, Clone)]
    struct Sample {
        id: String,
        title: String,
        created: Option<DateTime<Utc>>,
        updated: Option<DateTime<Utc>>,
        rank: i64,
    }

    impl FromMeta for Sample {
        fn from_meta(meta: &Meta) -> Option<Sample> {
            let id = meta_str(meta, "id")?.to_string();
            if !id.starts_with("n-") {
                return None;
            }
            Some(Sample {
                id,
                title: meta_str(meta, "title").unwrap_or_default().to_string(),
                created: meta_time(meta, "created"),
                updated: meta_time(meta, "updated"),
                rank: match meta_str(meta, "priority") {
                    Some("high") => 0,
                    Some("normal") => 1,
                    Some("low") => 2,
                    _ => 3,
                },
            })
        }
    }

    impl Sortable for Sample {
        fn sort_value(&self, key: SortKey) -> SortValue {
            match key {
                SortKey::Title => SortValue::Text(self.title.clone()),
                SortKey::Created => SortValue::Time(self.created),
                SortKey::Updated => SortValue::Time(self.updated),
                SortKey::Priority => SortValue::Rank(self.rank),
                SortKey::Importance | SortKey::Bytes => SortValue::Num(self.rank),
            }
        }
    }

    fn row(path: &str, yaml: &str) -> Row {
        Row {
            path: PathBuf::from(path),
            meta: parse_meta(yaml).unwrap(),
        }
    }

    fn corpus() -> Vec<Row> {
        vec![
            row(
                "/v/b.md",
                "id: n-B\ntitle: beta\nowner: alice\ntags:\n  - x\ncreated: 2026-01-02T00:00:00Z\nupdated: 2026-03-02T00:00:00Z\npriority: normal\n",
            ),
            row(
                "/v/a.md",
                "id: n-A\ntitle: Alpha\nowner: bob\ntags:\n  - x\n  - y\ncreated: 2026-01-01T00:00:00Z\nupdated: 2026-03-03T00:00:00Z\npriority: high\n",
            ),
            row(
                "/v/c.md",
                "id: n-C\ntitle: gamma\nowner: alice\ntags: []\ncreated: 2026-01-03T00:00:00Z\nupdated: 2026-03-01T00:00:00Z\n",
            ),
            row("/v/x.md", "id: t-X\ntitle: not a note\n"),
        ]
    }

    fn ids(views: &[View<Sample>]) -> Vec<String> {
        views.iter().map(|v| v.item.id.clone()).collect()
    }

    #[test]
    fn admission_skips_rows_the_view_rejects() {
        let out: Vec<View<Sample>> = select(corpus(), &Filter::unbounded());
        assert_eq!(ids(&out), ["n-A", "n-B", "n-C"]);
    }

    #[test]
    fn filters_are_conjunctive() {
        let f = Filter {
            owner: Some("alice".into()),
            ..Filter::unbounded()
        };
        assert_eq!(ids(&select(corpus(), &f)), ["n-B", "n-C"]);

        let f = Filter {
            tags: Some(vec!["x".into(), "y".into()]),
            ..Filter::unbounded()
        };
        assert_eq!(ids(&select(corpus(), &f)), ["n-A"]);

        let f = Filter {
            tags: Some(vec!["x".into(), "y".into()]),
            any_tag: true,
            ..Filter::unbounded()
        };
        assert_eq!(ids(&select(corpus(), &f)), ["n-A", "n-B"]);

        let f = Filter::unbounded().with_extra("priority", Some("high"));
        assert_eq!(ids(&select(corpus(), &f)), ["n-A"]);
    }

    #[test]
    fn mine_with_no_identity_matches_nothing() {
        let f = Filter {
            mine: true,
            me: None,
            ..Filter::unbounded()
        };
        assert!(select::<Sample>(corpus(), &f).is_empty());
        let f = Filter {
            mine: true,
            me: Some("alice".into()),
            ..Filter::unbounded()
        };
        assert_eq!(ids(&select(corpus(), &f)), ["n-B", "n-C"]);
    }

    #[test]
    fn since_is_a_floor_and_stale_a_ceiling() {
        let cut = crate::timefmt::parse_since("2026-03-02T00:00:00Z").unwrap();
        let f = Filter {
            cutoff: Some(cut),
            ..Filter::unbounded()
        };
        assert_eq!(ids(&select(corpus(), &f)), ["n-A", "n-B"]);
        let f = Filter {
            stale_cutoff: Some(cut),
            ..Filter::unbounded()
        };
        assert_eq!(ids(&select(corpus(), &f)), ["n-C"]);
    }

    #[test]
    fn sorts_compose_stably() {
        let f = Filter::unbounded();
        assert_eq!(ids(&select(corpus(), &f)), ["n-A", "n-B", "n-C"]);
        let f = Filter {
            sort: SortKey::Created,
            ..Filter::unbounded()
        };
        assert_eq!(ids(&select(corpus(), &f)), ["n-C", "n-B", "n-A"]);
        let f = Filter {
            sort: SortKey::Title,
            ..Filter::unbounded()
        };
        assert_eq!(ids(&select(corpus(), &f)), ["n-A", "n-B", "n-C"]);
        let f = Filter {
            sort: SortKey::Priority,
            ..Filter::unbounded()
        };
        // high, then normal, then unranked; FIFO by created inside a rank.
        assert_eq!(ids(&select(corpus(), &f)), ["n-A", "n-B", "n-C"]);
    }

    #[test]
    fn limit_boundaries() {
        let mut f = Filter::unbounded();
        f.limit = Some(0);
        assert!(select::<Sample>(corpus(), &f).is_empty());
        f.limit = Some(2);
        assert_eq!(select::<Sample>(corpus(), &f).len(), 2);
        f.limit = Some(-1);
        assert_eq!(select::<Sample>(corpus(), &f).len(), 3);
        f.limit = None;
        assert_eq!(select::<Sample>(corpus(), &f).len(), 3);
    }

    #[test]
    fn sort_key_rejection_names_the_allowed_values() {
        let err = SortKey::parse(
            "bogus",
            &[SortKey::Updated, SortKey::Created, SortKey::Title],
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid sort field: 'bogus' (use updated, created, title)"
        );
        let err = SortKey::parse(
            "bogus",
            &[
                SortKey::Updated,
                SortKey::Created,
                SortKey::Title,
                SortKey::Priority,
            ],
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid sort field: 'bogus' (use updated, created, title, priority)"
        );
        assert_eq!(
            SortKey::parse("title", &[SortKey::Title]).unwrap(),
            SortKey::Title
        );
    }

    #[test]
    fn csv_parsing_trims_dedupes_and_drops_empties() {
        assert_eq!(parse_csv(" a , b ,, a "), ["a", "b"]);
        assert!(parse_csv(" , ").is_empty());
    }
}
