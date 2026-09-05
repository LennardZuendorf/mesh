//! The canonical frontmatter emitter: deterministic, declaration-ordered, no anchors.
//!
//! Mesh writes its own YAML dialect (overrides.md O2): keys in `Meta` order, RFC 3339 `Z`
//! timestamps for values we set, block lists indented two spaces, double quotes when a plain
//! scalar would be ambiguous. Values read from disk and not modified re-emit verbatim.

use crate::fm::value::{Meta, Value};

const INDICATORS: &[char] = &[
    '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`',
];

/// Render a frontmatter map as the YAML block that goes between the `---` fences.
///
/// The returned string always ends with a newline.
pub fn emit_meta(meta: &Meta) -> String {
    if meta.is_empty() {
        return "{}\n".to_string();
    }
    let mut out = String::new();
    emit_map(&mut out, meta, 0);
    out
}

fn emit_map(out: &mut String, meta: &Meta, indent: usize) {
    let pad = " ".repeat(indent);
    for (key, value) in meta {
        match value {
            Value::List(items) if !items.is_empty() => {
                out.push_str(&pad);
                out.push_str(&quote_key(key));
                out.push_str(":\n");
                emit_list(out, items, indent + 2);
            }
            Value::Map(inner) if !inner.is_empty() => {
                out.push_str(&pad);
                out.push_str(&quote_key(key));
                out.push_str(":\n");
                emit_map(out, inner, indent + 2);
            }
            other => {
                out.push_str(&pad);
                out.push_str(&quote_key(key));
                out.push_str(": ");
                out.push_str(&scalar(other));
                out.push('\n');
            }
        }
    }
}

fn emit_list(out: &mut String, items: &[Value], indent: usize) {
    let pad = " ".repeat(indent);
    for item in items {
        match item {
            Value::Map(inner) if !inner.is_empty() => {
                let mut nested = String::new();
                emit_map(&mut nested, inner, indent + 2);
                let mut lines = nested.split_inclusive('\n');
                match lines.next() {
                    Some(first) => {
                        out.push_str(&pad);
                        out.push_str("- ");
                        out.push_str(first.trim_start());
                        for line in lines {
                            out.push_str(line);
                        }
                    }
                    None => {
                        out.push_str(&pad);
                        out.push_str("- {}\n");
                    }
                }
            }
            Value::List(inner) if !inner.is_empty() => {
                out.push_str(&pad);
                out.push_str("-\n");
                emit_list(out, inner, indent + 2);
            }
            other => {
                out.push_str(&pad);
                out.push_str("- ");
                out.push_str(&scalar(other));
                out.push('\n');
            }
        }
    }
}

fn quote_key(key: &str) -> String {
    if needs_quotes(key) {
        double_quote(key)
    } else {
        key.to_string()
    }
}

/// Render a non-container value as a single-line YAML scalar.
pub fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => float_text(*f),
        Value::Ts(ts) => ts.raw.clone(),
        Value::Str(s) => {
            if needs_quotes(s) {
                double_quote(s)
            } else {
                s.clone()
            }
        }
        Value::List(_) => "[]".to_string(),
        Value::Map(_) => "{}".to_string(),
    }
}

fn float_text(f: f64) -> String {
    if f.is_nan() {
        return ".nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            ".inf".to_string()
        } else {
            "-.inf".to_string()
        };
    }
    let text = format!("{f}");
    if text.contains(['.', 'e', 'E']) {
        text
    } else {
        format!("{text}.0")
    }
}

/// True when a plain scalar would be ambiguous, empty, or re-resolve to another type.
fn needs_quotes(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    if text.trim() != text {
        return true;
    }
    if text.contains('\n') || text.contains('\r') || text.contains('\t') {
        return true;
    }
    if text.starts_with(INDICATORS) {
        return true;
    }
    if text.contains(": ") || text.contains(" #") || text.ends_with(':') {
        return true;
    }
    resolves_to_non_string(text)
}

