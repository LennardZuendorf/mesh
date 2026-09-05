//! The frontmatter reader: split the `---` block, then parse it with yaml-rust2's event API.

use std::collections::HashMap;

use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::{Marker, TScalarStyle};

use crate::fm::value::{Meta, Ts, Value};
use crate::timefmt::parse_iso_lenient;

/// Split a document into its frontmatter block and its body.
///
/// Returns `(Some(yaml), body)` when the text opens with a `---` line and has a closing one;
/// `(None, body)` for foreign Markdown. The body is whitespace-trimmed on both ends, matching
/// `python-frontmatter`.
pub fn split_frontmatter(text: &str) -> (Option<String>, String) {
    let stripped = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = stripped.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (None, String::new());
    };
    if first.trim_end() != "---" {
        return (None, stripped.trim().to_string());
    }
    let mut yaml = String::new();
    for line in lines.by_ref() {
        let t = line.trim_end();
        if t == "---" || t == "..." {
            let body: String = lines.collect();
            return (Some(yaml), body.trim().to_string());
        }
        yaml.push_str(line);
    }
    // No closing delimiter: not frontmatter at all.
    (None, stripped.trim().to_string())
}

/// Parse a frontmatter YAML block into an ordered `Meta`. `None` on malformed YAML.
pub fn parse_meta(yaml: &str) -> Option<Meta> {
    if yaml.trim().is_empty() {
        return Some(Meta::new());
    }
    let mut sink = Loader::default();
    let mut parser = Parser::new_from_str(yaml);
    parser.load(&mut sink, false).ok()?;
    match sink.docs.into_iter().next() {
        Some(Value::Map(m)) => Some(m),
        Some(Value::Null) | None => Some(Meta::new()),
        Some(_) => None,
    }
}

/// Resolve a plain scalar the way YAML 1.2 does, with a lenient timestamp pass on top.
fn resolve_plain(text: &str) -> Value {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(i) = text.parse::<i64>() {
        return Value::Int(i);
    }
    if let Some(hex) = text.strip_prefix("0x") {
        if let Ok(i) = i64::from_str_radix(hex, 16) {
            return Value::Int(i);
        }
    }
    if let Ok(f) = text.parse::<f64>() {
        if text.contains(['.', 'e', 'E']) {
            return Value::Float(f);
        }
    }
    if let Some(ts) = parse_iso_lenient(text) {
        return Value::Ts(Ts::new(text, ts));
    }
    Value::Str(text.to_string())
}

#[derive(Default)]
struct Loader {
    docs: Vec<Value>,
    stack: Vec<Node>,
    anchors: HashMap<usize, Value>,
}

enum Node {
    Seq(usize, Vec<Value>),
    /// (anchor, entries, pending key)
    Map(usize, Meta, Option<String>),
}

impl Loader {
    fn push(&mut self, value: Value, anchor: usize) {
        if anchor > 0 {
            self.anchors.insert(anchor, value.clone());
        }
        match self.stack.last_mut() {
            None => self.docs.push(value),
            Some(Node::Seq(_, items)) => items.push(value),
            Some(Node::Map(_, entries, pending)) => match pending.take() {
                None => {
                    let key = match &value {
                        Value::Str(s) => s.clone(),
                        Value::Ts(t) => t.raw.clone(),
                        Value::Int(i) => i.to_string(),
                        Value::Bool(b) => b.to_string(),
                        Value::Null => "null".to_string(),
                        _ => String::new(),
                    };
                    *pending = Some(key);
                }
                Some(key) => {
                    entries.insert(key, value);
                }
            },
        }
    }
}

