//! The `--tags` grammar, shared by every space that has tags.

use crate::error::{MeshError, Result};

/// The one sentence describing the grammar. Byte-identical in the CLI help, the MCP schema
/// descriptions and the MCP instructions block.
pub const TAG_SPEC_SEMANTICS: &str =
    "Bare 'x,y' adds tags (additive, idempotent); '+x,-y' adds/removes; '=x,y' replaces the whole list.";

fn tokens(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// Apply a tag spec to an existing list.
///
/// `=x,y` replaces; `+x,-y` is a delta; a bare `x,y` is additive and idempotent. Mixing
/// prefixed and unprefixed tokens without a leading `=` is a validation error, raised before
/// anything is mutated.
pub fn apply_tag_spec(existing: &[String], spec: &str) -> Result<Vec<String>> {
    let trimmed = spec.trim();
    if let Some(rest) = trimmed.strip_prefix('=') {
        return Ok(dedupe(tokens(rest)));
    }
    let items = tokens(trimmed);
    let prefixed: Vec<bool> = items
        .iter()
        .map(|t| t.starts_with('+') || t.starts_with('-'))
        .collect();
    let is_delta = !items.is_empty() && prefixed.iter().all(|p| *p);
    if !items.is_empty() && prefixed.iter().any(|p| *p) && !is_delta {
        return Err(MeshError::Validation(format!(
            "ambiguous tag spec '{spec}': mixes prefixed (+/-) and unprefixed tokens with no \
             leading '='. Use '+x,-y' (delta), a bare 'x,y' (additive), or '=x,y' (explicit \
             replace) — not a mix of them."
        )));
    }
    let mut out: Vec<String> = existing.to_vec();
    if is_delta {
        for token in items {
            let mut chars = token.chars();
            let op = chars.next().unwrap_or('+');
            let name: String = chars.collect();
            if name.is_empty() {
                continue;
            }
            match op {
                '+' => {
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
                '-' => out.retain(|t| t != &name),
                _ => {}
            }
        }
        return Ok(out);
    }
    for token in items {
        if !out.contains(&token) {
            out.push(token);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn bare_is_additive_and_idempotent() {
        assert_eq!(
            apply_tag_spec(&v(&["a"]), "b,c").unwrap(),
            v(&["a", "b", "c"])
        );
        assert_eq!(apply_tag_spec(&v(&["a"]), "a").unwrap(), v(&["a"]));
        let once = apply_tag_spec(&v(&["a"]), "b").unwrap();
        assert_eq!(apply_tag_spec(&once, "b").unwrap(), once);
    }

    #[test]
    fn delta_adds_and_removes() {
        assert_eq!(
            apply_tag_spec(&v(&["a", "b"]), "+c,-a").unwrap(),
            v(&["b", "c"])
        );
        assert_eq!(apply_tag_spec(&v(&["a"]), "-zz").unwrap(), v(&["a"]));
        let x = apply_tag_spec(&v(&["a"]), "+x").unwrap();
        assert_eq!(apply_tag_spec(&x, "-x").unwrap(), v(&["a"]));
    }

    #[test]
    fn equals_replaces_and_clears() {
        assert_eq!(
            apply_tag_spec(&v(&["a", "b"]), "=x,y").unwrap(),
            v(&["x", "y"])
        );
        assert_eq!(
            apply_tag_spec(&v(&["a"]), "=").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(apply_tag_spec(&v(&[]), "=x,x").unwrap(), v(&["x"]));
    }

    #[test]
    fn mixed_specs_are_rejected_verbatim() {
        let err = apply_tag_spec(&v(&["a"]), "+x,y").unwrap_err();
        assert_eq!(err.code(), 2);
        assert_eq!(
            err.to_string(),
            "ambiguous tag spec '+x,y': mixes prefixed (+/-) and unprefixed tokens with no leading '='. Use '+x,-y' (delta), a bare 'x,y' (additive), or '=x,y' (explicit replace) — not a mix of them."
        );
    }

    #[test]
    fn only_the_first_character_is_a_prefix() {
        assert_eq!(
            apply_tag_spec(&v(&[]), "c++,sci-fi").unwrap(),
            v(&["c++", "sci-fi"])
        );
    }

    #[test]
    fn semantics_sentence_is_pinned() {
        assert_eq!(
            TAG_SPEC_SEMANTICS,
            "Bare 'x,y' adds tags (additive, idempotent); '+x,-y' adds/removes; '=x,y' replaces the whole list."
        );
    }
}