fn resolves_to_non_string(text: &str) -> bool {
    matches!(
        text,
        "null"
            | "Null"
            | "NULL"
            | "~"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | "yes"
            | "Yes"
            | "YES"
            | "no"
            | "No"
            | "NO"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
    ) || text.parse::<i64>().is_ok()
        || text.parse::<f64>().is_ok()
        || crate::timefmt::parse_iso_lenient(text).is_some()
}

fn double_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
    use crate::fm::load::parse_meta;
    use crate::fm::value::{Ts, TsValue, Value};

    fn meta(pairs: Vec<(&str, Value)>) -> Meta {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    #[test]
    fn emits_declaration_order_not_alphabetical() {
        let m = meta(vec![
            ("id", Value::str("n-1")),
            ("type", Value::str("note")),
            ("title", Value::str("Alpha")),
        ]);
        assert_eq!(emit_meta(&m), "id: n-1\ntype: note\ntitle: Alpha\n");
    }

    #[test]
    fn lists_and_maps_indent_two_spaces() {
        let m = meta(vec![
            ("tags", Value::strings(["a", "b"])),
            ("empty", Value::List(vec![])),
            ("extra", Value::Map(meta(vec![("k", Value::str("v"))]))),
        ]);
        assert_eq!(
            emit_meta(&m),
            "tags:\n  - a\n  - b\nempty: []\nextra:\n  k: v\n"
        );
    }

    #[test]
    fn quotes_only_what_must_be_quoted() {
        let cases = [
            ("plain", "plain"),
            ("a: b", "\"a: b\""),
            ("#x", "\"#x\""),
            ("yes", "\"yes\""),
            ("null", "\"null\""),
            ("12", "\"12\""),
            ("1.5", "\"1.5\""),
            ("", "\"\""),
            ("2026-01-02", "\"2026-01-02\""),
            (" lead", "\" lead\""),
            ("- dash", "\"- dash\""),
            ("he said \"hi\"", "he said \"hi\""),
            ("\"quoted\"", "\"\\\"quoted\\\"\""),
            ("two\nlines", "\"two\\nlines\""),
            ("héllo ünicode", "héllo ünicode"),
            ("c++", "c++"),
            ("Hello: World", "\"Hello: World\""),
            ("ends:", "\"ends:\""),
        ];
        for (input, want) in cases {
            assert_eq!(scalar(&Value::str(input)), want, "input {input:?}");
        }
    }

    #[test]
    fn timestamps_reemit_verbatim() {
        let raw = "2026-09-05 07:27:02.307028+00:00";
        let ts = Value::Ts(Ts::new(
            raw,
            crate::timefmt::parse_iso_lenient(raw).unwrap(),
        ));
        assert_eq!(scalar(&ts), raw);
        let ours = Value::Ts(Ts::new(
            "2026-09-05T07:27:02Z",
            TsValue::Offset(chrono::DateTime::parse_from_rfc3339("2026-09-05T07:27:02Z").unwrap()),
        ));
        assert_eq!(scalar(&ours), "2026-09-05T07:27:02Z");
    }

    #[test]
    fn round_trips_through_the_loader() {
        let m = meta(vec![
            ("id", Value::str("n-1")),
            ("nothing", Value::Null),
            ("flag", Value::Bool(false)),
            ("count", Value::Int(3)),
            ("ratio", Value::Float(0.5)),
            ("tags", Value::strings(["a", "b"])),
            ("empty", Value::List(vec![])),
            (
                "extra",
                Value::Map(meta(vec![("nested", Value::str("yes"))])),
            ),
            ("weird", Value::str("a: b")),
        ]);
        let text = emit_meta(&m);
        let back = parse_meta(&text).unwrap();
        assert_eq!(back, m);
        assert_eq!(emit_meta(&back), text, "dump is idempotent");
    }

    #[test]
    fn empty_meta_is_a_flow_map() {
        assert_eq!(emit_meta(&Meta::new()), "{}\n");
    }
}
