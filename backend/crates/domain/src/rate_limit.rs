use crate::ids::WriterId;
use crate::model::MatchRecord;
use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RateLimitError {
    #[error("rate limited")]
    Limited,
}

pub fn rotates_in_window(history: &[MatchRecord], writer: &WriterId, now: DateTime<Utc>) -> u32 {
    history
        .iter()
        .filter(|record| {
            &record.writer_id == writer
                && record.matched_at <= now
                && now.signed_duration_since(record.matched_at) < Duration::hours(24)
        })
        .count() as u32
}

/// Daily counter stored beside the user, not in the match log.
/// Returns the new window start and the count after this attempt.
pub fn bump_daily_counter(
    existing: Option<(DateTime<Utc>, u32)>,
    now: DateTime<Utc>,
    max: u32,
) -> Result<(DateTime<Utc>, u32), RateLimitError> {
    if max == 0 {
        return Err(RateLimitError::Limited);
    }
    match existing {
        Some((start, count))
            if now >= start && now.signed_duration_since(start) < Duration::hours(24) =>
        {
            if count >= max {
                Err(RateLimitError::Limited)
            } else {
                Ok((start, count + 1))
            }
        }
        _ => Ok((now, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{MatchId, ReaderId};
    use chrono::TimeZone;

    fn record(hours_ago: i64) -> MatchRecord {
        let now = Utc.with_ymd_and_hms(2026, 11, 19, 12, 0, 0).unwrap();
        MatchRecord {
            match_id: MatchId::new(format!("m-{hours_ago}")).unwrap(),
            writer_id: WriterId::new("writer-1").unwrap(),
            writer_pseudonym: "North Window".into(),
            reader_id: ReaderId::new("reader-1").unwrap(),
            reader_pseudonym: "Cedar".into(),
            matched_at: now - Duration::hours(hours_ago),
            sent: false,
            current: false,
        }
    }

    #[test]
    fn window_ignores_older_matches() {
        let now = Utc.with_ymd_and_hms(2026, 11, 19, 12, 0, 0).unwrap();
        let history = vec![record(1), record(23), record(25)];
        let writer = WriterId::new("writer-1").unwrap();
        assert_eq!(rotates_in_window(&history, &writer, now), 2);
    }

    #[test]
    fn counter_resets_after_a_day_and_caps() {
        let start = Utc.with_ymd_and_hms(2026, 11, 19, 12, 0, 0).unwrap();
        let next = bump_daily_counter(Some((start, 2)), start + Duration::hours(2), 2);
        assert!(next.is_err());
        let reset = bump_daily_counter(Some((start, 2)), start + Duration::hours(24), 2).unwrap();
        assert_eq!(reset.1, 1);
    }
}
