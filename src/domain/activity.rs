//! `recent-activity`: the mtime-ordered vault change feed.

use std::cmp::Ordering;
use std::path::Path;
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use crate::config::Config;
use crate::error::Result;
use crate::render::activity_row;
use crate::spaces::Space;
use crate::timefmt::parse_since;

/// The corpus `recent-activity` reads when no `--space` is given (final.md §5.9).
pub const DEFAULT_SPACES: [Space; 2] = [Space::Notes, Space::Tasks];

/// The default row cap (Python's `DEFAULT_RECENT_LIMIT`).
pub const DEFAULT_LIMIT: i64 = 20;

/// A file's mtime as float epoch seconds; `None` when it cannot be stat'ed.
fn mtime_of(path: &Path) -> Option<f64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64())
}

/// Whether an id carries one of the space id prefixes.
fn is_mesh_id(id: &str) -> bool {
    Space::ALL
        .iter()
        .filter_map(|s| s.id_prefix())
        .any(|prefix| id.starts_with(prefix))
}

/// The `mtime` of a row; `0.0` when it is missing or not a number, so any cutoff drops it.
pub fn row_mtime(entry: &Json) -> f64 {
    entry.get("mtime").and_then(Json::as_f64).unwrap_or(0.0)
}

/// Epoch seconds for a timestamp, at nanosecond resolution.
fn epoch_seconds(at: &DateTime<Utc>) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let secs = at.timestamp() as f64;
    secs + f64::from(at.timestamp_subsec_nanos()) / 1_000_000_000.0
}

/// `limit >= 0` slices; a negative limit is unbounded.
fn apply_limit(rows: &mut Vec<Json>, limit: i64) {
    if limit >= 0 {
        rows.truncate(usize::try_from(limit).unwrap_or(0));
    }
}

