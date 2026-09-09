//! Every timestamp on every surface is rendered here. Nothing else formats a time.

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, Timelike, Utc};

use crate::error::{MeshError, Result};
use crate::fm::value::TsValue;

/// The wall clock. The only call site of `Utc::now`.
pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

fn seconds_and_micros(hour: u32, minute: u32, second: u32, micros: u32) -> String {
    if micros == 0 {
        format!("{hour:02}:{minute:02}:{second:02}")
    } else {
        format!("{hour:02}:{minute:02}:{second:02}.{micros:06}")
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.ffffff]Z` — the wire and human form for a UTC instant.
pub fn iso_z(dt: &DateTime<Utc>) -> String {
    let naive = dt.naive_utc();
    format!("{}Z", naive_iso(&naive))
}

/// `YYYY-MM-DDTHH:MM:SSZ` — second precision, used for body stamps.
pub fn iso_seconds_z(dt: &DateTime<Utc>) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    )
}

/// A naive datetime in `YYYY-MM-DDTHH:MM:SS[.ffffff]` form, no offset.
pub fn naive_iso(dt: &NaiveDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{}",
        dt.year(),
        dt.month(),
        dt.day(),
        seconds_and_micros(
            dt.hour(),
            dt.minute(),
            dt.second(),
            dt.and_utc().timestamp_subsec_micros()
        )
    )
}

/// Render a parsed on-disk timestamp for a JSON/human surface.
///
/// `promote_naive` reinterprets a bare date or naive datetime as UTC (the rule for the typed
/// `created`/`updated`/`expires` fields); unknown stash keys keep their own shape.
pub fn ts_wire(value: &TsValue, promote_naive: bool) -> String {
    match value {
        TsValue::Date(d) => {
            if promote_naive {
                format!("{:04}-{:02}-{:02}T00:00:00Z", d.year(), d.month(), d.day())
            } else {
                format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
            }
        }
        TsValue::Naive(dt) => {
            if promote_naive {
                format!("{}Z", naive_iso(dt))
            } else {
                naive_iso(dt)
            }
        }
        TsValue::Offset(dt) => {
            if dt.offset().local_minus_utc() == 0 {
                iso_z(&dt.with_timezone(&Utc))
            } else {
                format!("{}{}", naive_iso(&dt.naive_local()), offset_suffix(dt))
            }
        }
    }
}

fn offset_suffix(dt: &DateTime<FixedOffset>) -> String {
    let total = dt.offset().local_minus_utc();
    let sign = if total < 0 { '-' } else { '+' };
    let abs = total.abs();
    format!("{sign}{:02}:{:02}", abs / 3600, (abs % 3600) / 60)
}

/// The UTC instant a parsed on-disk timestamp denotes (naive read as UTC, never shifted).
pub fn ts_instant(value: &TsValue) -> DateTime<Utc> {
    match value {
        TsValue::Date(d) => d
            .and_hms_opt(0, 0, 0)
            .map_or_else(Utc::now, |n| n.and_utc()),
        TsValue::Naive(dt) => dt.and_utc(),
        TsValue::Offset(dt) => dt.with_timezone(&Utc),
    }
}

/// `<int>d|h|w` relative to now, else a lenient ISO date/datetime. Naive is read as UTC.
pub fn parse_since(value: &str) -> Result<DateTime<Utc>> {
    let text = value.trim();
    if let Some(delta) = parse_duration(text) {
        return Ok(now_utc() - delta);
    }
    match parse_iso_lenient(text) {
        Some(v) => Ok(ts_instant(&v)),
        None => Err(MeshError::Validation(format!(
            "invalid time value: '{text}'"
        ))),
    }
}

/// `^(\d+)([dhw])$` — no `m`, no `s`, no `y`, no sign, anchored at both ends.
fn parse_duration(text: &str) -> Option<Duration> {
    let mut chars = text.chars();
    let unit = text.chars().next_back()?;
    let digits: String = {
        let mut d = String::new();
        for _ in 0..text.chars().count().saturating_sub(1) {
            d.push(chars.next()?);
        }
        d
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: i64 = digits.parse().ok()?;
    match unit {
        'd' => Duration::try_days(n),
        'h' => Duration::try_hours(n),
        'w' => Duration::try_weeks(n),
        _ => None,
    }
}

/// Parse a scalar that may be a date or datetime.
///
/// Accepts `YYYY-MM-DD`, `YYYY-MM-DD[T ]HH:MM[:SS[.ffffff]]`, with an optional `Z` or `±HH:MM`
/// (`±HHMM` and `±HH` too). Returns `None` for anything else.
pub fn parse_iso_lenient(value: &str) -> Option<TsValue> {
    let text = value.trim();
    if text.len() < 10 {
        return None;
    }
    let (date_part, rest) = text.split_at_checked(10)?;
    let date = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
    if rest.is_empty() {
        return Some(TsValue::Date(date));
    }
    let sep = rest.chars().next()?;
    if sep != 'T' && sep != 't' && sep != ' ' {
        return None;
    }
    let rest = rest.get(1..)?;
    let (time_text, offset_text) = split_offset(rest);
    let time = parse_time(time_text)?;
    let naive = date.and_time(time);
    match offset_text {
        None => Some(TsValue::Naive(naive)),
        Some(off) => {
            let offset = parse_offset(off)?;
            let dt = naive.and_local_timezone(offset).single()?;
            Some(TsValue::Offset(dt))
        }
    }
}

fn split_offset(rest: &str) -> (&str, Option<&str>) {
    if let Some(stripped) = rest.strip_suffix('Z').or_else(|| rest.strip_suffix('z')) {
        return (stripped, Some("+00:00"));
    }
    // Look for a sign after the time, i.e. not at index 0.
    let bytes = rest.as_bytes();
    for (i, b) in bytes.iter().enumerate().skip(1) {
        if *b == b'+' || *b == b'-' {
            if let (Some(head), Some(tail)) = (rest.get(..i), rest.get(i..)) {
                return (head, Some(tail));
            }
        }
    }
    (rest, None)
}

fn parse_time(text: &str) -> Option<chrono::NaiveTime> {
    for fmt in ["%H:%M:%S%.f", "%H:%M:%S", "%H:%M"] {
        if let Ok(t) = chrono::NaiveTime::parse_from_str(text, fmt) {
            return Some(t);
        }
    }
    None
}

fn parse_offset(text: &str) -> Option<FixedOffset> {
    let (sign, rest) = match text.chars().next()? {
        '+' => (1, text.get(1..)?),
        '-' => (-1, text.get(1..)?),
        _ => return None,
    };
    let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
    let (h, m) = match digits.len() {
        2 => (digits.parse::<i32>().ok()?, 0),
        4 => (
            digits.get(..2)?.parse::<i32>().ok()?,
            digits.get(2..)?.parse::<i32>().ok()?,
        ),
        6 => (
            digits.get(..2)?.parse::<i32>().ok()?,
            digits.get(2..4)?.parse::<i32>().ok()?,
        ),
        _ => return None,
    };
    FixedOffset::east_opt(sign * (h * 3600 + m * 60))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn dt(s: &str) -> DateTime<Utc> {
        match parse_iso_lenient(s).unwrap() {
            TsValue::Offset(d) => d.with_timezone(&Utc),
            TsValue::Naive(d) => d.and_utc(),
            TsValue::Date(d) => d.and_hms_opt(0, 0, 0).unwrap().and_utc(),
        }
    }

    #[test]
    fn iso_z_omits_zero_microseconds() {
        assert_eq!(iso_z(&dt("2026-09-05T07:27:02Z")), "2026-09-05T07:27:02Z");
        assert_eq!(
            iso_z(&dt("2026-09-05T07:27:02.307028Z")),
            "2026-09-05T07:27:02.307028Z"
        );
        assert_eq!(
            iso_seconds_z(&dt("2026-09-05T07:27:02.307028Z")),
            "2026-09-05T07:27:02Z"
        );
    }

    #[test]
    fn lenient_parse_accepts_the_python_shapes() {
        assert!(matches!(
            parse_iso_lenient("2026-01-02"),
            Some(TsValue::Date(_))
        ));
        assert!(matches!(
            parse_iso_lenient("2026-01-02T03:04:05"),
            Some(TsValue::Naive(_))
        ));
        assert!(matches!(
            parse_iso_lenient("2026-09-05 07:27:02.307028+00:00"),
            Some(TsValue::Offset(_))
        ));
        assert!(matches!(
            parse_iso_lenient("2026-01-02 03:04:05+02:00"),
            Some(TsValue::Offset(_))
        ));
        assert!(matches!(
            parse_iso_lenient("2026-01-02T03:04:05Z"),
            Some(TsValue::Offset(_))
        ));
        assert!(parse_iso_lenient("not a date").is_none());
        assert!(parse_iso_lenient("2026-13-40").is_none());
        assert!(parse_iso_lenient("hello world here").is_none());
        assert!(parse_iso_lenient("").is_none());
    }

    #[test]
    fn ts_wire_promotes_only_when_asked() {
        let d = parse_iso_lenient("2026-01-02").unwrap();
        assert_eq!(ts_wire(&d, true), "2026-01-02T00:00:00Z");
        assert_eq!(ts_wire(&d, false), "2026-01-02");
        let n = parse_iso_lenient("2026-01-02T03:04:05").unwrap();
        assert_eq!(ts_wire(&n, true), "2026-01-02T03:04:05Z");
        assert_eq!(ts_wire(&n, false), "2026-01-02T03:04:05");
        let o = parse_iso_lenient("2026-01-02 03:04:05+02:00").unwrap();
        assert_eq!(ts_wire(&o, true), "2026-01-02T03:04:05+02:00");
        let u = parse_iso_lenient("2026-09-05 08:45:37.739036+00:00").unwrap();
        assert_eq!(ts_wire(&u, true), "2026-09-05T08:45:37.739036Z");
    }

    #[test]
    fn since_grammar_is_dhw_only() {
        let now = now_utc();
        let d = parse_since("7d").unwrap();
        let delta = (now - d).num_seconds();
        assert!(
            (7 * 86_400 - 2..=7 * 86_400 + 2).contains(&delta),
            "delta {delta}"
        );
        assert!(parse_since("12h").is_ok());
        assert!(parse_since("2w").is_ok());
        assert!(parse_since("5m").is_err());
        assert!(parse_since("5s").is_err());
        assert!(parse_since("1y").is_err());
        assert!(parse_since("-1d").is_err());
        assert!(parse_since("2026-07-01").is_ok());
        assert!(parse_since("2026-07-01T12:00:00+02:00").is_ok());
        assert!(parse_since("bogus").is_err());
    }
}
