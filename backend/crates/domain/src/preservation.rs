//! Opt-in copies of a physical letter.
//!
//! Private copies stay with the person who wrote them. Archive submissions
//! start pending and uncurated. Nothing here is a mailing address, an email,
//! or a legal name, and the public site only renders records a person has
//! approved in the static archive.

use crate::redaction::redact_text;
use crate::validation::ValidationError;
use chrono::{DateTime, Utc};
use serde::Serialize;

pub const MAX_PRESERVED_CHARS: usize = 4_000;
pub const DEFAULT_PAGE_LIMIT: i32 = 10;
pub const MAX_PAGE_LIMIT: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Thought,
    Reflection,
}

impl ArchiveKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArchiveKind::Thought => "thought",
            ArchiveKind::Reflection => "reflection",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrivateCopyView {
    pub copy_id: String,
    pub match_id: String,
    pub body: String,
    pub saved_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveMineView {
    pub match_id: String,
    pub kind: String,
    pub status: String,
    pub curated: bool,
    pub paired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreservationPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

pub fn screen_preserved_text(input: &str) -> Result<String, ValidationError> {
    let cleaned = input.trim();
    let chars = cleaned.chars().count();
    if !(1..=MAX_PRESERVED_CHARS).contains(&chars) {
        return Err(ValidationError::Invalid { field: "body" });
    }
    if cleaned
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(ValidationError::Invalid { field: "body" });
    }
    let lowered = cleaned.to_ascii_lowercase();
    if cleaned.contains('@')
        || lowered.contains("http://")
        || lowered.contains("https://")
        || lowered.contains("www.")
    {
        return Err(ValidationError::Invalid { field: "body" });
    }
    if redact_text(cleaned) != cleaned {
        return Err(ValidationError::Invalid { field: "body" });
    }
    if contains_phone(cleaned) || contains_street(cleaned) || contains_po_box(&lowered) {
        return Err(ValidationError::Invalid { field: "body" });
    }
    Ok(cleaned.to_string())
}

pub fn page_limit(raw: Option<&str>) -> Result<i32, ValidationError> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(DEFAULT_PAGE_LIMIT),
        Some(value) => {
            let parsed: i32 = value
                .parse()
                .map_err(|_| ValidationError::Invalid { field: "limit" })?;
            if (1..=MAX_PAGE_LIMIT).contains(&parsed) {
                Ok(parsed)
            } else {
                Err(ValidationError::Invalid { field: "limit" })
            }
        }
    }
}

pub fn page_cursor(raw: Option<&str>) -> Result<Option<String>, ValidationError> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(None),
        Some(value) if valid_cursor(value) => Ok(Some(value.to_string())),
        Some(_) => Err(ValidationError::Invalid { field: "cursor" }),
    }
}

pub fn archive_mine_view(match_id: &str, kind: ArchiveKind, paired: bool) -> ArchiveMineView {
    ArchiveMineView {
        match_id: match_id.to_string(),
        kind: kind.as_str().to_string(),
        status: "pending".to_string(),
        curated: false,
        paired,
    }
}

fn valid_cursor(value: &str) -> bool {
    value.len() <= 80
        && (value.starts_with("copy#") || value.starts_with("archive#"))
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '#' || c == '-')
}

fn contains_phone(value: &str) -> bool {
    let digits: Vec<char> = value.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.len() >= 10 && digits.windows(10).any(|window| window.iter().all(|c| c.is_ascii_digit()))
        && value.chars().any(|c| matches!(c, '(' | ')' | '-'))
        && count_digit_runs(value) >= 3
}

fn count_digit_runs(value: &str) -> usize {
    let mut runs = 0;
    let mut in_run = false;
    for c in value.chars() {
        if c.is_ascii_digit() {
            if !in_run {
                runs += 1;
                in_run = true;
            }
        } else {
            in_run = false;
        }
    }
    runs
}

fn contains_street(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    const SUFFIXES: &[&str] = &[
        " street", " st.", " st ", " avenue", " ave.", " ave ", " road", " rd.", " rd ",
        " boulevard", " blvd", " lane", " ln.", " drive", " dr.", " court", " ct.", " way",
        " place", " pl.",
    ];
    SUFFIXES.iter().any(|suffix| lowered.contains(suffix))
        && lowered.chars().any(|c| c.is_ascii_digit())
}

fn contains_po_box(lowered: &str) -> bool {
    lowered.contains("po box") || lowered.contains("p.o. box") || lowered.contains("p o box")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_email_street_and_phone_and_keeps_a_plain_sentence() {
        let ok = screen_preserved_text("The second paragraph keeps circling the kitchen window.").unwrap();
        assert!(ok.contains("kitchen"));
        assert!(screen_preserved_text("write me at person@example.com").is_err());
        assert!(screen_preserved_text("12 Maple Street, apartment 4").is_err());
        assert!(screen_preserved_text("PO Box 18").is_err());
        assert!(screen_preserved_text("call 415-555-1212 tonight").is_err());
        assert!(screen_preserved_text("see https://example.com/secret").is_err());
    }

    #[test]
    fn archive_and_private_views_ban_pii_keys() {
        let copy = PrivateCopyView {
            copy_id: "match-1".into(),
            match_id: "match-1".into(),
            body: "A sentence with no name and no place.".into(),
            saved_at: Utc::now(),
        };
        let archive = archive_mine_view("match-1", ArchiveKind::Thought, false);
        assert!(!archive.curated);
        assert_eq!(archive.status, "pending");
        for value in [serde_json::to_value(copy).unwrap(), serde_json::to_value(archive).unwrap()] {
            assert!(crate::redaction::find_pii_key(&value).is_none(), "{value}");
        }
        let leaked = json!({"email": "a@b.c", "line1": "1"});
        assert!(crate::redaction::find_pii_key(&leaked).is_some());
    }

    #[test]
    fn page_limit_is_capped_and_cursor_is_narrow() {
        assert_eq!(page_limit(None).unwrap(), 10);
        assert_eq!(page_limit(Some("20")).unwrap(), 20);
        assert!(page_limit(Some("21")).is_err());
        assert!(page_limit(Some("0")).is_err());
        assert_eq!(page_cursor(Some("copy#match-1")).unwrap().as_deref(), Some("copy#match-1"));
        assert!(page_cursor(Some("../users")).is_err());
        assert!(page_cursor(Some("archive#*")).is_err());
    }
}
