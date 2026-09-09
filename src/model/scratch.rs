// STUB: owned by agent 4 (scratch). Fields and constants are frozen; the bodies are not.
//! The `Scratch` view — name-addressed, no id.

use chrono::{DateTime, Utc};

use crate::domain::select::{FromMeta, SortKey, SortValue, Sortable};
use crate::fm::Meta;
use crate::model::common::{meta_str, meta_strings, meta_time, FieldOrder};

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
    fn from_meta(meta: &Meta) -> Option<Scratch> {
        if meta_str(meta, "type") != Some("scratch") {
            return None;
        }
        let name = meta_str(meta, "name")?.to_string();
        let agent = meta_str(meta, "agent")?.to_string();
        Some(Scratch {
            name,
            agent,
            tags: meta_strings(meta, "tags"),
            created: meta_time(meta, "created"),
            updated: meta_time(meta, "updated"),
            // Not derivable from frontmatter: the domain layer patches this in from the
            // body length it just read ("mutate-in-place, validate-a-view").
            bytes: 0,
            meta: meta.clone(),
        })
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

    #[test]
    fn from_meta_requires_type_name_and_agent() {
        let meta = parse_meta("type: scratch\nname: plan\nagent: flights-agent\n").unwrap();
        let s = Scratch::from_meta(&meta).unwrap();
        assert_eq!(s.name, "plan");
        assert_eq!(s.agent, "flights-agent");
        assert_eq!(s.bytes, 0);

        assert!(
            Scratch::from_meta(&parse_meta("type: note\nname: x\nagent: a\n").unwrap()).is_none()
        );
        assert!(Scratch::from_meta(&parse_meta("type: scratch\nagent: a\n").unwrap()).is_none());
        assert!(Scratch::from_meta(&parse_meta("type: scratch\nname: x\n").unwrap()).is_none());
    }

    #[test]
    fn sort_values_cover_every_key() {
        let meta = parse_meta(
            "type: scratch\nname: b\nagent: a\ncreated: 2026-01-01T00:00:00Z\nupdated: 2026-01-02T00:00:00Z\n",
        )
        .unwrap();
        let mut s = Scratch::from_meta(&meta).unwrap();
        s.bytes = 42;
        assert_eq!(s.sort_value(SortKey::Title), SortValue::Text("b".into()));
        assert_eq!(s.sort_value(SortKey::Bytes), SortValue::Num(42));
        assert!(matches!(
            s.sort_value(SortKey::Updated),
            SortValue::Time(Some(_))
        ));
        assert!(matches!(
            s.sort_value(SortKey::Created),
            SortValue::Time(Some(_))
        ));
    }
}
