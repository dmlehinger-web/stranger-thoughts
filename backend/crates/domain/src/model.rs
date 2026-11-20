use std::collections::HashMap;

use crate::correspondence::{self, Turn};
use crate::ids::{MatchId, ReaderId, WriterId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One row in the ordered send log.
///
/// The log stores the match id, the two pseudonyms, timestamps, and the sent
/// checkbox. It does not store a mailing address. Address ciphertext lives on
/// the reader record and is decrypted only for the current match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchRecord {
    pub match_id: MatchId,
    pub writer_id: WriterId,
    pub writer_pseudonym: String,
    pub reader_id: ReaderId,
    pub reader_pseudonym: String,
    pub matched_at: DateTime<Utc>,
    pub sent: bool,
    pub current: bool,
}

/// What a writer sees in "My letters". No address and no reader id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LetterSummary {
    pub match_id: String,
    pub reader_pseudonym: String,
    pub matched_at: DateTime<Utc>,
    pub sent: bool,
    pub status: String,
    pub actions: Vec<String>,
    pub turns: Vec<correspondence::TurnView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurrentMatchView {
    pub match_id: String,
    pub reader_pseudonym: String,
    pub matched_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriterHomeView {
    pub pseudonym: String,
    pub current: Option<CurrentMatchView>,
    pub letters: Vec<LetterSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExpectedLetter {
    pub match_id: String,
    pub writer_pseudonym: String,
    pub matched_at: DateTime<Utc>,
    pub status: String,
    pub actions: Vec<String>,
    pub turns: Vec<correspondence::TurnView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReaderHomeView {
    pub pseudonym: String,
    pub letters: Vec<ExpectedLetter>,
    pub tips_received_cents: u64,
    pub payouts_ready: bool,
}

pub fn letter_log(history: &[MatchRecord], turns: &HashMap<String, Vec<Turn>>) -> Vec<LetterSummary> {
    let mut rows: Vec<LetterSummary> = history
        .iter()
        .map(|record| {
            let exchange = correspondence::exchange_for(
                crate::authz::Role::Writer,
                turns.get(record.match_id.as_str()).map(Vec::as_slice).unwrap_or(&[]),
            );
            LetterSummary {
                match_id: record.match_id.as_str().to_string(),
                reader_pseudonym: record.reader_pseudonym.clone(),
                matched_at: record.matched_at,
                sent: exchange.sent,
                status: exchange.status,
                actions: exchange.actions,
                turns: exchange.turns,
            }
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.matched_at));
    rows
}

pub fn writer_home(
    pseudonym: &str,
    history: &[MatchRecord],
    turns: &HashMap<String, Vec<Turn>>,
) -> WriterHomeView {
    let letters = letter_log(history, turns);
    let current = history.iter().find(|record| record.current).map(|record| {
        CurrentMatchView {
            match_id: record.match_id.as_str().to_string(),
            reader_pseudonym: record.reader_pseudonym.clone(),
            matched_at: record.matched_at,
        }
    });
    WriterHomeView {
        pseudonym: pseudonym.to_string(),
        current,
        letters,
    }
}

pub fn reader_expectations(
    history: &[MatchRecord],
    turns: &HashMap<String, Vec<Turn>>,
) -> Vec<ExpectedLetter> {
    let mut rows: Vec<ExpectedLetter> = history
        .iter()
        .map(|record| {
            let exchange = correspondence::exchange_for(
                crate::authz::Role::Reader,
                turns.get(record.match_id.as_str()).map(Vec::as_slice).unwrap_or(&[]),
            );
            ExpectedLetter {
                match_id: record.match_id.as_str().to_string(),
                writer_pseudonym: record.writer_pseudonym.clone(),
                matched_at: record.matched_at,
                status: exchange.status,
                actions: exchange.actions,
                turns: exchange.turns,
            }
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.matched_at));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::Value;

    fn sample() -> MatchRecord {
        MatchRecord {
            match_id: MatchId::new("match-1").unwrap(),
            writer_id: WriterId::new("writer-1").unwrap(),
            writer_pseudonym: "North Window".into(),
            reader_id: ReaderId::new("reader-1").unwrap(),
            reader_pseudonym: "Cedar".into(),
            matched_at: Utc.with_ymd_and_hms(2026, 11, 12, 15, 0, 0).unwrap(),
            sent: false,
            current: true,
        }
    }

    #[test]
    fn match_record_fields_exclude_address() {
        let value = serde_json::to_value(sample()).unwrap();
        let mut keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "current",
                "match_id",
                "matched_at",
                "reader_id",
                "reader_pseudonym",
                "sent",
                "writer_id",
                "writer_pseudonym",
            ]
        );
    }

    #[test]
    fn writer_home_json_has_no_address_or_email() {
        let view = writer_home("North Window", &[sample()], &HashMap::new());
        let raw = serde_json::to_string(&view).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert!(value.get("current").unwrap().get("reader_pseudonym").is_some());
        assert!(value.get("current").unwrap().get("reader_id").is_none());
        assert!(value.get("letters").unwrap()[0].get("sent").is_some());
        let lowered = raw.to_ascii_lowercase();
        for banned in ["line1", "postal", "ciphertext", "email", "@", "address"] {
            assert!(!lowered.contains(banned), "{banned} leaked into {raw}");
        }
        assert!(crate::redaction::find_pii_key(&value).is_none(), "{value}");
    }
}
