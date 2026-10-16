//! Shared privacy helpers for local session importers.
//!
//! These are conservative text filters, not a substitute for a persistent
//! local-only sync policy. Session import commands must still pass their
//! sync-capture preflight before writing to PostgreSQL.

use once_cell::sync::Lazy;
use regex::Regex;
use sha2::{Digest, Sha256};
use uuid::Uuid;

static SECRET_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r#"(?i)\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|password|secret)\s*[:=]\s*['"]?[^\s,;"']+"#,
        r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{8,}",
        r"\b(?:sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{20,})\b",
        r"\bAKIA[0-9A-Z]{16}\b",
        r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).expect("valid session redaction pattern"))
    .collect()
});

/// Redact common credentials before excerpts or transcript text are persisted.
/// Returns the sanitized text and the number of matched secret spans.
pub fn redact_secrets(input: &str) -> (String, usize) {
    let mut text = input.to_owned();
    let mut redactions = 0;
    for pattern in SECRET_PATTERNS.iter() {
        redactions += pattern.find_iter(&text).count();
        text = pattern.replace_all(&text, "[REDACTED]").into_owned();
    }
    (text, redactions)
}

/// Stable UUID for per-source derived rows without adding a schema migration.
pub fn stable_source_uuid(kind: &str, source_key: &str) -> Uuid {
    let digest = Sha256::digest(format!("memory-session-v1:{kind}:{source_key}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Mark this deterministic value as a custom UUID version and RFC variant.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_named_tokens_bearer_keys_and_private_key_blocks() {
        let input = concat!(
            "api_key=sk-test01234567890123456789 ",
            "Authorization: Bearer abcdefghijklmnop ",
            "ghp_abcdefghijklmnopqrstuvwxyz123456 ",
            "AKIA1234567890ABCDEF\n",
            "-----BEGIN PRIVATE KEY-----\nsynthetic-key-material\n-----END PRIVATE KEY-----"
        );
        let (safe, count) = redact_secrets(input);
        assert!(count >= 5);
        for marker in [
            "sk-test01234567890123456789",
            "abcdefghijklmnop",
            "ghp_abcdefghijklmnopqrstuvwxyz123456",
            "AKIA1234567890ABCDEF",
            "synthetic-key-material",
        ] {
            assert!(!safe.contains(marker), "secret marker survived redaction");
        }
        assert!(safe.contains("[REDACTED]"));
    }

    #[test]
    fn stable_ids_are_repeatable_and_scoped_by_kind_and_source() {
        let first = stable_source_uuid("memory", "codex://2026/10/session.jsonl");
        assert_eq!(
            first,
            stable_source_uuid("memory", "codex://2026/10/session.jsonl")
        );
        assert_ne!(
            first,
            stable_source_uuid("experience", "codex://2026/10/session.jsonl")
        );
        assert_ne!(
            first,
            stable_source_uuid("memory", "codex://2026/10/other.jsonl")
        );
    }
}
