//! The tag pull: metadata only, no query, no bodies.
//!
//! It routes through `domain::select`, the same engine `note list` and `task list` use, so a
//! tag pull cannot drift from a listing. Every hit scores `1.0` and carries no snippet;
//! ordering is `updated` descending then `path` ascending; `limit == 0` yields `[]` and a
//! negative limit is unbounded.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::Config;
use crate::domain::select::{select, FromMeta, SortKey, SortValue, Sortable};
use crate::error::Result;
use crate::fm::{Meta, Row};
use crate::model::common::{meta_str, meta_strings, meta_time};
use crate::search::corpus::{base_filter, rows_with_space};
use crate::search::{Hit, SearchFilter};
use crate::spaces::Space;

/// The score every tag-pull hit carries.
pub const TAG_PULL_SCORE: f64 = 1.0;

/// The corpus-wide view: it admits every readable row, foreign Markdown included.
///
/// `select` drops a row whose `from_meta` returns `None`; a tag pull must not drop foreign
/// files, so this view never fails.
#[derive(Clone, Debug)]
pub struct CorpusView {
    pub meta: Meta,
}

impl FromMeta for CorpusView {
    fn from_meta(meta: &Meta) -> Option<CorpusView> {
        Some(CorpusView { meta: meta.clone() })
    }
}

impl Sortable for CorpusView {
    fn sort_value(&self, key: SortKey) -> SortValue {
        match key {
            SortKey::Created => SortValue::Time(meta_time(&self.meta, "created")),
            SortKey::Title => SortValue::Text(
                meta_str(&self.meta, "title")
                    .unwrap_or_default()
                    .to_string(),
            ),
            _ => SortValue::Time(meta_time(&self.meta, "updated")),
        }
    }
}

/// Run a tag pull over the requested spaces.
pub fn tag_pull(cfg: &Config, f: &SearchFilter) -> Result<Vec<Hit>> {
    let scanned = rows_with_space(cfg, &f.spaces);
    let spaces: HashMap<PathBuf, Space> = scanned
        .iter()
        .map(|(row, space)| (row.path.clone(), *space))
        .collect();
    let rows: Vec<Row> = scanned.into_iter().map(|(row, _)| row).collect();
    let fallback = f.spaces.first().copied().unwrap_or(Space::Notes);

    let views = select::<CorpusView>(rows, &base_filter(f));
    Ok(views
        .into_iter()
        .map(|view| {
            let space = spaces.get(&view.path).copied().unwrap_or(fallback);
            hit_from(&view.item.meta, view.path, space)
        })
        .collect())
}

