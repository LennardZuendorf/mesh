// STUB: owned by agent 2 (memory). Fields and constants are frozen; the bodies are not.
//! The `Memory` view, its field order, kinds and scopes.

use chrono::{DateTime, Utc};

use crate::domain::select::{FromMeta, SortKey, SortValue, Sortable};
use crate::fm::Meta;
use crate::model::common::FieldOrder;

/// The five memory kinds. Closed on write, free-form on read.
pub const MEMORY_KINDS: [&str; 5] = ["fact", "preference", "procedure", "insight", "episode"];
/// The two scopes. `private` is a courtesy filter, never authorisation.
pub const MEMORY_SCOPES: [&str; 2] = ["shared", "private"];

/// Memory key order on disk and in JSON.
pub const MEMORY_FIELDS: FieldOrder = FieldOrder(&[
    "id",
    "type",
    "title",
    "tags",
    "owner",
    "created",
    "updated",
    "related",
    "kind",
    "scope",
    "importance",
    "source",
    "expires",
    "superseded_by",
]);

/// A validated memory.
#[derive(Clone, Debug)]
pub struct Memory {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub created: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub related: Vec<String>,
    pub kind: String,
    pub scope: String,
    pub importance: Option<i64>,
    pub source: Option<String>,
    pub expires: Option<DateTime<Utc>>,
    pub superseded_by: Option<String>,
    pub meta: Meta,
}

impl FromMeta for Memory {
    fn from_meta(_meta: &Meta) -> Option<Memory> {
        None
    }
}

impl Sortable for Memory {
    fn sort_value(&self, key: SortKey) -> SortValue {
        match key {
            SortKey::Title => SortValue::Text(self.title.clone()),
            SortKey::Created => SortValue::Time(self.created),
            SortKey::Importance => SortValue::Num(self.importance.unwrap_or(0)),
            _ => SortValue::Time(self.updated),
        }
    }
}

/// The `status` payload's memories block.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemorySummary {
    pub total: u64,
    pub expired: u64,
    pub superseded: u64,
}
