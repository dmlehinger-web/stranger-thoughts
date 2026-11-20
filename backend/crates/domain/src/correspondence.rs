//! Physical-letter turns on a match.
//!
//! A turn records that someone mailed a letter, or that the other person
//! marked it received. It does not store letter text, a name beyond the
//! match the caller already has, or any address.

use crate::authz::Role;
use chrono::{DateTime, Utc};
use serde::Serialize;

/// Ongoing correspondence stops here so one match cannot mint unlimited mail notices.
pub const MAX_TURNS_PER_MATCH: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    WriterToReader,
    ReaderToWriter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub turn_id: String,
    pub sequence: u32,
    pub direction: Direction,
    pub mailed_at: DateTime<Utc>,
    pub received_at: Option<DateTime<Utc>>,
    pub notice_sent_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TurnView {
    pub sequence: u32,
    pub status: String,
    pub mailed_on: String,
    pub received_on: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LetterExchange {
    pub sent: bool,
    pub status: String,
    pub actions: Vec<String>,
    pub turns: Vec<TurnView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkResult {
    /// This request created the mailed turn. The caller may send one notice.
    Created(Turn),
    /// The turn was already mailed. Do not send another notice.
    AlreadyMailed(Turn),
    Received(Turn),
    AlreadyReceived(Turn),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CorrespondenceError {
    #[error("not your turn")]
    NotYourTurn,
    #[error("waiting on the other person")]
    Waiting,
    #[error("too many letters on this match")]
    Full,
}

pub fn sender(direction: Direction) -> Role {
    match direction {
        Direction::WriterToReader => Role::Writer,
        Direction::ReaderToWriter => Role::Reader,
    }
}

pub fn recipient(direction: Direction) -> Role {
    match direction {
        Direction::WriterToReader => Role::Reader,
        Direction::ReaderToWriter => Role::Writer,
    }
}

pub fn exchange_for(role: Role, turns: &[Turn]) -> LetterExchange {
    let sent = turns
        .iter()
        .any(|turn| turn.direction == Direction::WriterToReader);
    let status = match turns.last() {
        Some(turn) => status_line(turn),
        None => "Not mailed yet".to_string(),
    };
    LetterExchange {
        sent,
        status,
        actions: actions_for(role, turns),
        turns: turns.iter().map(turn_view).collect(),
    }
}

pub fn mark_mailed(
    role: Role,
    turns: &[Turn],
    now: DateTime<Utc>,
    turn_id: &str,
) -> Result<MarkResult, CorrespondenceError> {
    if turns.len() as u32 >= MAX_TURNS_PER_MATCH {
        return Err(CorrespondenceError::Full);
    }
    match turns.last() {
        None => {
            if role != Role::Writer {
                return Err(CorrespondenceError::NotYourTurn);
            }
            Ok(MarkResult::Created(new_turn(
                turn_id,
                1,
                Direction::WriterToReader,
                now,
            )))
        }
        Some(last) if last.received_at.is_none() => {
            if sender(last.direction) == role {
                Ok(MarkResult::AlreadyMailed(last.clone()))
            } else {
                Err(CorrespondenceError::Waiting)
            }
        }
        Some(last) => {
            if recipient(last.direction) != role {
                return Err(CorrespondenceError::NotYourTurn);
            }
            let direction = match role {
                Role::Writer => Direction::WriterToReader,
                Role::Reader => Direction::ReaderToWriter,
            };
            Ok(MarkResult::Created(new_turn(
                turn_id,
                last.sequence + 1,
                direction,
                now,
            )))
        }
    }
}

pub fn mark_received(
    role: Role,
    turns: &[Turn],
    now: DateTime<Utc>,
) -> Result<MarkResult, CorrespondenceError> {
    let last = turns.last().ok_or(CorrespondenceError::NotYourTurn)?;
    if recipient(last.direction) != role {
        return Err(CorrespondenceError::NotYourTurn);
    }
    if last.received_at.is_some() {
        return Ok(MarkResult::AlreadyReceived(last.clone()));
    }
    let mut updated = last.clone();
    updated.received_at = Some(now);
    Ok(MarkResult::Received(updated))
}

fn new_turn(turn_id: &str, sequence: u32, direction: Direction, now: DateTime<Utc>) -> Turn {
    Turn {
        turn_id: turn_id.to_string(),
        sequence,
        direction,
        mailed_at: now,
        received_at: None,
        notice_sent_at: None,
    }
}

fn turn_view(turn: &Turn) -> TurnView {
    TurnView {
        sequence: turn.sequence,
        status: status_line(turn),
        mailed_on: day(turn.mailed_at),
        received_on: turn.received_at.map(day),
    }
}

pub fn status_line(turn: &Turn) -> String {
    match turn.received_at {
        Some(received) => format!("Received · marked received {}", day(received)),
        None => format!("In transit · marked mailed {}", day(turn.mailed_at)),
    }
}

fn day(ts: DateTime<Utc>) -> String {
    ts.format("%Y-%m-%d").to_string()
}

fn actions_for(role: Role, turns: &[Turn]) -> Vec<String> {
    if turns.len() as u32 >= MAX_TURNS_PER_MATCH {
        return Vec::new();
    }
    match turns.last() {
        None => {
            if role == Role::Writer {
                vec!["mark_mailed".into()]
            } else {
                Vec::new()
            }
        }
        Some(turn) if turn.received_at.is_none() => {
            if recipient(turn.direction) == role {
                vec!["mark_received".into()]
            } else {
                Vec::new()
            }
        }
        Some(turn) => {
            if recipient(turn.direction) != role {
                return Vec::new();
            }
            if role == Role::Writer {
                vec!["mark_mailed".into()]
            } else {
                vec!["mark_reply_mailed".into()]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::Value;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, 15, 0, 0).unwrap()
    }

    fn mailed(role: Role, turns: &[Turn], n: u8) -> Turn {
        match mark_mailed(role, turns, now(), &format!("turn-{n}")).unwrap() {
            MarkResult::Created(turn) => turn,
            other => panic!("expected a new turn, got {other:?}"),
        }
    }

    #[test]
    fn writer_mails_reader_receives_reader_replies() {
        let first = mailed(Role::Writer, &[], 1);
        assert_eq!(first.sequence, 1);
        assert_eq!(first.direction, Direction::WriterToReader);
        let writer_view = exchange_for(Role::Writer, std::slice::from_ref(&first));
        assert_eq!(writer_view.actions, Vec::<String>::new());
        assert_eq!(writer_view.status, "In transit · marked mailed 2026-11-20");
        let reader_view = exchange_for(Role::Reader, std::slice::from_ref(&first));
        assert_eq!(reader_view.actions, vec!["mark_received".to_string()]);

        let received = match mark_received(Role::Reader, std::slice::from_ref(&first), now()).unwrap() {
            MarkResult::Received(turn) => turn,
            other => panic!("{other:?}"),
        };
        let after = vec![received];
        assert_eq!(
            exchange_for(Role::Reader, &after).actions,
            vec!["mark_reply_mailed".to_string()]
        );
        let reply = mailed(Role::Reader, &after, 2);
        assert_eq!(reply.direction, Direction::ReaderToWriter);
        assert_eq!(reply.sequence, 2);
        let both = vec![after[0].clone(), reply];
        assert_eq!(
            exchange_for(Role::Writer, &both).actions,
            vec!["mark_received".to_string()]
        );
        assert_eq!(
            exchange_for(Role::Writer, &both).status,
            "In transit · marked mailed 2026-11-20"
        );
    }

    #[test]
    fn second_click_does_not_create_another_turn() {
        let first = mailed(Role::Writer, &[], 1);
        let again = mark_mailed(Role::Writer, std::slice::from_ref(&first), now(), "turn-again").unwrap();
        assert!(matches!(again, MarkResult::AlreadyMailed(_)));
        assert!(mark_mailed(Role::Reader, &[first], now(), "nope").is_err());
    }

    #[test]
    fn views_have_no_pii_keys_or_sender_details() {
        let first = mailed(Role::Writer, &[], 1);
        let view = exchange_for(Role::Reader, &[first]);
        let value = serde_json::to_value(&view).unwrap();
        assert!(crate::redaction::find_pii_key(&value).is_none(), "{value}");
        let raw = serde_json::to_string(&value).unwrap();
        for banned in ["@", "line1", "postal", "email", "address", "Cedar", "city"] {
            assert!(!raw.to_ascii_lowercase().contains(&banned.to_ascii_lowercase()), "{banned} in {raw}");
        }
        let keys = pii_walk(&value);
        assert!(keys.is_empty(), "{keys:?}");
    }

    fn pii_walk(value: &Value) -> Vec<String> {
        let mut found = Vec::new();
        fn walk(value: &Value, found: &mut Vec<String>) {
            match value {
                Value::Object(map) => {
                    for (key, child) in map {
                        if crate::redaction::is_sensitive_key(key) {
                            found.push(key.clone());
                        }
                        walk(child, found);
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        walk(item, found);
                    }
                }
                _ => {}
            }
        }
        walk(value, &mut found);
        found
    }
}
