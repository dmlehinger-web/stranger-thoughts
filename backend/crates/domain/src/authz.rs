use crate::ids::{MatchId, ReaderId, WriterId};
use crate::model::MatchRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Writer,
    Reader,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub user_id: String,
    pub role: Role,
    pub email_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressGrant {
    pub reader_id: ReaderId,
    pub match_id: MatchId,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthzError {
    #[error("email unverified")]
    EmailUnverified,
    #[error("not found")]
    NotFound,
}

pub fn principal_from_claims(claims: &HashMap<String, String>) -> Result<Principal, AuthError> {
    let sub = claims
        .get("sub")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or(AuthError::Unauthorized)?;
    let token_use = claims.get("token_use").map(String::as_str).unwrap_or("");
    if token_use != "access" && token_use != "id" {
        return Err(AuthError::Unauthorized);
    }
    let groups = parse_groups(claims.get("cognito:groups").map(String::as_str).unwrap_or(""));
    let writer = groups.iter().any(|group| group == "writer");
    let reader = groups.iter().any(|group| group == "reader");
    let role = match (writer, reader) {
        (true, false) => Role::Writer,
        (false, true) => Role::Reader,
        _ => return Err(AuthError::Forbidden),
    };
    let email_verified = claims.get("email_verified").map(String::as_str) == Some("true");
    Ok(Principal {
        user_id: sub.to_string(),
        role,
        email_verified,
    })
}

fn parse_groups(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if let Ok(values) = serde_json::from_str::<Vec<String>>(trimmed) {
        return values;
    }
    trimmed
        .trim_matches(['[', ']'])
        .split([',', ' '])
        .map(|part| part.trim().trim_matches('"').to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

/// Address plaintext is released only to the verified writer on the current match.
/// Everyone else, including the same writer looking at history, gets not-found
/// so a guess does not reveal whether the match id exists.
pub fn authorize_address_read(
    principal: &Principal,
    match_id: &MatchId,
    history: &[MatchRecord],
) -> Result<AddressGrant, AuthzError> {
    if principal.role != Role::Writer {
        return Err(AuthzError::NotFound);
    }
    if !principal.email_verified {
        return Err(AuthzError::EmailUnverified);
    }
    let record = history
        .iter()
        .find(|record| &record.match_id == match_id)
        .ok_or(AuthzError::NotFound)?;
    if record.writer_id.as_str() != principal.user_id || !record.current {
        return Err(AuthzError::NotFound);
    }
    Ok(AddressGrant {
        reader_id: record.reader_id.clone(),
        match_id: record.match_id.clone(),
    })
}

pub fn require_role(principal: &Principal, role: Role) -> Result<(), AuthError> {
    if principal.role == role {
        Ok(())
    } else {
        Err(AuthError::Forbidden)
    }
}

pub fn require_verified(principal: &Principal) -> Result<(), AuthzError> {
    if principal.email_verified {
        Ok(())
    } else {
        Err(AuthzError::EmailUnverified)
    }
}

pub fn writer_id(principal: &Principal) -> Result<WriterId, AuthError> {
    if principal.role != Role::Writer {
        return Err(AuthError::Forbidden);
    }
    WriterId::new(principal.user_id.clone()).map_err(|_| AuthError::Unauthorized)
}

pub fn reader_id(principal: &Principal) -> Result<ReaderId, AuthError> {
    if principal.role != Role::Reader {
        return Err(AuthError::Forbidden);
    }
    ReaderId::new(principal.user_id.clone()).map_err(|_| AuthError::Unauthorized)
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
            reader_pseudonym: "Cedar".into(),
            matched_at: Utc.with_ymd_and_hms(2026, 11, 12, 12, 0, 0).unwrap(),
            sent: false,
            current,
        }
    }

    fn writer(id: &str, verified: bool) -> Principal {
        Principal {
            user_id: id.into(),
            role: Role::Writer,
            email_verified: verified,
        }
    }

    #[test]
    fn address_is_only_for_the_current_matched_writer() {
        let history = vec![
            record("past", "writer-1", "reader-old", false),
            record("live", "writer-1", "reader-live", true),
            record("other", "writer-2", "reader-other", true),
        ];
        let ok = authorize_address_read(
            &writer("writer-1", true),
            &MatchId::new("live").unwrap(),
            &history,
        )
        .unwrap();
        assert_eq!(ok.reader_id.as_str(), "reader-live");

        assert_eq!(
            authorize_address_read(&writer("writer-1", true), &MatchId::new("past").unwrap(), &history),
            Err(AuthzError::NotFound)
        );
        assert_eq!(
            authorize_address_read(&writer("writer-1", true), &MatchId::new("other").unwrap(), &history),
            Err(AuthzError::NotFound)
        );
        assert_eq!(
            authorize_address_read(&writer("writer-2", true), &MatchId::new("live").unwrap(), &history),
            Err(AuthzError::NotFound)
        );
        assert_eq!(
            authorize_address_read(&writer("writer-1", false), &MatchId::new("live").unwrap(), &history),
            Err(AuthzError::EmailUnverified)
        );
        let reader = Principal {
            user_id: "reader-live".into(),
            role: Role::Reader,
            email_verified: true,
        };
        assert_eq!(
            authorize_address_read(&reader, &MatchId::new("live").unwrap(), &history),
            Err(AuthzError::NotFound)
        );
    }

    #[test]
    fn claims_require_one_group_and_a_real_token() {
        let mut claims = HashMap::from([
            ("sub".into(), "user-1".into()),
            ("token_use".into(), "access".into()),
            ("cognito:groups".into(), "[\"writer\"]".into()),
            ("email_verified".into(), "true".into()),
            ("email".into(), "person@example.com".into()),
        ]);
        let principal = principal_from_claims(&claims).unwrap();
        assert_eq!(principal.role, Role::Writer);
        assert!(principal.email_verified);
        assert_eq!(principal.user_id, "user-1");

        claims.insert("cognito:groups".into(), "[\"writer\",\"reader\"]".into());
        assert_eq!(principal_from_claims(&claims), Err(AuthError::Forbidden));

        claims.insert("cognito:groups".into(), "reader".into());
        claims.insert("email_verified".into(), "false".into());
        let reader = principal_from_claims(&claims).unwrap();
        assert_eq!(reader.role, Role::Reader);
        assert!(!reader.email_verified);

        claims.insert("token_use".into(), "refresh".into());
        assert!(principal_from_claims(&claims).is_err());
    }
}
