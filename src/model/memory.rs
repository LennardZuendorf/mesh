//! The `Memory` view, its field order, kinds and scopes.

use chrono::{DateTime, Utc};

use crate::domain::select::{FromMeta, SortKey, SortValue, Sortable};
use crate::fm::{Meta, Value};
use crate::model::common::{meta_time, FieldOrder};

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

/// The id prefix every mesh memory carries.
pub const MEMORY_ID_PREFIX: &str = "m-";
/// The `type` value a memory file declares.
pub const MEMORY_TYPE: &str = "memory";
/// The kind a memory takes when none is given.
pub const DEFAULT_KIND: &str = "fact";
/// The scope a memory takes when none is given.
pub const DEFAULT_SCOPE: &str = "shared";
/// The scope that hides a memory from a listing whose viewer is not its owner.
pub const PRIVATE_SCOPE: &str = "private";
/// The importance a memory takes when none is given, and the weight a null one ranks at.
pub const DEFAULT_IMPORTANCE: i64 = 3;
/// The inclusive lower importance bound enforced on every write.
pub const MIN_IMPORTANCE: i64 = 1;
/// The inclusive upper importance bound enforced on every write.
pub const MAX_IMPORTANCE: i64 = 5;

/// A validated memory. `meta` is the frontmatter the view was derived from and is what gets
/// rendered; the typed fields exist for validation, filtering, ranking and sorting.
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

impl Memory {
    /// The importance this memory ranks and sorts at: its own, else the documented default.
    pub fn effective_importance(&self) -> i64 {
        self.importance.unwrap_or(DEFAULT_IMPORTANCE)
    }

    /// Whether the soft TTL has passed. **Nothing is ever auto-deleted** — expiry only hides
    /// a memory from the default listing and from recall.
    pub fn is_expired(&self, at: DateTime<Utc>) -> bool {
        self.expires.is_some_and(|e| e <= at)
    }

    /// Whether a newer memory replaced this one. Kept for audit, excluded from recall.
    pub fn is_superseded(&self) -> bool {
        self.superseded_by.as_deref().is_some_and(|s| !s.is_empty())
    }

    /// The courtesy scope filter: a `private` memory is hidden from a viewer who is not its
    /// owner. Never authorisation — the file is on disk and readable by anyone.
    pub fn is_visible_to(&self, me: Option<&str>) -> bool {
        self.scope != PRIVATE_SCOPE || self.owner.as_deref() == me
    }

    /// Age in days from `updated` — the input to the recency term. A memory stamped in the
    /// future ages zero days rather than scoring above a fresh one.
    pub fn age_days(&self, at: DateTime<Utc>) -> f64 {
        let Some(updated) = self.updated else {
            return 0.0;
        };
        let seconds = (at - updated).num_seconds();
        if seconds <= 0 {
            return 0.0;
        }
        seconds as f64 / 86_400.0
    }
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

/// A free-form enum field: absent or null takes `default`, a string passes through
/// unvalidated (closed on write, free-form on read — the `priority` precedent).
fn open_enum(meta: &Meta, key: &str, default: &str) -> Option<String> {
    match meta.get(key) {
        None | Some(Value::Null) => Some(default.to_string()),
        Some(value) => value.as_str().map(str::to_string),
    }
}

/// An optional timestamp: absent or null is `None`, anything present must parse.
fn optional_time(meta: &Meta, key: &str) -> Option<Option<DateTime<Utc>>> {
    match meta.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(_) => meta_time(meta, key).map(Some),
    }
}

impl FromMeta for Memory {
    /// Validate frontmatter as a memory.
    ///
    /// `id` must be a string carrying the `m-` prefix, `type` must be `memory` when present,
    /// `title` a string, `created` and `updated` parseable timestamps, `importance` an
    /// integer when present and `expires` a parseable timestamp when present. `kind` and
    /// `scope` are free-form on read and default to `fact` / `shared`. Anything else is not a
    /// memory: a listing skips the row silently and a read or amend verb reports not found.
    fn from_meta(meta: &Meta) -> Option<Memory> {
        let id = meta.get("id")?.as_str()?.to_string();
        if !id.starts_with(MEMORY_ID_PREFIX) {
            return None;
        }
        match meta.get("type") {
            None | Some(Value::Null) => {}
            Some(value) => {
                if value.as_str()? != MEMORY_TYPE {
                    return None;
                }
            }
        }
        let importance = match meta.get("importance") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_int()?),
        };
        Some(Memory {
            id,
            title: meta.get("title")?.as_str()?.to_string(),
            tags: string_list(meta, "tags")?,
            owner: optional_string(meta, "owner")?,
            created: Some(meta_time(meta, "created")?),
            updated: Some(meta_time(meta, "updated")?),
            related: string_list(meta, "related")?,
            kind: open_enum(meta, "kind", DEFAULT_KIND)?,
            scope: open_enum(meta, "scope", DEFAULT_SCOPE)?,
            importance,
            source: optional_string(meta, "source")?,
            expires: optional_time(meta, "expires")?,
            superseded_by: optional_string(meta, "superseded_by")?,
            meta: meta.clone(),
        })
    }
}