/// Every id-bearing file in `spaces` as a seven-key activity row, newest first.
///
/// Ordering is the two-pass stable composition the Python index used: `id` ascending
/// underneath, `mtime` descending on top. `limit < 0` is uncapped; `limit == 0` yields `[]`.
/// Unreadable, unparseable and id-less files are skipped silently.
pub fn scan_recent(cfg: &Config, limit: i64, spaces: &[Space]) -> Vec<Json> {
    let mut rows: Vec<(String, f64, Json)> = Vec::new();
    for row in crate::search::corpus_rows(cfg, spaces) {
        let Some(id) = crate::model::common::meta_str(&row.meta, "id") else {
            continue;
        };
        if !is_mesh_id(id) {
            continue;
        }
        let id = id.to_string();
        let Some(mtime) = mtime_of(&row.path) else {
            continue;
        };
        rows.push((id, mtime, activity_row(&row.path, &row.meta, mtime)));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
    let mut out: Vec<Json> = rows.into_iter().map(|(_, _, entry)| entry).collect();
    apply_limit(&mut out, limit);
    out
}

/// Whether a row passes the `--owner` / `--mine` predicates.
///
/// The row always carries `owner` and `claimed_by` (`render::activity_row` writes both keys,
/// null when absent), so there is no legacy re-read-the-disk branch.
///
/// An **unset** identity degrades `--mine` to matching nothing, exactly as
/// `select::matches_filters` does for every other listing — never to "every row whose
/// `claimed_by` is null", which is what Python's `None`-comparing `_owner_match` actually
/// did (final.md §5.9 vs `map/lenses.md` §1.4; see the report).
fn owner_match(entry: &Json, owner: Option<&str>, mine: bool, me: Option<&str>) -> bool {
    let row_owner = entry.get("owner").and_then(Json::as_str);
    let row_claimed = entry.get("claimed_by").and_then(Json::as_str);
    if let Some(want) = owner {
        if row_owner != Some(want) {
            return false;
        }
    }
    if mine {
        let Some(me) = me else {
            return false;
        };
        if row_owner != Some(me) && row_claimed != Some(me) {
            return false;
        }
    }
    true
}

/// `recent-activity` over the default corpus (notes + tasks).
pub fn recent_activity(
    cfg: &Config,
    since: Option<&str>,
    owner: Option<&str>,
    mine: bool,
    limit: i64,
) -> Result<Vec<Json>> {
    recent_activity_in(cfg, since, owner, mine, limit, &DEFAULT_SPACES)
}

/// `recent-activity` over an explicit space set.
///
/// Any active filter forces an unbounded scan and applies `limit` afterwards as a display
/// cap, so a filtered query never loses rows to the fetch cap.
pub fn recent_activity_in(
    cfg: &Config,
    since: Option<&str>,
    owner: Option<&str>,
    mine: bool,
    limit: i64,
    spaces: &[Space],
) -> Result<Vec<Json>> {
    let cutoff = since.map(parse_since).transpose()?;
    let filtered = cutoff.is_some() || owner.is_some() || mine;
    let fetch_limit = if filtered { -1 } else { limit };
    let mut rows = scan_recent(cfg, fetch_limit, spaces);
    if let Some(at) = cutoff {
        let floor = epoch_seconds(&at);
        rows.retain(|entry| row_mtime(entry) >= floor);
    }
    if owner.is_some() || mine {
        let me = cfg.agent();
        rows.retain(|entry| owner_match(entry, owner, mine, me));
    }
    if filtered {
        apply_limit(&mut rows, limit);
    }
    Ok(rows)
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
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn note(dir: &Path, id: &str, owner: Option<&str>) -> PathBuf {
        let owner = owner.map_or("null".to_string(), str::to_string);
        let text = format!(
            "---\nid: {id}\ntype: note\ntitle: {id}\ntags: []\nowner: {owner}\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated: []\n---\n\nbody\n"
        );
        let path = dir.join("notes").join(format!("{id}.md"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn task(dir: &Path, id: &str, owner: &str, claimed: &str) -> PathBuf {
        let text = format!(
            "---\nid: {id}\ntype: task\ntitle: {id}\ntags: []\nowner: {owner}\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated: []\n\
             status: open\npriority: null\nclaimed_by: {claimed}\nproject: null\n\
             blocks: []\nblocked_by: []\n---\n\nbody\n"
        );
        let path = dir.join("tasks/open").join(format!("{id}.md"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn set_mtime(path: &Path, secs_ago: u64) {
        set_mtime_at(
            path,
            SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_now() - secs_ago),
        );
    }

    fn epoch_now() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn set_mtime_at(path: &Path, when: SystemTime) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(when).unwrap();
    }

    fn ids(rows: &[Json]) -> Vec<String> {
        rows.iter()
            .map(|r| r["id"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[test]
    fn a_row_has_exactly_seven_keys_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", Some("test-agent"));
        let rows = scan_recent(&cfg, -1, &DEFAULT_SPACES);
        assert_eq!(rows.len(), 1);
        let keys: Vec<&str> = rows[0]
            .as_object()
            .unwrap()
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
        assert!(rows[0]["mtime"].is_f64());
        assert!(rows[0]["claimed_by"].is_null());
    }

    #[test]
    fn ordering_is_mtime_desc_then_id_asc() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let a = note(dir.path(), "n-a", None);
        let b = note(dir.path(), "n-b", None);
        let c = note(dir.path(), "n-c", None);
        set_mtime(&a, 10);
        set_mtime(&b, 10);
        set_mtime(&c, 100);
        let rows = scan_recent(&cfg, -1, &DEFAULT_SPACES);
        assert_eq!(ids(&rows), ["n-a", "n-b", "n-c"]);
    }

    #[test]
    fn limits_slice_and_zero_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", None);
        note(dir.path(), "n-b", None);
        assert_eq!(scan_recent(&cfg, 0, &DEFAULT_SPACES).len(), 0);
        assert_eq!(scan_recent(&cfg, 1, &DEFAULT_SPACES).len(), 1);
        assert_eq!(scan_recent(&cfg, -1, &DEFAULT_SPACES).len(), 2);
    }

    #[test]
    fn corrupt_and_foreign_files_never_appear() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", None);
        fs::write(
            dir.path().join("notes/bad.md"),
            "---\ntitle: [oops\n---\n\nx",
        )
        .unwrap();
        fs::write(dir.path().join("notes/foreign.md"), "# no frontmatter\n").unwrap();
        assert_eq!(ids(&scan_recent(&cfg, -1, &DEFAULT_SPACES)), ["n-a"]);
    }

    #[test]
    fn the_filter_runs_before_the_display_cap() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let mine = note(dir.path(), "n-mine", Some("test-agent"));
        let other1 = note(dir.path(), "n-o1", Some("someone"));
        let other2 = note(dir.path(), "n-o2", Some("someone"));
        set_mtime(&mine, 100);
        set_mtime(&other1, 10);
        set_mtime(&other2, 20);
        let rows = recent_activity(&cfg, None, None, true, 2).unwrap();
        assert_eq!(ids(&rows), ["n-mine"]);
    }

    #[test]
    fn mine_matches_owner_or_claimed_by() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        task(dir.path(), "t-own", "test-agent", "null");
        task(dir.path(), "t-claim", "someone", "test-agent");
        task(dir.path(), "t-other", "someone", "null");
        let mut got = ids(&recent_activity(&cfg, None, None, true, -1).unwrap());
        got.sort();
        assert_eq!(got, ["t-claim", "t-own"]);
        let mut by_owner = ids(&recent_activity(&cfg, None, Some("someone"), false, -1).unwrap());
        by_owner.sort();
        assert_eq!(by_owner, ["t-claim", "t-other"]);
    }

    #[test]
    fn since_drops_older_rows_and_a_bad_value_is_validation() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        let fresh = note(dir.path(), "n-fresh", None);
        let old = note(dir.path(), "n-old", None);
        set_mtime(&fresh, 60);
        set_mtime(&old, 60 * 60 * 24 * 30);
        let rows = recent_activity(&cfg, Some("7d"), None, false, -1).unwrap();
        assert_eq!(ids(&rows), ["n-fresh"]);
        let err = recent_activity(&cfg, Some("bogus"), None, false, -1).unwrap_err();
        assert_eq!(err.code(), 2);
    }

    #[test]
    fn an_unset_identity_makes_mine_match_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config_for(dir.path());
        cfg.core.agent = None;
        note(dir.path(), "n-null", None);
        note(dir.path(), "n-owned", Some("someone"));
        assert!(recent_activity(&cfg, None, None, true, -1)
            .unwrap()
            .is_empty());
        // Without `--mine` the same vault still lists everything.
        assert_eq!(
            recent_activity(&cfg, None, None, false, -1).unwrap().len(),
            2
        );
    }

    #[test]
    fn a_space_set_widens_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_for(dir.path());
        note(dir.path(), "n-a", None);
        let mem = dir.path().join("memories/m-a.md");
        fs::create_dir_all(mem.parent().unwrap()).unwrap();
        fs::write(
            &mem,
            "---\nid: m-a\ntype: memory\ntitle: M\ntags: []\nowner: null\n\
             created: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nrelated: []\n---\n\nx\n",
        )
        .unwrap();
        assert_eq!(ids(&scan_recent(&cfg, -1, &DEFAULT_SPACES)), ["n-a"]);
        let mut wide = ids(&scan_recent(
            &cfg,
            -1,
            &[Space::Notes, Space::Tasks, Space::Memories],
        ));
        wide.sort();
        assert_eq!(wide, ["m-a", "n-a"]);
    }
}
