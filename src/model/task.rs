//! The `Task` view, its field order, and the status/priority tables.

use chrono::{DateTime, Utc};

use crate::domain::select::{FromMeta, SortKey, SortValue, Sortable};
use crate::fm::{Meta, Value};
use crate::model::common::{meta_str, meta_time, FieldOrder};

/// The four task statuses.
pub const TASK_STATUSES: [&str; 4] = ["open", "claimed", "done", "cancelled"];
/// The three writable priorities. Reads stay free-form.
pub const TASK_PRIORITIES: [&str; 3] = ["high", "normal", "low"];
/// The bucket an unknown or absent priority sorts into.
pub const PRIORITY_UNRANKED: i64 = 3;

/// The two statuses that end a task's life.
pub const TERMINAL_STATUSES: [&str; 2] = ["done", "cancelled"];

/// Task key order on disk and in JSON.
pub const TASK_FIELDS: FieldOrder = FieldOrder(&[
    "id",
    "type",
    "title",
    "tags",
    "owner",
    "created",
    "updated",
    "related",
    "status",
    "priority",
    "claimed_by",
    "project",
    "blocks",
    "blocked_by",
]);

/// The sort rank of a priority label: high 0, normal 1, low 2, anything else 3.
pub fn priority_rank(priority: Option<&str>) -> i64 {
    match priority {
        Some("high") => 0,
        Some("normal") => 1,
        Some("low") => 2,
        _ => PRIORITY_UNRANKED,
    }
}

/// Whether a status ends a task's life. Reads stay tolerant; only these two are terminal.
pub fn is_terminal(status: &str) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

/// A validated task.
#[derive(Clone, Debug)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub created: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub related: Vec<String>,
    pub status: String,
    pub priority: Option<String>,
    pub claimed_by: Option<String>,
    pub project: Option<String>,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
    pub meta: Meta,
}

/// A `str | None` field: absent or explicitly null is `None`, a plain string is `Some`,
/// anything else fails validation.
fn optional_text(meta: &Meta, key: &str) -> Option<Option<String>> {
    match meta.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(Value::Str(s)) => Some(Some(s.clone())),
        Some(_) => None,
    }
}

/// A `list[str]` field: absent or null is `[]`, a list of strings is itself, anything else
/// fails validation.
fn string_list(meta: &Meta, key: &str) -> Option<Vec<String>> {
    match meta.get(key) {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::List(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(item.as_str()?.to_string());
            }
            Some(out)
        }
        Some(_) => None,
    }
}

impl FromMeta for Task {
    fn from_meta(meta: &Meta) -> Option<Task> {
        let id = meta_str(meta, "id")?;
        if !id.starts_with("t-") {
            return None;
        }
        // `type` is pinned to the literal "task" — a note filed under tasks/ is not a task.
        if meta_str(meta, "type") != Some("task") {
            return None;
        }
        let title = meta_str(meta, "title")?.to_string();
        // `created` and `updated` are required datetimes; a naive value reads as UTC.
        let created = meta_time(meta, "created")?;
        let updated = meta_time(meta, "updated")?;
        let status = match meta.get("status") {
            None | Some(Value::Null) => "open".to_string(),
            Some(Value::Str(s)) if TASK_STATUSES.contains(&s.as_str()) => s.clone(),
            Some(_) => return None,
        };
        Some(Task {
            id: id.to_string(),
            title,
            tags: string_list(meta, "tags")?,
            owner: optional_text(meta, "owner")?,
            created: Some(created),
            updated: Some(updated),
            related: string_list(meta, "related")?,
            status,
            // Free-form on read (map/core.md §9.14); constrained only at the write boundary.
            priority: optional_text(meta, "priority")?,
            claimed_by: optional_text(meta, "claimed_by")?,
            project: optional_text(meta, "project")?,
            blocks: string_list(meta, "blocks")?,
            blocked_by: string_list(meta, "blocked_by")?,
            meta: meta.clone(),
        })
    }
}

