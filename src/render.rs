//! JSON builders. Every payload's key order is decided here, from one `FieldOrder` per model.

use std::path::Path;

use serde_json::{Map, Value as Json};

use crate::fm::{Meta, Value};
use crate::model::common::TIMESTAMP_FIELDS;
use crate::spaces::Space;
use crate::timefmt::ts_wire;

/// Convert a frontmatter value to JSON.
///
/// `promote_naive` reinterprets a bare date or naive datetime as UTC; it is set for the typed
/// timestamp fields and left off for stashed unknown keys, which keep their own shape.
pub fn value_to_json(value: &Value, promote_naive: bool) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::from(*i),
        Value::Float(f) => serde_json::Number::from_f64(*f).map_or(Json::Null, Json::Number),
        Value::Str(s) => Json::String(s.clone()),
        Value::Ts(ts) => Json::String(ts_wire(&ts.value, promote_naive)),
        Value::List(items) => Json::Array(
            items
                .iter()
                .map(|v| value_to_json(v, promote_naive))
                .collect(),
        ),
        Value::Map(inner) => {
            let mut out = Map::new();
            for (k, v) in inner {
                out.insert(k.clone(), value_to_json(v, false));
            }
            Json::Object(out)
        }
    }
}

/// A frontmatter payload: owned fields in declaration order, unknown keys after, then
/// `body` and `path` when supplied.
pub fn entry(meta: &Meta, order: &[&str], body: Option<&str>, path: Option<&Path>) -> Json {
    let mut out = Map::new();
    for key in order {
        if let Some(value) = meta.get(*key) {
            out.insert((*key).to_string(), value_to_json(value, is_timestamp(key)));
        }
    }
    for (key, value) in meta {
        if !order.contains(&key.as_str()) {
            out.insert(key.clone(), value_to_json(value, false));
        }
    }
    if let Some(b) = body {
        out.insert("body".to_string(), Json::String(b.to_string()));
    }
    if let Some(p) = path {
        out.insert("path".to_string(), Json::String(p.display().to_string()));
    }
    Json::Object(out)
}

fn is_timestamp(key: &str) -> bool {
    TIMESTAMP_FIELDS.contains(&key)
}

/// The seven-key `recent-activity` row: `id, type, title, path, mtime, owner, claimed_by`.
pub fn activity_row(path: &Path, meta: &Meta, mtime: f64) -> Json {
    let mut out = Map::new();
    let field = |key: &str| -> Json {
        meta.get(key)
            .map_or(Json::Null, |v| value_to_json(v, false))
    };
    out.insert("id".to_string(), field("id"));
    out.insert("type".to_string(), field("type"));
    out.insert("title".to_string(), field("title"));
    out.insert("path".to_string(), Json::String(path.display().to_string()));
    out.insert(
        "mtime".to_string(),
        serde_json::Number::from_f64(mtime).map_or(Json::Null, Json::Number),
    );
    out.insert("owner".to_string(), field("owner"));
    out.insert("claimed_by".to_string(), field("claimed_by"));
    Json::Object(out)
}

/// One search hit, with the conditional keys the contract specifies.
///
/// `--meta-only` drops `snippet` and beats `--full`; `--full` puts the whole body there.
pub fn hit(h: &crate::search::Hit, meta_only: bool, full: bool, space: Option<Space>) -> Json {
    let mut out = Map::new();
    out.insert(
        "id".to_string(),
        h.id.clone().map_or(Json::Null, Json::String),
    );
    out.insert(
        "type".to_string(),
        h.r#type.clone().map_or(Json::Null, Json::String),
    );
    out.insert(
        "title".to_string(),
        h.title.clone().map_or(Json::Null, Json::String),
    );
    out.insert(
        "score".to_string(),
        serde_json::Number::from_f64(h.score).map_or(Json::Null, Json::Number),
    );
    out.insert(
        "path".to_string(),
        Json::String(h.path.display().to_string()),
    );
    if !h.tags.is_empty() {
        out.insert(
            "tags".to_string(),
            Json::Array(h.tags.iter().map(|t| Json::String(t.clone())).collect()),
        );
    }
    if let Some(owner) = &h.owner {
        out.insert("owner".to_string(), Json::String(owner.clone()));
    }
    if let Some(updated) = &h.updated {
        out.insert(
            "updated".to_string(),
            Json::String(crate::timefmt::iso_z(updated)),
        );
    }
    if !meta_only {
        let _ = full;
        if let Some(snippet) = &h.snippet {
            out.insert("snippet".to_string(), Json::String(snippet.clone()));
        }
    }
    if let Some(space) = space {
        out.insert("space".to_string(), Json::String(space.name().to_string()));
    }
    Json::Object(out)
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
    use crate::model::common::BASE_FIELDS;

    #[test]
    fn entry_orders_owned_fields_then_stash_then_body_then_path() {
        let meta = parse_meta(
            "custom_key: keep me\ntitle: T\nid: n-1\ntype: note\ntags: []\nowner: null\n\
             created: 2026-01-02\nupdated: 2026-01-02T03:04:05\nrelated: []\nextra:\n  nested: v\n",
        )
        .unwrap();
        let json = entry(
            &meta,
            BASE_FIELDS,
            Some("body"),
            Some(Path::new("/v/n-1.md")),
        );
        let keys: Vec<&str> = json
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
                "tags",
                "owner",
                "created",
                "updated",
                "related",
                "custom_key",
                "extra",
                "body",
                "path"
            ]
        );
        assert_eq!(json["created"], Json::String("2026-01-02T00:00:00Z".into()));
        assert_eq!(json["updated"], Json::String("2026-01-02T03:04:05Z".into()));
        assert_eq!(json["owner"], Json::Null);
        assert_eq!(json["path"], Json::String("/v/n-1.md".into()));
    }

    #[test]
    fn stash_timestamps_keep_their_own_shape() {
        let meta = parse_meta("id: n-1\noffset_ts: 2026-01-02 03:04:05+02:00\nbare: 2026-01-02\n")
            .unwrap();
        let json = entry(&meta, BASE_FIELDS, None, None);
        assert_eq!(
            json["offset_ts"],
            Json::String("2026-01-02T03:04:05+02:00".into())
        );
        assert_eq!(json["bare"], Json::String("2026-01-02".into()));
    }

    #[test]
    fn activity_row_has_exactly_seven_keys() {
        let meta = parse_meta("id: n-1\ntype: note\ntitle: T\nowner: a\n").unwrap();
        let json = activity_row(Path::new("/v/n-1.md"), &meta, 1.5);
        let keys: Vec<&str> = json
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
        assert_eq!(json["claimed_by"], Json::Null);
    }
}
