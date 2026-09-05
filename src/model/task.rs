// STUB: owned by agent 3 (task). Fields and constants are frozen; the bodies are not.
//! The `Task` view, its field order, and the status/priority tables.

use chrono::{DateTime, Utc};

use crate::domain::select::{FromMeta, SortKey, SortValue, Sortable};
use crate::fm::Meta;
use crate::model::common::FieldOrder;

/// The four task statuses.
pub const TASK_STATUSES: [&str; 4] = ["open", "claimed", "done", "cancelled"];
/// The three writable priorities. Reads stay free-form.
pub const TASK_PRIORITIES: [&str; 3] = ["high", "normal", "low"];
/// The bucket an unknown or absent priority sorts into.
pub const PRIORITY_UNRANKED: i64 = 3;

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

impl FromMeta for Task {
    fn from_meta(_meta: &Meta) -> Option<Task> {
        None
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
