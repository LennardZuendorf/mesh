// STUB: owned by agent 1 (note). Fields and constants are frozen; the bodies are not.
//! The `Note` view and its field order.

use chrono::{DateTime, Utc};

use crate::domain::select::{FromMeta, SortKey, SortValue, Sortable};
use crate::fm::Meta;
use crate::model::common::{FieldOrder, BASE_FIELDS};

/// The five note types, in declaration order. `--help` text is generated from this.
pub const NOTE_TYPES: [&str; 5] = ["note", "log", "decision", "reference", "project"];

/// Note key order on disk and in JSON.
pub const NOTE_FIELDS: FieldOrder = FieldOrder(BASE_FIELDS);

/// A validated note. `meta` is the frontmatter the view was derived from and is what gets
/// rendered; the typed fields exist for validation, filtering and sorting.
#[derive(Clone, Debug)]
pub struct Note {
    pub id: String,
    pub note_type: String,
    pub title: String,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub created: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub related: Vec<String>,
    pub meta: Meta,
}

impl FromMeta for Note {
    fn from_meta(_meta: &Meta) -> Option<Note> {
        None
    }
}

impl Sortable for Note {
    fn sort_value(&self, key: SortKey) -> SortValue {
        match key {
            SortKey::Title => SortValue::Text(self.title.clone()),
            SortKey::Created => SortValue::Time(self.created),
            _ => SortValue::Time(self.updated),
        }
    }
}

/// A foreign Markdown file surfaced by `--foreign`: no id, no type, title from `# H1`.
#[derive(Clone, Debug)]
pub struct ForeignView {
    pub title: Option<String>,
    pub body: String,
    pub path: std::path::PathBuf,
}
