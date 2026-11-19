use crate::ids::{MatchId, WriterId};
use crate::matching::{self, MatchCandidate, MatchError};
use crate::model::MatchRecord;
use crate::rate_limit;
use chrono::{DateTime, Utc};
use rand::{CryptoRng, Rng};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotateOutcome {
    pub history: Vec<MatchRecord>,
    pub created: MatchRecord,
}

pub struct RotateInput<'a> {
    pub writer_id: &'a WriterId,
    pub writer_pseudonym: &'a str,
    pub now: DateTime<Utc>,
    pub history: &'a [MatchRecord],
    pub candidates: &'a [MatchCandidate],
    pub max_per_day: u32,
    pub new_match_id: MatchId,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RotateError {
    #[error("rate limited")]
    RateLimited,
    #[error("no eligible reader")]
    NoEligibleReaders,
    #[error("history belongs to another writer")]
    CorruptHistory,
    #[error("not found")]
    NotFound,
}

impl From<MatchError> for RotateError {
    fn from(value: MatchError) -> Self {
        match value {
            MatchError::NoEligibleReaders => RotateError::NoEligibleReaders,
        }
    }
}

/// Create a new current match and keep every previous row.
///
/// The new row copies the reader's pseudonym only. Address bytes are not a
/// parameter of this function, so they cannot be written into the log.
pub fn rotate<R: CryptoRng + Rng>(
    rng: &mut R,
    input: RotateInput<'_>,
) -> Result<RotateOutcome, RotateError> {
    if input
        .history
        .iter()
        .any(|record| record.writer_id != *input.writer_id)
    {
        return Err(RotateError::CorruptHistory);
    }
    let used = rate_limit::rotates_in_window(input.history, input.writer_id, input.now);
    if input.max_per_day == 0 || used >= input.max_per_day {
        return Err(RotateError::RateLimited);
    }
    let current_reader = input
        .history
        .iter()
        .find(|record| record.current)
        .map(|record| record.reader_id.clone());
    let chosen_id =
        matching::choose_reader(rng, input.writer_id, input.candidates, current_reader.as_ref())?;
    let candidate = input
        .candidates
        .iter()
        .find(|candidate| candidate.reader_id == chosen_id)
        .ok_or(RotateError::NoEligibleReaders)?;
    let mut updated = input.history.to_vec();
    for record in &mut updated {
        record.current = false;
    }
    let created = MatchRecord {
        match_id: input.new_match_id,
        writer_id: input.writer_id.clone(),
        writer_pseudonym: input.writer_pseudonym.to_string(),
        reader_id: candidate.reader_id.clone(),
        reader_pseudonym: candidate.pseudonym.clone(),
        matched_at: input.now,
        sent: false,
        current: true,
    };
    updated.push(created.clone());
    Ok(RotateOutcome {
        history: updated,
        created,
    })
}

pub fn mark_sent(
    history: &mut [MatchRecord],
    writer_id: &WriterId,
    match_id: &MatchId,
    sent: bool,
) -> Result<(), RotateError> {
    let record = history
        .iter_mut()
        .find(|record| &record.match_id == match_id)
        .ok_or(RotateError::NotFound)?;
    if &record.writer_id != writer_id {
        return Err(RotateError::NotFound);
    }
    record.sent = sent;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ReaderId;
    use chrono::TimeZone;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    fn candidate(n: u8) -> MatchCandidate {
        MatchCandidate {
            reader_id: ReaderId::new(format!("reader-{n}")).unwrap(),
            pseudonym: format!("Reader {n}"),
            active: true,
            blocked_writer_ids: Vec::new(),
        }
    }

    fn existing(id: &str, reader: &str, current: bool, hours_ago: i64) -> MatchRecord {
        let now = Utc.with_ymd_and_hms(2026, 11, 19, 18, 0, 0).unwrap();
        MatchRecord {
            match_id: MatchId::new(id).unwrap(),
            writer_id: WriterId::new("writer-1").unwrap(),
            writer_pseudonym: "North Window".into(),
            reader_id: ReaderId::new(reader).unwrap(),
            reader_pseudonym: reader.into(),
            matched_at: now - chrono::Duration::hours(hours_ago),
            sent: true,
            current,
        }
    }

    #[test]
    fn rotate_keeps_history_and_moves_current() {
        let writer = WriterId::new("writer-1").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 11, 19, 18, 0, 0).unwrap();
        let history = vec![
            existing("m-old", "reader-1", false, 48),
            existing("m-now", "reader-2", true, 2),
        ];
        let pool = vec![candidate(1), candidate(2), candidate(3)];
        let mut rng = ChaCha20Rng::seed_from_u64(3);
        let outcome = rotate(
            &mut rng,
            RotateInput {
                writer_id: &writer,
                writer_pseudonym: "North Window",
                now,
                history: &history,
                candidates: &pool,
                max_per_day: 3,
                new_match_id: MatchId::new("m-new").unwrap(),
            },
        )
        .unwrap();
        assert_eq!(outcome.history.len(), 3);
        assert_eq!(
            outcome.history.iter().filter(|record| record.current).count(),
            1
        );
        assert!(outcome.history.iter().any(|record| record.match_id.as_str() == "m-old"));
        assert!(!outcome.history.iter().any(|record| {
            record.match_id.as_str() == "m-now" && record.current
        }));
        assert_eq!(outcome.created.reader_id.as_str(), "reader-3");
        assert!(!outcome.created.sent);
        let raw = serde_json::to_string(&outcome.created).unwrap();
        assert!(!raw.contains("line1"));
        assert!(!raw.contains("address"));
    }

    #[test]
    fn rotate_rate_limit_counts_the_last_day_only() {
        let writer = WriterId::new("writer-1").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 11, 19, 18, 0, 0).unwrap();
        let history = vec![
            existing("a", "reader-1", false, 30),
            existing("b", "reader-2", false, 5),
            existing("c", "reader-3", true, 1),
        ];
        let pool = vec![candidate(4)];
        let mut rng = ChaCha20Rng::seed_from_u64(1);
        let err = rotate(
            &mut rng,
            RotateInput {
                writer_id: &writer,
                writer_pseudonym: "North Window",
                now,
                history: &history,
                candidates: &pool,
                max_per_day: 2,
                new_match_id: MatchId::new("d").unwrap(),
            },
        )
        .unwrap_err();
        assert_eq!(err, RotateError::RateLimited);
    }

    #[test]
    fn sent_flag_does_not_open_another_writers_row() {
        let writer = WriterId::new("writer-1").unwrap();
        let other = WriterId::new("writer-2").unwrap();
        let mut history = vec![existing("m-now", "reader-2", true, 1)];
        mark_sent(&mut history, &writer, &MatchId::new("m-now").unwrap(), false).unwrap();
        assert!(!history[0].sent);
        let err = mark_sent(&mut history, &other, &MatchId::new("m-now").unwrap(), true).unwrap_err();
        assert_eq!(err, RotateError::NotFound);
        assert!(!history[0].sent);
    }
}
