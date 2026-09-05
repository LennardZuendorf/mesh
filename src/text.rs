//! The five hand-written text scanners plus the body-editing helpers.

/// How many code points a preview or snippet keeps.
pub const PREVIEW_CHARS: usize = 200;

/// Lowercase, runs of non-`[a-z0-9]` collapsed to `-`, trimmed of leading/trailing `-`.
pub fn slugify(text: &str) -> String {
    let lowered = text.trim().to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    out
}

/// The first `PREVIEW_CHARS` code points of a body, or all of it under `--full`.
pub fn preview(body: &str, full: bool) -> String {
    if full {
        body.to_string()
    } else {
        body.chars().take(PREVIEW_CHARS).collect()
    }
}

/// The head of a body as a snippet: trimmed, capped, `None` when empty.
pub fn snippet_head(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(PREVIEW_CHARS).collect())
}

/// A snippet window centred on the code-point offset `at`.
pub fn snippet_around(body: &str, at: usize) -> Option<String> {
    let chars: Vec<char> = body.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let half = PREVIEW_CHARS / 2;
    let start = at.saturating_sub(half).min(chars.len());
    let text: String = chars.iter().skip(start).take(PREVIEW_CHARS).collect();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Every `[[target]]` in body order, alias- and anchor-stripped, deduped, empties dropped.
pub fn link_targets(body: &str) -> Vec<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i + 1 < chars.len() {
        if chars.get(i) == Some(&'[') && chars.get(i + 1) == Some(&'[') {
            let mut j = i + 2;
            let mut raw = String::new();
            let mut closed = false;
            while j + 1 < chars.len() {
                let (a, b) = (chars.get(j), chars.get(j + 1));
                if a == Some(&']') && b == Some(&']') {
                    closed = true;
                    break;
                }
                match a {
                    Some('\n') | Some('[') | Some(']') | None => break,
                    Some(c) => raw.push(*c),
                }
                j += 1;
            }
            if closed && !raw.is_empty() {
                let target = normalise_target(&raw);
                if !target.is_empty() && !out.contains(&target) {
                    out.push(target);
                }
                i = j + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn normalise_target(text: &str) -> String {
    let base = text.split('|').next().unwrap_or("");
    let base = base.split('#').next().unwrap_or("");
    let base = base.split('^').next().unwrap_or("");
    base.trim().to_string()
}

/// `^[nmta]-[0-9A-Za-z]+$` — an id-form link target, resolved with no file lookup.
pub fn is_id_form(target: &str) -> bool {
    let mut chars = target.chars();
    let Some(prefix) = chars.next() else {
        return false;
    };
    if !matches!(prefix, 'n' | 'm' | 't' | 'a') {
        return false;
    }
    if chars.next() != Some('-') {
        return false;
    }
    let rest: Vec<char> = chars.collect();
    !rest.is_empty() && rest.iter().all(char::is_ascii_alphanumeric)
}

/// `^#{1,2}(?!#)\s` — a `#` or `##` heading, never `###`. Matched on the stripped line.
pub fn is_top_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 2 {
        return false;
    }
    trimmed.chars().nth(hashes).is_some_and(char::is_whitespace)
}

/// Append a block at the end of a body, separated by one blank line.
pub fn append_to_end(body: &str, block: &str) -> String {
    let base = body.trim_end_matches('\n');
    if base.is_empty() {
        block.to_string()
    } else {
        format!("{base}\n\n{block}")
    }
}

/// Append a block under `## {section}`, creating the section at the end when it is absent.
pub fn append_under_section(body: &str, block: &str, section: &str) -> String {
    let heading = format!("## {section}");
    let lines: Vec<&str> = body.split('\n').collect();
    let start = lines.iter().position(|l| l.trim() == heading);
    let Some(start) = start else {
        return append_to_end(body, &format!("{heading}\n\n{block}"));
    };
    let mut end = lines.len();
    for (offset, line) in lines.iter().enumerate().skip(start + 1) {
        if is_top_heading(line) {
            end = offset;
            break;
        }
    }
    let head = lines
        .iter()
        .take(end)
        .copied()
        .collect::<Vec<&str>>()
        .join("\n")
        .trim_end_matches('\n')
        .to_string();
    let tail = lines
        .iter()
        .skip(end)
        .copied()
        .collect::<Vec<&str>>()
        .join("\n")
        .trim_matches('\n')
        .to_string();
    let result = format!("{head}\n\n{block}");
    if tail.is_empty() {
        result
    } else {
        format!("{result}\n\n{tail}")
    }
}

/// `"{iso} — {actor}"`, or the bare ISO when there is no actor. The dash is U+2014.
pub fn format_stamp(iso: &str, actor: Option<&str>) -> String {
    match actor.filter(|a| !a.is_empty()) {
        Some(a) => format!("{iso} — {a}"),
        None => iso.to_string(),
    }
}

/// A body block, optionally preceded by a second-precision attribution stamp.
pub fn format_block(text: &str, timestamp: bool, actor: Option<&str>) -> String {
    if !timestamp {
        return text.to_string();
    }
    let iso = crate::timefmt::iso_seconds_z(&crate::timefmt::now_utc());
    format!("{}\n{}", format_stamp(&iso, actor), text)
}

/// Levenshtein distance over code points, used for the `candidates` hint on a not-found error.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut current: Vec<usize> = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        if let Some(slot) = current.first_mut() {
            *slot = i + 1;
        }
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let deletion = prev
                .get(j + 1)
                .copied()
                .unwrap_or(usize::MAX)
                .saturating_add(1);
            let insertion = current
                .get(j)
                .copied()
                .unwrap_or(usize::MAX)
                .saturating_add(1);
            let substitution = prev
                .get(j)
                .copied()
                .unwrap_or(usize::MAX)
                .saturating_add(cost);
            if let Some(slot) = current.get_mut(j + 1) {
                *slot = deletion.min(insertion).min(substitution);
            }
        }
        std::mem::swap(&mut prev, &mut current);
    }
    prev.last().copied().unwrap_or(0)
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

    #[test]
    fn slugify_matches_the_python_rule() {
        assert_eq!(slugify("  CLID  Fallback! "), "clid-fallback");
        assert_eq!(slugify("Plan: step 2"), "plan-step-2");
        assert_eq!(slugify("---"), "");
        assert_eq!(slugify("Ünicöde"), "nic-de");
        assert_eq!(slugify("c++"), "c");
    }

    #[test]
    fn preview_counts_code_points_not_bytes() {
        let body = "é".repeat(250);
        assert_eq!(preview(&body, false).chars().count(), 200);
        assert_eq!(preview(&body, true).chars().count(), 250);
        assert_eq!(snippet_head("  hi  ").as_deref(), Some("hi"));
        assert_eq!(snippet_head("   "), None);
    }

    #[test]
    fn link_targets_normalise_alias_and_anchor() {
        assert_eq!(link_targets("[[Note|display]]"), ["Note"]);
        assert_eq!(link_targets("[[Note#Section]]"), ["Note"]);
        assert_eq!(link_targets("[[Note^block]]"), ["Note"]);
        assert_eq!(link_targets("[[Note#Section|display]]"), ["Note"]);
        assert!(link_targets("[[#Heading]]").is_empty());
        assert_eq!(link_targets("a [[X]] b [[X]] c [[Y]]"), ["X", "Y"]);
        assert!(link_targets("[[unclosed").is_empty());
        assert!(link_targets("[[with\nnewline]]").is_empty());
    }

    #[test]
    fn id_form_covers_the_four_prefixes() {
        for good in ["n-1", "t-ABC", "m-9zZ", "a-7Q3KDX9M"] {
            assert!(is_id_form(good), "{good}");
        }
        for bad in ["x-1", "n-", "n1", "Alpha Note", "n-a b", ""] {
            assert!(!is_id_form(bad), "{bad}");
        }
    }

    #[test]
    fn top_heading_excludes_three_hashes() {
        assert!(is_top_heading("# Title"));
        assert!(is_top_heading("## Section"));
        assert!(is_top_heading("  ## Indented"));
        assert!(!is_top_heading("### Sub"));
        assert!(!is_top_heading("#NoSpace"));
        assert!(!is_top_heading("text"));
    }

    #[test]
    fn append_to_end_matches_the_table() {
        assert_eq!(append_to_end("", "X"), "X");
        assert_eq!(append_to_end("a\n\n", "X"), "a\n\nX");
    }

    #[test]
    fn append_under_section_matches_the_table() {
        let body = "Intro.\n\n## A\n\nitem1\n\n## B\n\nitem2";
        assert_eq!(
            append_under_section(body, "NEW", "A"),
            "Intro.\n\n## A\n\nitem1\n\nNEW\n\n## B\n\nitem2"
        );
        assert_eq!(
            append_under_section(body, "NEW", "Z"),
            "Intro.\n\n## A\n\nitem1\n\n## B\n\nitem2\n\n## Z\n\nNEW"
        );
        assert_eq!(
            append_under_section("# Title\n\ntext", "NEW", "Title"),
            "# Title\n\ntext\n\n## Title\n\nNEW"
        );
        assert_eq!(
            append_under_section("## A\n\n### sub\n\nx\n\n## B", "NEW", "A"),
            "## A\n\n### sub\n\nx\n\nNEW\n\n## B"
        );
    }

    #[test]
    fn stamps_degrade_to_a_bare_iso() {
        assert_eq!(
            format_stamp("2026-01-01T00:00:00Z", Some("alice")),
            "2026-01-01T00:00:00Z — alice"
        );
        assert_eq!(
            format_stamp("2026-01-01T00:00:00Z", None),
            "2026-01-01T00:00:00Z"
        );
        assert_eq!(format_block("text", false, Some("a")), "text");
        let stamped = format_block("text", true, Some("a"));
        assert!(stamped.ends_with("\ntext"));
        assert!(stamped.contains(" — a"));
        assert!(!stamped.contains('.'), "second precision only: {stamped}");
    }

    #[test]
    fn edit_distance_is_levenshtein() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("abc", "abd"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
    }
}
