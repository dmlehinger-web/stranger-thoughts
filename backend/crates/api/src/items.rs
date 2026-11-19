use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;
use chrono::{DateTime, Utc};
use stranger_domain::address::SealedAddress;
use stranger_domain::ids::{MatchId, ReaderId, WriterId};
use stranger_domain::matching::MatchCandidate;
use stranger_domain::model::MatchRecord;
use stranger_domain::validation::{AgeRange, Demographics, EconomicStatus, Race};

use crate::store::StoreError;

pub fn s(value: impl Into<String>) -> AttributeValue {
    AttributeValue::S(value.into())
}

pub fn match_item(record: &MatchRecord) -> HashMap<String, AttributeValue> {
    HashMap::from([
        ("writerId".into(), s(record.writer_id.as_str())),
        ("matchedAt".into(), s(record.matched_at.to_rfc3339())),
        ("matchId".into(), s(record.match_id.as_str())),
        ("writerPseudonym".into(), s(&record.writer_pseudonym)),
        ("readerId".into(), s(record.reader_id.as_str())),
        ("readerPseudonym".into(), s(&record.reader_pseudonym)),
        ("sent".into(), AttributeValue::Bool(record.sent)),
        ("current".into(), AttributeValue::Bool(record.current)),
    ])
}

pub fn match_from_item(item: &HashMap<String, AttributeValue>) -> Result<MatchRecord, StoreError> {
    let matched_at = attr_s(item, "matchedAt").ok_or(StoreError)?;
    Ok(MatchRecord {
        match_id: MatchId::new(attr_s(item, "matchId").ok_or(StoreError)?).map_err(|_| StoreError)?,
        writer_id: WriterId::new(attr_s(item, "writerId").ok_or(StoreError)?).map_err(|_| StoreError)?,
        writer_pseudonym: attr_s(item, "writerPseudonym").ok_or(StoreError)?,
        reader_id: ReaderId::new(attr_s(item, "readerId").ok_or(StoreError)?).map_err(|_| StoreError)?,
        reader_pseudonym: attr_s(item, "readerPseudonym").ok_or(StoreError)?,
        matched_at: DateTime::parse_from_rfc3339(&matched_at)
            .map_err(|_| StoreError)?
            .with_timezone(&Utc),
        sent: attr_bool(item, "sent"),
        current: attr_bool(item, "current"),
    })
}

pub fn candidate_from_item(item: &HashMap<String, AttributeValue>) -> Result<MatchCandidate, StoreError> {
    Ok(MatchCandidate {
        reader_id: ReaderId::new(attr_s(item, "readerId").ok_or(StoreError)?).map_err(|_| StoreError)?,
        pseudonym: attr_s(item, "pseudonym").unwrap_or_default(),
        active: attr_s(item, "activePk").as_deref() == Some("ACTIVE"),
        blocked_writer_ids: attr_ss(item, "blockedWriterIds")
            .into_iter()
            .filter_map(|value| WriterId::new(value).ok())
            .collect(),
    })
}

pub fn sealed_from_item(
    reader_id: &str,
    item: &HashMap<String, AttributeValue>,
) -> Result<SealedAddress, StoreError> {
    Ok(SealedAddress {
        reader_id: reader_id.to_string(),
        ciphertext: attr_s(item, "ciphertext").ok_or(StoreError)?,
        encrypted_data_key: attr_s(item, "encryptedDataKey").ok_or(StoreError)?,
        nonce: attr_s(item, "nonce").ok_or(StoreError)?,
    })
}

pub fn demographic_values(demographics: &Demographics) -> Vec<(&'static str, Option<String>)> {
    vec![
        (
            "ageRange",
            demographics.age_range.map(AgeRange::as_str).map(str::to_string),
        ),
        ("locationBucket", demographics.location.clone()),
        ("race", demographics.race.map(Race::as_str).map(str::to_string)),
        (
            "economicStatus",
            demographics
                .economic_status
                .map(EconomicStatus::as_str)
                .map(str::to_string),
        ),
    ]
}

pub fn attr_s(item: &HashMap<String, AttributeValue>, key: &str) -> Option<String> {
    item.get(key).and_then(|value| value.as_s().ok()).cloned()
}

pub fn attr_bool(item: &HashMap<String, AttributeValue>, key: &str) -> bool {
    item.get(key).and_then(|value| value.as_bool().ok()).copied().unwrap_or(false)
}

pub fn attr_n_u64(item: &HashMap<String, AttributeValue>, key: &str) -> u64 {
    item.get(key)
        .and_then(|value| value.as_n().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn attr_ss(item: &HashMap<String, AttributeValue>, key: &str) -> Vec<String> {
    item.get(key)
        .and_then(|value| value.as_ss().ok())
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn match_items_do_not_carry_an_address() {
        let record = MatchRecord {
            match_id: MatchId::new("match-1").unwrap(),
            writer_id: WriterId::new("writer-1").unwrap(),
            writer_pseudonym: "North Window".into(),
            reader_id: ReaderId::new("reader-1").unwrap(),
            reader_pseudonym: "Cedar".into(),
            matched_at: Utc.with_ymd_and_hms(2026, 11, 12, 15, 0, 0).unwrap(),
            sent: false,
            current: true,
        };
        let item = match_item(&record);
        let mut keys: Vec<_> = item.keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "current",
                "matchId",
                "matchedAt",
                "readerId",
                "readerPseudonym",
                "sent",
                "writerId",
                "writerPseudonym",
            ]
        );
        let raw = format!("{item:?}");
        assert!(!raw.to_ascii_lowercase().contains("line1"));
        assert!(!raw.to_ascii_lowercase().contains("ciphertext"));
    }

    #[test]
    fn routes_file_lists_health_and_the_webhook_as_public() {
        let routes: serde_json::Value =
            serde_json::from_str(include_str!("../../../routes.json")).unwrap();
        let public = routes["public"].as_array().unwrap();
        assert!(public.iter().any(|route| route["path"] == "/health" && route["method"] == "GET"));
        assert!(public.iter().any(|route| {
            route["path"] == "/webhooks/stripe" && route["method"] == "POST"
        }));
        let authed = routes["authenticated"].as_array().unwrap();
        assert!(authed.iter().all(|route| route["path"] != "/health"));
        assert!(authed
            .iter()
            .any(|route| route["path"] == "/matches/{matchId}/address"));
    }
}
