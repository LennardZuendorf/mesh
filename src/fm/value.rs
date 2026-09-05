//! The ordered frontmatter map and its scalar type.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};
use indexmap::IndexMap;

/// Frontmatter: an insertion-ordered map. Order on disk is this map's order.
pub type Meta = IndexMap<String, Value>;

/// A date or datetime scalar as it was written on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TsValue {
    /// `YYYY-MM-DD`.
    Date(NaiveDate),
    /// `YYYY-MM-DD[T ]HH:MM:SS[.ffffff]` with no offset.
    Naive(NaiveDateTime),
    /// The same with a `Z` or `±HH:MM` offset.
    Offset(DateTime<FixedOffset>),
}

/// A timestamp scalar plus the exact text it was read from.
///
/// A `Ts` we did not modify re-emits `raw` verbatim, which is what makes a Python-written
/// vault round-trip without the emitter having to reproduce PyYAML's formatting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ts {
    /// The scalar exactly as it appeared on disk.
    pub raw: String,
    /// The parsed value.
    pub value: TsValue,
}

impl Ts {
    /// Build a `Ts` from raw text plus its parse.
    pub fn new(raw: impl Into<String>, value: TsValue) -> Self {
        Ts {
            raw: raw.into(),
            value,
        }
    }
}

/// A frontmatter value. Quoted scalars are always `Str`, never `Ts`.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Ts(Ts),
    List(Vec<Value>),
    Map(Meta),
}

impl Value {
    /// A borrowed `&str` when this is a plain string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// The scalar coerced to a comparison string: strings, timestamps (raw), bools and ints.
    pub fn as_scalar_text(&self) -> Option<String> {
        match self {
            Value::Str(s) => Some(s.clone()),
            Value::Ts(t) => Some(t.raw.clone()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Int(i) => Some(i.to_string()),
            _ => None,
        }
    }

    /// The integer value when this is an int.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// The list of strings when this is a list; non-string members are dropped.
    pub fn as_str_list(&self) -> Option<Vec<String>> {
        match self {
            Value::List(items) => Some(
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
            ),
            _ => None,
        }
    }

    /// The timestamp when this is one.
    pub fn as_ts(&self) -> Option<&Ts> {
        match self {
            Value::Ts(t) => Some(t),
            _ => None,
        }
    }

    /// True when this is `Value::Null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// A string value.
    pub fn str(s: impl Into<String>) -> Value {
        Value::Str(s.into())
    }

    /// A list of string values.
    pub fn strings<I, S>(items: I) -> Value
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Value::List(items.into_iter().map(|s| Value::Str(s.into())).collect())
    }
}
