//! Content fingerprinting: exact and normalized hashes.

use sha2::{Digest, Sha256};

/// SHA-256 of the raw text; identical bytes collide.
pub fn exact_hash(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// SHA-256 of normalized text: lowercased with whitespace collapsed.
///
/// Catches "Customer prefers email." vs "customer  prefers EMAIL" —
/// byte-different but semantically identical statements.
pub fn normalized_hash(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut last_ws = true;
    for c in text.chars() {
        if c.is_whitespace() {
            if !last_ws {
                normalized.push(' ');
                last_ws = true;
            }
        } else if c.is_alphanumeric() {
            // Punctuation and symbols are dropped: statements compare
            // on words, not on sentence furniture.
            normalized.extend(c.to_lowercase());
            last_ws = false;
        }
    }
    let trimmed = normalized.trim_end();
    hex(&Sha256::digest(trimmed.as_bytes()))
}

/// Both fingerprints at once.
pub fn text_fingerprint(text: &str) -> (String, String) {
    (exact_hash(text), normalized_hash(text))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_hash_is_stable_and_sensitive() {
        assert_eq!(exact_hash("hello"), exact_hash("hello"));
        assert_ne!(exact_hash("hello"), exact_hash("hello "));
        assert_eq!(exact_hash("").len(), 64);
    }

    #[test]
    fn normalized_hash_collapses_case_and_whitespace() {
        assert_eq!(
            normalized_hash("Customer prefers email."),
            normalized_hash("customer  PREFERS   email")
        );
        assert_ne!(
            normalized_hash("prefers email"),
            normalized_hash("prefers phone"),
            "different content must not collide"
        );
    }

    #[test]
    fn fingerprints_pair_consistently() {
        let (e, n) = text_fingerprint("Atlas uses PostgreSQL");
        assert_eq!(e, exact_hash("Atlas uses PostgreSQL"));
        assert_eq!(n, normalized_hash("atlas uses postgresql"));
    }
}
