//! Shared model machinery: field order, the stash rule, and frontmatter accessors.

use chrono::{DateTime, Utc};

use crate::fm::{Meta, Value};
use crate::timefmt::{parse_iso_lenient, ts_instant};

/// The eight base frontmatter keys every id-bearing entity carries, in declaration order.
pub const BASE_FIELDS: &[&str] = &[
    "id", "type", "title", "tags", "owner", "created", "updated", "related",
];

/// Keys whose value is a timestamp we render as UTC on every surface.
pub const TIMESTAMP_FIELDS: &[&str] = &["created", "updated", "expires"];

/// One model's declaration order. Drives both the on-disk key order and the JSON key order.
#[derive(Clone, Copy, Debug)]
pub struct FieldOrder(pub &'static [&'static str]);

impl FieldOrder {
    /// The owned keys, in declaration order.
    pub fn fields(&self) -> &'static [&'static str] {
        self.0
    }

    /// Whether `key` is an owned field rather than a stashed unknown key.
    ///
    /// The literal key `extra` is never owned — it stashes like any other foreign key.
    pub fn is_known(&self, key: &str) -> bool {
        self.0.contains(&key)
    }

    /// A copy of `meta` with owned keys first in declaration order, unknown keys after in
    /// their original insertion order.
    pub fn reorder(&self, meta: &Meta) -> Meta {
        let mut out = Meta::new();
        for key in self.0 {
            if let Some(value) = meta.get(*key) {
                out.insert((*key).to_string(), value.clone());
            }
        }
        for (key, value) in meta {
            if !self.is_known(key) {
                out.insert(key.clone(), value.clone());
            }
        }
        out
    }
}

/// The string value of a frontmatter key, when it is a plain string.
pub fn meta_str<'a>(meta: &'a Meta, key: &str) -> Option<&'a str> {
    meta.get(key).and_then(Value::as_str)
}

/// The string list value of a frontmatter key; a missing or wrong-typed key yields `[]`.
pub fn meta_strings(meta: &Meta, key: &str) -> Vec<String> {
    meta.get(key)
        .and_then(Value::as_str_list)
        .unwrap_or_default()
}

/// The integer value of a frontmatter key.
pub fn meta_int(meta: &Meta, key: &str) -> Option<i64> {
    meta.get(key).and_then(Value::as_int)
}

/// The instant a timestamp key denotes; naive values are read as UTC, never shifted.
pub fn meta_time(meta: &Meta, key: &str) -> Option<DateTime<Utc>> {
    match meta.get(key)? {
        Value::Ts(ts) => Some(ts_instant(&ts.value)),
        Value::Str(s) => parse_iso_lenient(s).map(|v| ts_instant(&v)),
        _ => None,
    }
}

/// A scalar coerced to text for an exact-match filter (`type`, `status`, `owner`, `kind`).
pub fn meta_text(meta: &Meta, key: &str) -> Option<String> {
    meta.get(key).and_then(Value::as_scalar_text)
}

/// True when `meta` looks like an entity of `space`: `id` is a string with the right prefix.
pub fn has_id_prefix(meta: &Meta, prefix: &str) -> bool {
    meta_str(meta, "id").is_some_and(|id| id.starts_with(prefix))
}

/// Write a timestamp we control: RFC 3339 UTC with a `Z` suffix (overrides.md O2).
pub fn ts_value(at: &DateTime<Utc>) -> Value {
    let raw = crate::timefmt::iso_z(at);
    let parsed = parse_iso_lenient(&raw);
    match parsed {
        Some(v) => Value::Ts(crate::fm::Ts::new(raw, v)),
        None => Value::Str(raw),
    }
}

/// `Value::Str` when `Some`, `Value::Null` when `None` — the "written as null, never omitted" rule.
pub fn optional_str(value: Option<&str>) -> Value {
    match value {
        Some(v) => Value::str(v),
        None => Value::Null,
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

    const ORDER: FieldOrder = FieldOrder(BASE_FIELDS);

    #[test]
    fn reorder_puts_owned_fields_first_and_keeps_stash_order() {
        let meta = parse_meta(
            "custom_key: keep me\nupdated: 2026-01-02T00:00:00Z\nid: n-1\nextra: stashed\ntitle: T\n",
        )
        .unwrap();
        let out = ORDER.reorder(&meta);
        let keys: Vec<&str> = out.keys().map(String::as_str).collect();
        assert_eq!(keys, ["id", "title", "updated", "custom_key", "extra"]);
    }

    #[test]
    fn extra_is_never_an_owned_field() {
        assert!(!ORDER.is_known("extra"));
        assert!(ORDER.is_known("id"));
    }

    #[test]
    fn accessors_degrade_instead_of_failing() {
        let meta = parse_meta("id: n-1\ntags:\n  - a\nowner: null\ncount: 3\n").unwrap();
        assert_eq!(meta_str(&meta, "id"), Some("n-1"));
        assert_eq!(meta_str(&meta, "owner"), None);
        assert_eq!(meta_strings(&meta, "tags"), ["a"]);
        assert_eq!(meta_strings(&meta, "missing"), Vec::<String>::new());
        assert_eq!(meta_int(&meta, "count"), Some(3));
        assert!(has_id_prefix(&meta, "n-"));
        assert!(!has_id_prefix(&meta, "t-"));
    }

    #[test]
    fn meta_time_reads_both_shapes() {
        let meta =
            parse_meta("a: 2026-01-02 03:04:05+00:00\nb: '2026-01-02T03:04:05Z'\nc: x\n").unwrap();
        assert!(meta_time(&meta, "a").is_some());
        assert!(meta_time(&meta, "b").is_some());
        assert!(meta_time(&meta, "c").is_none());
        assert_eq!(meta_time(&meta, "a"), meta_time(&meta, "b"));
    }

    #[test]
    fn ts_value_writes_rfc3339_z() {
        let now = crate::timefmt::now_utc();
        let value = ts_value(&now);
        let text = crate::fm::emit::scalar(&value);
        assert!(text.ends_with('Z'), "{text}");
        assert!(!text.contains(' '), "{text}");
    }
}
