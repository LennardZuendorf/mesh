//! The one tokenizer. Documents and queries go through the same function, always.
//!
//! Lowercase via `to_lowercase`, split on `!char::is_alphanumeric`, keep every non-empty run.
//! No stemming, no stop-words, no minimum length (final.md §7.2).

/// Every token in `text`, in order, with repeats.
pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// The query's token *set*: `tokenize` with an order-preserving dedupe.
pub fn token_set(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in tokenize(text) {
        if !out.contains(&token) {
            out.push(token);
        }
    }
    out
}

/// How often `token` occurs in `tokens`.
pub fn term_frequency(tokens: &[String], token: &str) -> usize {
    tokens.iter().filter(|t| t.as_str() == token).count()
}

/// The `#`-led heading lines of a Markdown body, hashes and leading whitespace stripped.
///
/// Fenced code is not parsed: a `#` inside a fence is a heading here, exactly as a naive
/// ranker would see it. That is deliberate — the headings field is a ranking hint, not a
/// Markdown AST.
pub fn headings(body: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            out.push(trimmed.trim_start_matches('#').trim());
        }
    }
    out.join("\n")
}

/// The body lowercased one code point at a time, so an index into the result is also an
/// index into `body.chars()`.
///
/// `str::to_lowercase` may map one code point to several (`İ`), which would shift every later
/// offset; taking only the first code point of each mapping keeps the 1:1 alignment a snippet
/// offset needs.
pub fn lower_chars(text: &str) -> Vec<char> {
    text.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// The code-point offset of the first occurrence of `needle` in `haystack`, both already
/// lowercased code point by code point.
pub fn find_chars(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .find(|start| haystack.get(*start..start + needle.len()) == Some(needle))
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
    fn tokenize_lowercases_and_splits_on_non_alphanumerics() {
        assert_eq!(
            tokenize("Hello, World! foo-bar_baz"),
            ["hello", "world", "foo", "bar", "baz"]
        );
    }

    #[test]
    fn tokenize_keeps_single_character_tokens_and_digits() {
        assert_eq!(tokenize("a b 1 22"), ["a", "b", "1", "22"]);
    }

    #[test]
    fn tokenize_keeps_unicode_alphanumerics() {
        assert_eq!(tokenize("Ünïcödé Tïtle"), ["ünïcödé", "tïtle"]);
    }

    #[test]
    fn tokenize_of_empty_or_punctuation_only_is_empty() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("---  !!! ").is_empty());
    }

    #[test]
    fn token_set_dedupes_and_preserves_order() {
        assert_eq!(token_set("beta alpha beta"), ["beta", "alpha"]);
    }

    #[test]
    fn term_frequency_counts_repeats() {
        let tokens = tokenize("zebra zebra horse");
        assert_eq!(term_frequency(&tokens, "zebra"), 2);
        assert_eq!(term_frequency(&tokens, "horse"), 1);
        assert_eq!(term_frequency(&tokens, "missing"), 0);
    }

    #[test]
    fn headings_strips_hashes_and_keeps_order() {
        let body = "intro\n# One\ntext\n### Three\n  ## Indented\nnot # a heading\n";
        assert_eq!(headings(body), "One\nThree\nIndented");
    }

    #[test]
    fn headings_of_a_bodyless_document_is_empty() {
        assert_eq!(headings(""), "");
        assert_eq!(headings("no headings here"), "");
    }

    #[test]
    fn find_chars_reports_a_code_point_offset() {
        let hay = lower_chars("Ünïcödé zebra here");
        let needle = lower_chars("ZEBRA");
        assert_eq!(find_chars(&hay, &needle), Some(8));
    }

    #[test]
    fn find_chars_misses_cleanly() {
        let hay = lower_chars("abc");
        assert_eq!(find_chars(&hay, &lower_chars("abcd")), None);
        assert_eq!(find_chars(&hay, &[]), Some(0));
    }
}
