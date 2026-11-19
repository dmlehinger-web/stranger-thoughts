use crate::ids::{MatchId, ReaderId, WriterId};
use crate::model::MatchRecord;
use crate::{platform_fee_cents, MAX_TIP_CENTS, MIN_TIP_CENTS};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipKind {
    Reader,
    Builder,
}

impl TipKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TipKind::Reader => "reader",
            TipKind::Builder => "builder",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedReaderTip {
    pub match_id: MatchId,
    pub writer_id: WriterId,
    pub reader_id: ReaderId,
    pub destination_account: String,
    pub amount_cents: u32,
    pub application_fee_cents: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedBuilderTip {
    pub user_id: String,
    pub amount_cents: u32,
    pub application_fee_cents: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TipError {
    #[error("not found")]
    NotFound,
    #[error("payouts unavailable")]
    PayoutsUnavailable,
    #[error("invalid amount")]
    InvalidAmount,
}

pub fn validate_tip_amount(amount_cents: u32) -> Result<(), TipError> {
    if (MIN_TIP_CENTS..=MAX_TIP_CENTS).contains(&amount_cents) {
        Ok(())
    } else {
        Err(TipError::InvalidAmount)
    }
}

/// Route a thank-you to the reader stored on the match.
///
/// There is no destination argument. The connected account is looked up from
/// `payout_accounts` using the reader id on that match row, so a caller cannot
/// aim the charge at a different person.
pub fn route_reader_tip(
    writer_id: &WriterId,
    match_id: &MatchId,
    amount_cents: u32,
    history: &[MatchRecord],
    payout_accounts: &HashMap<ReaderId, String>,
) -> Result<RoutedReaderTip, TipError> {
    validate_tip_amount(amount_cents)?;
    let record = history
        .iter()
        .find(|record| &record.match_id == match_id)
        .ok_or(TipError::NotFound)?;
    if &record.writer_id != writer_id {
        return Err(TipError::NotFound);
    }
    let account = payout_accounts
        .get(&record.reader_id)
        .filter(|account| account.starts_with("acct_") && account.len() > "acct_".len())
        .ok_or(TipError::PayoutsUnavailable)?;
    Ok(RoutedReaderTip {
        match_id: record.match_id.clone(),
        writer_id: record.writer_id.clone(),
        reader_id: record.reader_id.clone(),
        destination_account: account.clone(),
        amount_cents,
        application_fee_cents: platform_fee_cents(),
    })
}

pub fn route_builder_tip(user_id: &str, amount_cents: u32) -> Result<RoutedBuilderTip, TipError> {
    validate_tip_amount(amount_cents)?;
    if user_id.is_empty() {
        return Err(TipError::NotFound);
    }
    Ok(RoutedBuilderTip {
        user_id: user_id.to_string(),
        amount_cents,
        application_fee_cents: platform_fee_cents(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn record(id: &str, writer: &str, reader: &str, current: bool) -> MatchRecord {
        MatchRecord {
            match_id: MatchId::new(id).unwrap(),
            writer_id: WriterId::new(writer).unwrap(),
            writer_pseudonym: "North Window".into(),
            reader_id: ReaderId::new(reader).unwrap(),
            reader_pseudonym: reader.into(),
            matched_at: Utc.with_ymd_and_hms(2026, 11, 3, 12, 0, 0).unwrap(),
            sent: true,
            current,
        }
    }

    #[test]
    fn tip_follows_the_match_even_when_another_account_is_present() {
        let history = vec![
            record("m-cedar", "writer-1", "reader-cedar", true),
            record("m-harbor", "writer-1", "reader-harbor", false),
            record("m-other", "writer-2", "reader-cedar", true),
        ];
        let mut accounts = HashMap::new();
        accounts.insert(ReaderId::new("reader-cedar").unwrap(), "acct_cedar".into());
        accounts.insert(ReaderId::new("reader-harbor").unwrap(), "acct_harbor".into());
        let writer = WriterId::new("writer-1").unwrap();

        let current = route_reader_tip(
            &writer,
            &MatchId::new("m-cedar").unwrap(),
            500,
            &history,
            &accounts,
        )
        .unwrap();
        assert_eq!(current.destination_account, "acct_cedar");
        assert_eq!(current.reader_id.as_str(), "reader-cedar");
        assert_eq!(current.application_fee_cents, 0);

        let historical = route_reader_tip(
            &writer,
            &MatchId::new("m-harbor").unwrap(),
            300,
            &history,
            &accounts,
        )
        .unwrap();
        assert_eq!(historical.destination_account, "acct_harbor");
        assert!(!history.iter().find(|r| r.match_id.as_str() == "m-harbor").unwrap().current);

        let foreign = route_reader_tip(
            &writer,
            &MatchId::new("m-other").unwrap(),
            500,
            &history,
            &accounts,
        );
        assert_eq!(foreign, Err(TipError::NotFound));
    }

    #[test]
    fn missing_payout_account_does_not_fall_back() {
        let history = vec![record("m-cedar", "writer-1", "reader-cedar", true)];
        let accounts = HashMap::from([(
            ReaderId::new("reader-harbor").unwrap(),
            "acct_harbor".to_string(),
        )]);
        let err = route_reader_tip(
            &WriterId::new("writer-1").unwrap(),
            &MatchId::new("m-cedar").unwrap(),
            500,
            &history,
            &accounts,
        )
        .unwrap_err();
        assert_eq!(err, TipError::PayoutsUnavailable);
    }

    #[test]
    fn builder_tip_has_no_reader_and_rejects_zero() {
        let tip = route_builder_tip("writer-1", 500).unwrap();
        assert_eq!(tip.application_fee_cents, 0);
        assert!(route_builder_tip("writer-1", 0).is_err());
        assert!(route_builder_tip("writer-1", 50).is_err());
        assert!(route_builder_tip("writer-1", 20_001).is_err());
    }
}
