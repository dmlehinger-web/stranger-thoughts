use crate::authz::{AuthzError, Principal, Role};
use crate::ids::WriterId;
use crate::model::MatchRecord;

pub const MAX_BLOCKS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportReason {
    Unwanted,
    Harassment,
    Spam,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReportError {
    #[error("not found")]
    NotFound,
    #[error("block list full")]
    Limit,
    #[error("invalid reason")]
    InvalidReason,
}

pub fn parse_reason(value: &str) -> Result<ReportReason, ReportError> {
    match value {
        "unwanted" => Ok(ReportReason::Unwanted),
        "harassment" => Ok(ReportReason::Harassment),
        "spam" => Ok(ReportReason::Spam),
        "other" => Ok(ReportReason::Other),
        _ => Err(ReportError::InvalidReason),
    }
}

/// A reader may block the writer on a match they actually hold.
/// Free-text notes are not accepted; the reason is a fixed code.
pub fn authorize_report(principal: &Principal, record: &MatchRecord) -> Result<WriterId, AuthzError> {
    if principal.role != Role::Reader || !principal.email_verified {
        return Err(AuthzError::NotFound);
    }
    if record.reader_id.as_str() != principal.user_id {
        return Err(AuthzError::NotFound);
    }
    Ok(record.writer_id.clone())
}

pub fn block_writer(blocked: &mut Vec<WriterId>, writer: WriterId) -> Result<(), ReportError> {
    if blocked.iter().any(|existing| existing == &writer) {
        return Ok(());
    }
    if blocked.len() >= MAX_BLOCKS {
        return Err(ReportError::Limit);
    }
    blocked.push(writer);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{MatchId, ReaderId};
    use chrono::{TimeZone, Utc};

    #[test]
    fn only_the_matched_reader_can_block() {
        let record = MatchRecord {
            match_id: MatchId::new("m-1").unwrap(),
            writer_id: WriterId::new("writer-1").unwrap(),
            writer_pseudonym: "North Window".into(),
            reader_id: ReaderId::new("reader-1").unwrap(),
            reader_pseudonym: "Cedar".into(),
            matched_at: Utc.with_ymd_and_hms(2026, 11, 8, 12, 0, 0).unwrap(),
            sent: false,
            current: true,
        };
        let reader = Principal {
            user_id: "reader-1".into(),
            role: Role::Reader,
            email_verified: true,
        };
        assert_eq!(
            authorize_report(&reader, &record).unwrap().as_str(),
            "writer-1"
        );
        let stranger = Principal {
            user_id: "reader-2".into(),
            role: Role::Reader,
            email_verified: true,
        };
        assert_eq!(authorize_report(&stranger, &record), Err(AuthzError::NotFound));
        assert!(parse_reason("please call me").is_err());
        assert_eq!(parse_reason("spam").unwrap(), ReportReason::Spam);
    }
}