impl Sortable for Memory {
    fn sort_value(&self, key: SortKey) -> SortValue {
        match key {
            SortKey::Title => SortValue::Text(self.title.clone()),
            SortKey::Created => SortValue::Time(self.created),
            SortKey::Importance => SortValue::Num(self.effective_importance()),
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
    use crate::timefmt::{iso_z, parse_since};

    fn meta(yaml: &str) -> Meta {
        parse_meta(yaml).unwrap()
    }

    const FULL: &str = "id: m-1\ntype: memory\ntitle: T\ntags:\n  - a\nowner: bob\n\
                        created: 2026-01-02T03:04:05Z\nupdated: 2026-01-03T00:00:00Z\n\
                        related:\n  - n-2\nkind: preference\nscope: private\nimportance: 5\n\
                        source: chat\nexpires: 2026-02-01T00:00:00Z\nsuperseded_by: m-9\n";

    #[test]
    fn a_complete_memory_validates() {
        let m = Memory::from_meta(&meta(FULL)).unwrap();
        assert_eq!(m.id, "m-1");
        assert_eq!(m.title, "T");
        assert_eq!(m.tags, ["a"]);
        assert_eq!(m.owner.as_deref(), Some("bob"));
        assert_eq!(m.related, ["n-2"]);
        assert_eq!(m.kind, "preference");
        assert_eq!(m.scope, "private");
        assert_eq!(m.importance, Some(5));
        assert_eq!(m.source.as_deref(), Some("chat"));
        assert_eq!(m.superseded_by.as_deref(), Some("m-9"));
        assert_eq!(iso_z(&m.expires.unwrap()), "2026-02-01T00:00:00Z");
    }

    #[test]
    fn optional_fields_default() {
        let m = Memory::from_meta(&meta(
            "id: m-1\ntitle: T\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
        ))
        .unwrap();
        assert_eq!(m.kind, "fact");
        assert_eq!(m.scope, "shared");
        assert_eq!(m.importance, None);
        assert_eq!(m.effective_importance(), 3);
        assert_eq!(m.source, None);
        assert_eq!(m.expires, None);
        assert_eq!(m.superseded_by, None);
        assert!(m.tags.is_empty() && m.related.is_empty());
    }

    #[test]
    fn kind_and_scope_are_free_form_on_read() {
        let m = Memory::from_meta(&meta(
            "id: m-1\ntitle: T\nkind: hunch\nscope: team\n\
             created: 2026-01-02\nupdated: 2026-01-02\n",
        ))
        .unwrap();
        assert_eq!(m.kind, "hunch");
        assert_eq!(m.scope, "team");
    }

    #[test]
    fn validation_failures_are_none() {
        for yaml in [
            // no id
            "title: T\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
            // the wrong prefix
            "id: n-1\ntitle: T\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
            // the wrong type
            "id: m-1\ntype: note\ntitle: T\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
            // no title
            "id: m-1\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
            // a non-string title
            "id: m-1\ntitle: 7\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
            // missing or unparseable stamps
            "id: m-1\ntitle: T\nupdated: 2026-01-02\n",
            "id: m-1\ntitle: T\ncreated: 2026-01-02\n",
            "id: m-1\ntitle: T\ncreated: nonsense\nupdated: 2026-01-02\n",
            // a non-integer importance
            "id: m-1\ntitle: T\nimportance: high\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
            // an unparseable expiry
            "id: m-1\ntitle: T\nexpires: soon\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
            // a non-string kind
            "id: m-1\ntitle: T\nkind: 3\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
        ] {
            assert!(Memory::from_meta(&meta(yaml)).is_none(), "{yaml}");
        }
    }

    #[test]
    fn unknown_keys_survive_on_the_view() {
        let m = Memory::from_meta(&meta(
            "id: m-1\ntitle: T\ncreated: 2026-01-02\nupdated: 2026-01-02\nextra: kept\n",
        ))
        .unwrap();
        assert_eq!(m.meta.get("extra").and_then(Value::as_str), Some("kept"));
        assert!(!MEMORY_FIELDS.is_known("extra"));
    }

    #[test]
    fn the_field_order_is_the_base_keys_then_the_six_memory_keys() {
        assert_eq!(
            MEMORY_FIELDS.fields(),
            [
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
                "superseded_by"
            ]
        );
    }

    #[test]
    fn expiry_is_inclusive_and_null_never_expires() {
        let now = crate::timefmt::now_utc();
        let expired = Memory::from_meta(&meta(
            "id: m-1\ntitle: T\nexpires: 2000-01-01T00:00:00Z\n\
             created: 2026-01-02\nupdated: 2026-01-02\n",
        ))
        .unwrap();
        assert!(expired.is_expired(now));
        let live = Memory::from_meta(&meta(
            "id: m-1\ntitle: T\nexpires: 2999-01-01T00:00:00Z\n\
             created: 2026-01-02\nupdated: 2026-01-02\n",
        ))
        .unwrap();
        assert!(!live.is_expired(now));
        let never = Memory::from_meta(&meta(
            "id: m-1\ntitle: T\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
        ))
        .unwrap();
        assert!(!never.is_expired(now));
    }

    #[test]
    fn supersession_needs_a_non_empty_id() {
        let plain = Memory::from_meta(&meta(
            "id: m-1\ntitle: T\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
        ))
        .unwrap();
        assert!(!plain.is_superseded());
        let empty = Memory::from_meta(&meta(
            "id: m-1\ntitle: T\nsuperseded_by: \"\"\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
        ))
        .unwrap();
        assert!(!empty.is_superseded());
        assert!(Memory::from_meta(&meta(FULL)).unwrap().is_superseded());
    }

    #[test]
    fn private_scope_hides_from_everyone_but_the_owner() {
        let m = Memory::from_meta(&meta(FULL)).unwrap();
        assert!(m.is_visible_to(Some("bob")));
        assert!(!m.is_visible_to(Some("alice")));
        assert!(!m.is_visible_to(None));
        let shared = Memory::from_meta(&meta(
            "id: m-2\ntitle: T\nowner: bob\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
        ))
        .unwrap();
        assert!(shared.is_visible_to(Some("alice")));
        assert!(shared.is_visible_to(None));
    }

    #[test]
    fn age_is_measured_from_updated_and_never_goes_negative() {
        let now = crate::timefmt::now_utc();
        let ninety = parse_since("90d").unwrap();
        let yaml = format!(
            "id: m-1\ntitle: T\ncreated: 2026-01-02\nupdated: {}\n",
            iso_z(&ninety)
        );
        let m = Memory::from_meta(&meta(&yaml)).unwrap();
        assert!((m.age_days(now) - 90.0).abs() < 0.01);
        let future = Memory::from_meta(&meta(
            "id: m-1\ntitle: T\ncreated: 2026-01-02\nupdated: 2999-01-01T00:00:00Z\n",
        ))
        .unwrap();
        assert_eq!(future.age_days(now), 0.0);
    }

    #[test]
    fn sort_values_follow_the_key() {
        let m = Memory::from_meta(&meta(FULL)).unwrap();
        assert_eq!(m.sort_value(SortKey::Title), SortValue::Text("T".into()));
        assert_eq!(m.sort_value(SortKey::Created), SortValue::Time(m.created));
        assert_eq!(m.sort_value(SortKey::Updated), SortValue::Time(m.updated));
        assert_eq!(m.sort_value(SortKey::Importance), SortValue::Num(5));
        let bare = Memory::from_meta(&meta(
            "id: m-1\ntitle: T\ncreated: 2026-01-02\nupdated: 2026-01-02\n",
        ))
        .unwrap();
        assert_eq!(bare.sort_value(SortKey::Importance), SortValue::Num(3));
    }

    #[test]
    fn the_kind_and_scope_tables_are_the_documented_ones() {
        assert_eq!(
            MEMORY_KINDS,
            ["fact", "preference", "procedure", "insight", "episode"]
        );
        assert_eq!(MEMORY_SCOPES, ["shared", "private"]);
        assert_eq!(MEMORY_SCOPES[1], PRIVATE_SCOPE);
        assert_eq!(MEMORY_KINDS[0], DEFAULT_KIND);
        assert_eq!(MEMORY_SCOPES[0], DEFAULT_SCOPE);
        assert!((MIN_IMPORTANCE..=MAX_IMPORTANCE).contains(&DEFAULT_IMPORTANCE));
    }
}
