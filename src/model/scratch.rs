// STUB: owned by agent 4 (scratch). Fields and constants are frozen; the bodies are not.
//! The `Scratch` view — name-addressed, no id.

use chrono::{DateTime, Utc};

use crate::domain::select::{FromMeta, SortKey, SortValue, Sortable};
use crate::fm::Meta;
use crate::model::common::FieldOrder;

/// Scratch key order on disk and in JSON (overrides.md O8).
pub const SCRATCH_FIELDS: FieldOrder =
    FieldOrder(&["type", "name", "agent", "tags", "created", "updated"]);

/// A scratch file: one agent's named working state.
#[derive(Clone, Debug)]
pub struct Scratch {
    pub name: String,
    pub agent: String,
    pub tags: Vec<String>,
    pub created: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub bytes: u64,
    pub meta: Meta,
}

impl FromMeta for Scratch {
    fn from_meta(_meta: &Meta) -> Option<Scratch> {
        None
    }
}

impl Sortable for Scratch {
    fn sort_value(&self, key: SortKey) -> SortValue {
        match key {
            SortKey::Title => SortValue::Text(self.name.clone()),
            SortKey::Created => SortValue::Time(self.created),
            SortKey::Bytes => SortValue::Num(i64::try_from(self.bytes).unwrap_or(i64::MAX)),
            _ => SortValue::Time(self.updated),
        }
    }
}

/// The `status` payload's scratch block.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScratchSummary {
    pub files: u64,
    pub agents: u64,
}
