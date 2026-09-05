//! Hash ids: Crockford base-32 over SHA-256, minimum four characters, extended on collision.

use sha2::{Digest, Sha256};

/// Crockford base-32 without I, L, O and U.
const CROCKFORD: [char; 32] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'J',
    'K', 'M', 'N', 'P', 'Q', 'R', 'S', 'T', 'V', 'W', 'X', 'Y', 'Z',
];

/// The shortest id body an entity gets.
pub const MIN_LENGTH: usize = 4;

/// Render a big-endian byte string as a Crockford base-32 integer, MSB first, unpadded.
fn crockford(bytes: &[u8]) -> String {
    let mut num = bytes.to_vec();
    let mut digits: Vec<char> = Vec::new();
    while num.iter().any(|b| *b != 0) {
        let mut remainder: u16 = 0;
        for byte in num.iter_mut() {
            let current = (remainder << 8) | u16::from(*byte);
            *byte = u8::try_from(current / 32).unwrap_or(0);
            remainder = current % 32;
        }
        digits.push(*CROCKFORD.get(remainder as usize).unwrap_or(&'0'));
    }
    if digits.is_empty() {
        return "0".to_string();
    }
    digits.reverse();
    digits.into_iter().collect()
}

fn extend(prefix: &str, full: &str, exists: &dyn Fn(&str) -> bool) -> String {
    let chars: Vec<char> = full.chars().collect();
    let max_length = chars.len().max(1);
    let mut length = MIN_LENGTH.min(max_length);
    loop {
        let body: String = chars.iter().take(length).collect();
        let candidate = format!("{prefix}{body}");
        if !exists(&candidate) || length >= max_length {
            return candidate;
        }
        length += 1;
    }
}

/// `SHA-256(created_iso + "\0" + title)` rendered in Crockford base-32 behind `prefix`.
///
/// `created_iso` is the `iso_z` form of the creation instant (overrides.md O2).
pub fn generate_id(
    prefix: &str,
    created_iso: &str,
    title: &str,
    exists: &dyn Fn(&str) -> bool,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(created_iso.as_bytes());
    hasher.update([0u8]);
    hasher.update(title.as_bytes());
    extend(prefix, &crockford(&hasher.finalize()), exists)
}

/// `"a-" + Crockford(sha256(bytes))` — an asset id is its content address.
pub fn content_id(bytes: &[u8], exists: &dyn Fn(&str) -> bool) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    extend("a-", &crockford(&hasher.finalize()), exists)
}

/// The full lowercase hex digest of `bytes` (the asset sidecar's `sha256`).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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

    fn never(_: &str) -> bool {
        false
    }

    #[test]
    fn default_length_is_prefix_plus_four() {
        let id = generate_id("n-", "2026-09-05T07:27:02.307028Z", "Alpha", &never);
        assert_eq!(id.chars().count(), 6);
        assert!(id.starts_with("n-"));
        assert!(id.chars().skip(2).all(|c| CROCKFORD.contains(&c)));
    }

    #[test]
    fn collision_extension_appends_exactly_one_char() {
        let base = generate_id("n-", "2026-09-05T07:27:02Z", "Alpha", &never);
        let taken = base.clone();
        let extended = generate_id("n-", "2026-09-05T07:27:02Z", "Alpha", &|c| c == taken);
        assert_eq!(extended.chars().count(), base.chars().count() + 1);
        assert!(extended.starts_with(&base));
    }

    #[test]
    fn ids_are_not_sequential() {
        let a = generate_id("n-", "2026-09-05T07:27:02Z", "A", &never);
        let b = generate_id("n-", "2026-09-05T07:27:02Z", "B", &never);
        assert_ne!(a, b);
    }

    #[test]
    fn the_digest_input_includes_a_nul_separator() {
        let a = generate_id("n-", "2026-01-01T00:00:00Z", "b", &never);
        let b = generate_id("n-", "2026-01-01T00:00:00Zb", "", &never);
        assert_ne!(a, b);
    }

    #[test]
    fn content_ids_are_stable_and_hex_matches() {
        let a = content_id(b"hello", &never);
        let b = content_id(b"hello", &never);
        assert_eq!(a, b);
        assert!(a.starts_with("a-"));
        assert_ne!(a, content_id(b"world", &never));
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn crockford_renders_msb_first_without_padding() {
        assert_eq!(crockford(&[0, 0, 0]), "0");
        assert_eq!(crockford(&[1]), "1");
        assert_eq!(crockford(&[32]), "10");
        assert_eq!(crockford(&[31]), "Z");
    }
}
