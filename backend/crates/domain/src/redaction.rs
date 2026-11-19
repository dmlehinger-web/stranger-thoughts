use serde::Serialize;
use std::collections::BTreeMap;

const SENSITIVE_KEYS: &[&str] = &[
    "email",
    "address",
    "line1",
    "line2",
    "postal",
    "token",
    "authorization",
    "secret",
    "password",
    "ciphertext",
    "data_key",
    "plaintext",
    "stripe",
    "destination",
    "refresh",
    "cvc",
    "pan",
];

pub fn is_sensitive_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    SENSITIVE_KEYS.iter().any(|needle| lowered.contains(needle))
}

pub fn redact_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if let Some(consumed) = secret_at(input, index) {
            out.push_str("[redacted]");
            index += consumed;
            continue;
        }
        if bytes[index] == b'@' {
            // Walk back over the local part we already copied, then skip the domain.
            let local = out
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-'))
                .count();
            let local_bytes: usize = out.chars().rev().take(local).map(|c| c.len_utf8()).sum();
            out.truncate(out.len() - local_bytes);
            index += 1;
            while index < bytes.len() && is_domain_byte(bytes[index]) {
                index += 1;
            }
            out.push_str("[email]");
            continue;
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

fn is_domain_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')
}

fn secret_at(input: &str, index: usize) -> Option<usize> {
    const PREFIXES: &[&str] = &[
        "sk_live_", "sk_test_", "rk_live_", "rk_test_", "pk_live_", "pk_test_", "whsec_",
    ];
    let rest = input.get(index..)?;
    let prefix = PREFIXES.iter().find(|prefix| rest.starts_with(*prefix))?;
    let mut end = prefix.len();
    let raw = rest.as_bytes();
    while end < raw.len() && (raw[end].is_ascii_alphanumeric() || raw[end] == b'_') {
        end += 1;
    }
    if end > prefix.len() {
        Some(end)
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SafeLog {
    pub message: String,
    pub fields: BTreeMap<String, String>,
}

impl SafeLog {
    pub fn new(message: impl AsRef<str>) -> Self {
        Self {
            message: redact_text(message.as_ref()),
            fields: BTreeMap::new(),
        }
    }

    pub fn field(mut self, key: &str, value: &str) -> Self {
        let stored = if is_sensitive_key(key) {
            "[redacted]".to_string()
        } else {
            redact_text(value)
        };
        self.fields.insert(key.to_string(), stored);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_email_secrets_and_sensitive_keys() {
        let log = SafeLog::new("login person@example.com used sk_live_abcDEF123")
            .field("line1", "PO Box 12")
            .field("match_id", "m-1");
        assert!(!log.message.contains("example.com"));
        assert!(!log.message.contains("sk_live_"));
        assert!(log.message.contains("[email]"));
        assert!(log.message.contains("[redacted]"));
        assert_eq!(log.fields.get("line1").map(String::as_str), Some("[redacted]"));
        assert_eq!(log.fields.get("match_id").map(String::as_str), Some("m-1"));
    }
}