impl Sortable for Task {
    fn sort_value(&self, key: SortKey) -> SortValue {
        match key {
            SortKey::Title => SortValue::Text(self.title.clone()),
            SortKey::Created => SortValue::Time(self.created),
            SortKey::Priority => SortValue::Rank(priority_rank(self.priority.as_deref())),
            _ => SortValue::Time(self.updated),
        }
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

    const FULL: &str = "id: t-A1\ntype: task\ntitle: Ship it\ntags:\n  - x\nowner: alice\n\
                        created: 2026-01-01T00:00:00Z\nupdated: 2026-01-02T00:00:00Z\n\
                        related: []\nstatus: open\npriority: high\nclaimed_by: null\n\
                        project: n-P\nblocks: []\nblocked_by:\n  - t-B2\n";

    fn task(yaml: &str) -> Option<Task> {
        Task::from_meta(&parse_meta(yaml).unwrap())
    }

    #[test]
    fn a_complete_task_validates() {
        let t = task(FULL).unwrap();
        assert_eq!(t.id, "t-A1");
        assert_eq!(t.title, "Ship it");
        assert_eq!(t.tags, ["x"]);
        assert_eq!(t.owner.as_deref(), Some("alice"));
        assert_eq!(t.status, "open");
        assert_eq!(t.priority.as_deref(), Some("high"));
        assert_eq!(t.claimed_by, None);
        assert_eq!(t.project.as_deref(), Some("n-P"));
        assert_eq!(t.blocked_by, ["t-B2"]);
        assert!(t.blocks.is_empty());
        assert!(t.created.is_some() && t.updated.is_some());
    }

    #[test]
    fn admission_requires_a_t_prefixed_id_and_a_task_type() {
        assert!(task(&FULL.replace("id: t-A1", "id: n-A1")).is_none());
        assert!(task(&FULL.replace("type: task", "type: note")).is_none());
        assert!(task(&FULL.replace("type: task\n", "")).is_none());
    }

    #[test]
    fn required_scalars_are_required() {
        assert!(task(&FULL.replace("title: Ship it\n", "")).is_none());
        assert!(task(&FULL.replace("created: 2026-01-01T00:00:00Z\n", "")).is_none());
        assert!(task(&FULL.replace("updated: 2026-01-02T00:00:00Z\n", "")).is_none());
    }

    #[test]
    fn an_unknown_status_fails_but_an_absent_one_defaults_to_open() {
        assert!(task(&FULL.replace("status: open", "status: wat")).is_none());
        let t = task(&FULL.replace("status: open\n", "")).unwrap();
        assert_eq!(t.status, "open");
        for s in TASK_STATUSES {
            let t = task(&FULL.replace("status: open", &format!("status: {s}"))).unwrap();
            assert_eq!(t.status, s);
        }
    }

    #[test]
    fn priority_is_free_form_on_read() {
        let t = task(&FULL.replace("priority: high", "priority: urgent")).unwrap();
        assert_eq!(t.priority.as_deref(), Some("urgent"));
        assert_eq!(priority_rank(t.priority.as_deref()), PRIORITY_UNRANKED);
        let t = task(&FULL.replace("priority: high", "priority: null")).unwrap();
        assert_eq!(t.priority, None);
    }

    #[test]
    fn a_wrong_typed_field_fails_validation() {
        assert!(task(&FULL.replace("owner: alice", "owner: 3")).is_none());
        assert!(task(&FULL.replace("blocks: []", "blocks: nope")).is_none());
        assert!(task(&FULL.replace("tags:\n  - x", "tags: 7")).is_none());
    }

    #[test]
    fn the_rank_table_puts_unknowns_in_the_trailing_bucket() {
        assert_eq!(priority_rank(Some("high")), 0);
        assert_eq!(priority_rank(Some("normal")), 1);
        assert_eq!(priority_rank(Some("low")), 2);
        assert_eq!(priority_rank(Some("urgent")), 3);
        assert_eq!(priority_rank(None), 3);
    }

    #[test]
    fn terminal_covers_exactly_done_and_cancelled() {
        assert!(is_terminal("done"));
        assert!(is_terminal("cancelled"));
        assert!(!is_terminal("open"));
        assert!(!is_terminal("claimed"));
    }

    #[test]
    fn sort_values_follow_the_key() {
        let t = task(FULL).unwrap();
        assert_eq!(
            t.sort_value(SortKey::Title),
            SortValue::Text("Ship it".into())
        );
        assert_eq!(t.sort_value(SortKey::Priority), SortValue::Rank(0));
        assert_eq!(t.sort_value(SortKey::Created), SortValue::Time(t.created));
        assert_eq!(t.sort_value(SortKey::Updated), SortValue::Time(t.updated));
    }

    #[test]
    fn the_field_order_is_the_documented_fourteen() {
        assert_eq!(
            TASK_FIELDS.fields(),
            [
                "id",
                "type",
                "title",
                "tags",
                "owner",
                "created",
                "updated",
                "related",
                "status",
                "priority",
                "claimed_by",
                "project",
                "blocks",
                "blocked_by"
            ]
        );
        assert!(!TASK_FIELDS.is_known("extra"));
    }
}