impl MarkedEventReceiver for Loader {
    fn on_event(&mut self, ev: Event, _mark: Marker) {
        match ev {
            Event::Scalar(text, style, anchor, _tag) => {
                let value = match style {
                    TScalarStyle::Plain => resolve_plain(&text),
                    _ => Value::Str(text),
                };
                self.push(value, anchor);
            }
            Event::SequenceStart(anchor, _) => self.stack.push(Node::Seq(anchor, Vec::new())),
            Event::MappingStart(anchor, _) => self.stack.push(Node::Map(anchor, Meta::new(), None)),
            Event::SequenceEnd => {
                if let Some(Node::Seq(anchor, items)) = self.stack.pop() {
                    self.push(Value::List(items), anchor);
                }
            }
            Event::MappingEnd => {
                if let Some(Node::Map(anchor, entries, _)) = self.stack.pop() {
                    self.push(Value::Map(entries), anchor);
                }
            }
            Event::Alias(anchor) => {
                let value = self.anchors.get(&anchor).cloned().unwrap_or(Value::Null);
                self.push(value, 0);
            }
            _ => {}
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
    use crate::fm::value::TsValue;

    #[test]
    fn splits_a_mesh_document() {
        let text = "---\nid: n-1\n---\n\nbody text\n";
        let (yaml, body) = split_frontmatter(text);
        assert_eq!(yaml.as_deref(), Some("id: n-1\n"));
        assert_eq!(body, "body text");
    }

    #[test]
    fn foreign_markdown_has_no_frontmatter() {
        let (yaml, body) = split_frontmatter("# Heading\n\ntext\n");
        assert!(yaml.is_none());
        assert_eq!(body, "# Heading\n\ntext");
    }

    #[test]
    fn unterminated_block_is_not_frontmatter() {
        let (yaml, body) = split_frontmatter("---\nid: n-1\nbody\n");
        assert!(yaml.is_none());
        assert!(body.starts_with("---"));
    }

    #[test]
    fn parses_python_written_scalars() {
        let meta = parse_meta(
            "created: 2026-09-05 07:27:02.307028+00:00\n\
             id: t-1S4Z\n\
             priority: null\n\
             blocked_by: []\n\
             tags:\n- x\n- y\n\
             quoted: '2026-01-02'\n\
             bare_date: 2026-01-02\n\
             naive: 2026-01-02T03:04:05\n\
             count: 12\n\
             flag: true\n\
             ratio: 1.5\n",
        )
        .unwrap();
        assert!(matches!(meta["created"], Value::Ts(_)));
        assert_eq!(meta["id"].as_str(), Some("t-1S4Z"));
        assert!(meta["priority"].is_null());
        assert_eq!(meta["blocked_by"].as_str_list(), Some(vec![]));
        assert_eq!(
            meta["tags"].as_str_list(),
            Some(vec!["x".into(), "y".into()])
        );
        assert_eq!(meta["quoted"].as_str(), Some("2026-01-02"));
        assert!(matches!(
            meta["bare_date"].as_ts().unwrap().value,
            TsValue::Date(_)
        ));
        assert!(matches!(
            meta["naive"].as_ts().unwrap().value,
            TsValue::Naive(_)
        ));
        assert_eq!(meta["count"].as_int(), Some(12));
        assert_eq!(meta["flag"], Value::Bool(true));
        assert!(matches!(meta["ratio"], Value::Float(_)));
    }

    #[test]
    fn key_order_is_preserved() {
        let meta = parse_meta("z: 1\na: 2\nm: 3\n").unwrap();
        let keys: Vec<&str> = meta.keys().map(String::as_str).collect();
        assert_eq!(keys, ["z", "a", "m"]);
    }

    #[test]
    fn anchors_are_resolved() {
        let meta =
            parse_meta("created: &id001 2026-01-02 03:04:05+00:00\nupdated: *id001\n").unwrap();
        assert_eq!(meta["created"], meta["updated"]);
    }

    #[test]
    fn nested_maps_load() {
        let meta = parse_meta("extra:\n  nested: yes\n  deep:\n    k: v\n").unwrap();
        let Value::Map(inner) = &meta["extra"] else {
            panic!("not a map")
        };
        assert_eq!(inner["nested"].as_str(), Some("yes"));
    }

    #[test]
    fn malformed_yaml_is_none() {
        assert!(parse_meta("title: [unclosed\n").is_none());
    }

    #[test]
    fn empty_block_is_an_empty_map() {
        assert_eq!(parse_meta("").unwrap().len(), 0);
        assert_eq!(parse_meta("{}").unwrap().len(), 0);
    }
}