/// The hit a metadata row denotes: score `1.0`, no snippet.
pub fn hit_from(meta: &Meta, path: PathBuf, space: Space) -> Hit {
    Hit {
        id: meta_str(meta, "id").map(str::to_string),
        r#type: meta_str(meta, "type").map(str::to_string),
        title: meta_str(meta, "title").map(str::to_string),
        score: TAG_PULL_SCORE,
        tags: meta_strings(meta, "tags"),
        owner: meta_str(meta, "owner").map(str::to_string),
        updated: meta_time(meta, "updated"),
        snippet: None,
        path,
        space,
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
    use crate::config::test_support::config_for;
    use std::path::Path;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn seeded() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "notes/n-a.md",
            "---\nid: n-a\ntype: note\ntitle: A\ntags:\n  - a\n  - b\nowner: demo\n\
             updated: 2026-06-01T00:00:00Z\n---\n\nbody a\n",
        );
        write(
            root,
            "notes/n-b.md",
            "---\nid: n-b\ntype: log\ntitle: B\ntags:\n  - a\nowner: other\n\
             updated: 2026-07-01T00:00:00Z\n---\n\nbody b\n",
        );
        write(root, "notes/foreign.md", "just text\n");
        write(
            root,
            "tasks/open/t-a.md",
            "---\nid: t-a\ntype: task\ntitle: T\nstatus: open\ntags:\n  - a\n\
             updated: 2026-05-01T00:00:00Z\n---\n\nt\n",
        );
        write(
            root,
            "memories/m-a.md",
            "---\nid: m-a\ntype: memory\ntitle: M\nkind: fact\n\
             updated: 2026-08-01T00:00:00Z\n---\n\nm\n",
        );
        dir
    }

    fn pull(cfg: &Config, f: SearchFilter) -> Vec<Hit> {
        tag_pull(cfg, &f).unwrap()
    }

    fn ids(hits: &[Hit]) -> Vec<String> {
        hits.iter()
            .map(|h| h.id.clone().unwrap_or_else(|| "<foreign>".into()))
            .collect()
    }

    fn default_spaces() -> Vec<Space> {
        vec![Space::Notes, Space::Tasks, Space::Memories, Space::Assets]
    }

    #[test]
    fn every_hit_scores_one_and_carries_no_snippet() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let hits = pull(
            &cfg,
            SearchFilter {
                spaces: default_spaces(),
                limit: -1,
                ..SearchFilter::default()
            },
        );
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.score == 1.0 && h.snippet.is_none()));
    }

    #[test]
    fn tags_are_anded_exactly_and_case_sensitively() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let both = pull(
            &cfg,
            SearchFilter {
                spaces: default_spaces(),
                tags: vec!["a".into(), "b".into()],
                limit: -1,
                ..SearchFilter::default()
            },
        );
        assert_eq!(ids(&both), ["n-a"]);
        let upper = pull(
            &cfg,
            SearchFilter {
                spaces: default_spaces(),
                tags: vec!["A".into()],
                limit: -1,
                ..SearchFilter::default()
            },
        );
        assert!(upper.is_empty());
    }

    #[test]
    fn ordering_is_updated_desc_then_path_asc() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let hits = pull(
            &cfg,
            SearchFilter {
                spaces: default_spaces(),
                limit: -1,
                ..SearchFilter::default()
            },
        );
        assert_eq!(ids(&hits), ["m-a", "n-b", "n-a", "t-a", "<foreign>"]);
    }

    #[test]
    fn limit_zero_is_empty_and_negative_is_unbounded() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        assert!(pull(
            &cfg,
            SearchFilter {
                spaces: default_spaces(),
                limit: 0,
                ..SearchFilter::default()
            }
        )
        .is_empty());
        assert_eq!(
            pull(
                &cfg,
                SearchFilter {
                    spaces: default_spaces(),
                    limit: -1,
                    ..SearchFilter::default()
                }
            )
            .len(),
            5
        );
    }

    #[test]
    fn limit_slices_after_the_sort() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let hits = pull(
            &cfg,
            SearchFilter {
                spaces: default_spaces(),
                limit: 2,
                ..SearchFilter::default()
            },
        );
        assert_eq!(ids(&hits), ["m-a", "n-b"]);
    }

    #[test]
    fn type_owner_status_and_kind_filter_exactly() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let base = SearchFilter {
            spaces: default_spaces(),
            limit: -1,
            ..SearchFilter::default()
        };
        assert_eq!(
            ids(&pull(
                &cfg,
                SearchFilter {
                    type_filter: Some("log".into()),
                    ..base.clone()
                }
            )),
            ["n-b"]
        );
        assert_eq!(
            ids(&pull(
                &cfg,
                SearchFilter {
                    owner: Some("demo".into()),
                    ..base.clone()
                }
            )),
            ["n-a"]
        );
        assert_eq!(
            ids(&pull(
                &cfg,
                SearchFilter {
                    status: Some("open".into()),
                    ..base.clone()
                }
            )),
            ["t-a"]
        );
        assert_eq!(
            ids(&pull(
                &cfg,
                SearchFilter {
                    kind: Some("fact".into()),
                    ..base
                }
            )),
            ["m-a"]
        );
    }

    #[test]
    fn a_status_filter_excludes_every_note() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let hits = pull(
            &cfg,
            SearchFilter {
                spaces: vec![Space::Notes],
                status: Some("open".into()),
                limit: -1,
                ..SearchFilter::default()
            },
        );
        assert!(hits.is_empty());
    }

    #[test]
    fn foreign_rows_survive_with_a_null_id() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let hits = pull(
            &cfg,
            SearchFilter {
                spaces: vec![Space::Notes],
                limit: -1,
                ..SearchFilter::default()
            },
        );
        let foreign = hits.iter().find(|h| h.id.is_none()).unwrap();
        assert!(foreign.r#type.is_none());
        assert!(foreign.updated.is_none());
        assert_eq!(foreign.space, Space::Notes);
    }

    #[test]
    fn hits_are_attributed_to_the_space_they_were_walked_from() {
        let dir = seeded();
        let cfg = config_for(dir.path());
        let hits = pull(
            &cfg,
            SearchFilter {
                spaces: default_spaces(),
                limit: -1,
                ..SearchFilter::default()
            },
        );
        let space_of = |id: &str| {
            hits.iter()
                .find(|h| h.id.as_deref() == Some(id))
                .map(|h| h.space)
        };
        assert_eq!(space_of("n-a"), Some(Space::Notes));
        assert_eq!(space_of("t-a"), Some(Space::Tasks));
        assert_eq!(space_of("m-a"), Some(Space::Memories));
    }
}
