//! The `Note` view and its field order.

use chrono::{DateTime, Utc};

use crate::domain::select::{FromMeta, SortKey, SortValue, Sortable};
use crate::fm::{Meta, Value};
use crate::model::common::{meta_time, FieldOrder, BASE_FIELDS};

/// The five note types, in declaration order. `--help` text is generated from this.
pub const NOTE_TYPES: [&str; 5] = ["note", "log", "decision", "reference", "project"];

/// Note key order on disk and in JSON.
pub const NOTE_FIELDS: FieldOrder = FieldOrder(BASE_FIELDS);

/// The id prefix every mesh note carries.
pub const NOTE_ID_PREFIX: &str = "n-";

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

/// A string-list field: absent or null is `[]`, a list keeps its string members, anything
/// else fails validation.
fn string_list(meta: &Meta, key: &str) -> Option<Vec<String>> {
    match meta.get(key) {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(value) => value.as_str_list(),
    }
}

/// An optional string field: absent or null is `None`, a string is `Some`, anything else
/// fails validation.
fn optional_string(meta: &Meta, key: &str) -> Option<Option<String>> {
    match meta.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(value) => value.as_str().map(|s| Some(s.to_string())),
    }
}

impl FromMeta for Note {
    /// Validate frontmatter as a note.
    ///
    /// `id` must be a string carrying the `n-` prefix, `title` a string, `created` and
    /// `updated` parseable timestamps, and `type` one of [`NOTE_TYPES`] (absent means
    /// `note`). Anything else is not a note: a listing skips the row silently and a read or
    /// amend verb reports it as not found.
    fn from_meta(meta: &Meta) -> Option<Note> {
        let id = meta.get("id")?.as_str()?.to_string();
        if !id.starts_with(NOTE_ID_PREFIX) {
            return None;
        }
        let note_type = match meta.get("type") {
            None | Some(Value::Null) => "note".to_string(),
            Some(value) => value.as_str()?.to_string(),
        };
        if !NOTE_TYPES.contains(&note_type.as_str()) {
            return None;
        }
        Some(Note {
            id,
            note_type,
            title: meta.get("title")?.as_str()?.to_string(),
            tags: string_list(meta, "tags")?,
            owner: optional_string(meta, "owner")?,
            created: Some(meta_time(meta, "created")?),
            updated: Some(meta_time(meta, "updated")?),
            related: string_list(meta, "related")?,
            meta: meta.clone(),
        })
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

    fn meta(yaml: &str) -> Meta {
        parse_meta(yaml).unwrap()
    }

    const FULL: &str = "id: n-1\ntype: log\ntitle: T\ntags:\n  - a\nowner: bob\n\
                        created: 2026-01-02T03:04:05Z\nupdated: 2026-01-03T00:00:00Z\n\
                        related:\n  - n-2\n";

    #[test]
    fn a_complete_note_validates() {
        let note = Note::from_meta(&meta(FULL)).unwrap();
        assert_eq!(note.id, "n-1");
        assert_eq!(note.note_type, "log");
        assert_eq!(note.title, "T");
        assert_eq!(note.tags, ["a"]);
        assert_eq!(note.owner.as_deref(), Some("bob"));
        assert_eq!(note.related, ["n-2"]);
        assert!(note.created.is_some() && note.updated.is_some());
    }

    #[test]
    fn optional_fields_default() {
        let note = Note::from_meta(&meta(
            "id: n-1\ntitle: T\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
        ))
        .unwrap();
        assert_eq!(note.note_type, "note");
        assert!(note.tags.is_empty());
        assert_eq!(note.owner, None);
        assert!(note.related.is_empty());
        // A bare date is midnight UTC, never shifted.
        assert_eq!(
            crate::timefmt::iso_z(&note.created.unwrap()),
            "2026-01-02T00:00:00Z"
        );
    }

    #[test]
    fn a_null_owner_is_none_not_a_failure() {
        let note = Note::from_meta(&meta(
            "id: n-1\ntitle: T\nowner: null\ntags: []\nrelated: []\n\
             created: 2026-01-02\nupdated: 2026-01-02\n",
        ))
        .unwrap();
        assert_eq!(note.owner, None);
    }

    #[test]
    fn validation_failures_are_none() {
        for yaml in [
            "title: T\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
            "id: t-1\ntitle: T\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
            "id: n-1\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
            "id: n-1\ntitle: T\nupdated: 2026-01-02\n",
            "id: n-1\ntitle: T\ncreated: 2026-01-02\n",
            "id: n-1\ntitle: T\ntype: memo\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
            "id: n-1\ntitle: T\ncreated: nonsense\nupdated: 2026-01-02\n",
            "id: n-1\ntitle: 7\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
        ] {
            assert!(Note::from_meta(&meta(yaml)).is_none(), "{yaml}");
        }
    }

    #[test]
    fn unknown_keys_survive_on_the_view() {
        let note = Note::from_meta(&meta(
            "id: n-1\ntitle: T\ncreated: 2026-01-02\nupdated: 2026-01-02\nextra: kept\n",
        ))
        .unwrap();
        assert_eq!(note.meta.get("extra").and_then(Value::as_str), Some("kept"));
        assert!(!NOTE_FIELDS.is_known("extra"));
    }

    #[test]
    fn the_field_order_is_the_eight_base_keys() {
        assert_eq!(
            NOTE_FIELDS.fields(),
            ["id", "type", "title", "tags", "owner", "created", "updated", "related"]
        );
    }

    #[test]
    fn sort_values_follow_the_key() {
        let note = Note::from_meta(&meta(FULL)).unwrap();
        assert_eq!(
            note.sort_value(SortKey::Title),
            SortValue::Text("T".to_string())
        );
        assert_eq!(
            note.sort_value(SortKey::Created),
            SortValue::Time(note.created)
        );
        assert_eq!(
            note.sort_value(SortKey::Updated),
            SortValue::Time(note.updated)
        );
    }
}
