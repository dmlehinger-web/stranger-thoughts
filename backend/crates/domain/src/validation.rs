use crate::address::PostalAddress;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgeRange {
    R18_24,
    R25_34,
    R35_44,
    R45_54,
    R55_64,
    R65Plus,
}

impl AgeRange {
    pub fn as_str(self) -> &'static str {
        match self {
            AgeRange::R18_24 => "18-24",
            AgeRange::R25_34 => "25-34",
            AgeRange::R35_44 => "35-44",
            AgeRange::R45_54 => "45-54",
            AgeRange::R55_64 => "55-64",
            AgeRange::R65Plus => "65+",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Race {
    PreferNotToSay,
    Asian,
    Black,
    HispanicOrLatino,
    MiddleEasternOrNorthAfrican,
    NativeAmericanOrAlaskaNative,
    PacificIslander,
    White,
    TwoOrMore,
    Another,
}

impl Race {
    pub fn as_str(self) -> &'static str {
        match self {
            Race::PreferNotToSay => "prefer_not_to_say",
            Race::Asian => "asian",
            Race::Black => "black",
            Race::HispanicOrLatino => "hispanic_or_latino",
            Race::MiddleEasternOrNorthAfrican => "middle_eastern_or_north_african",
            Race::NativeAmericanOrAlaskaNative => "native_american_or_alaska_native",
            Race::PacificIslander => "pacific_islander",
            Race::White => "white",
            Race::TwoOrMore => "two_or_more",
            Race::Another => "another",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EconomicStatus {
    PreferNotToSay,
    Lower,
    Middle,
    Higher,
}

impl EconomicStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            EconomicStatus::PreferNotToSay => "prefer_not_to_say",
            EconomicStatus::Lower => "lower",
            EconomicStatus::Middle => "middle",
            EconomicStatus::Higher => "higher",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Demographics {
    pub age_range: Option<AgeRange>,
    pub location: Option<String>,
    pub race: Option<Race>,
    pub economic_status: Option<EconomicStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("invalid {field}")]
    Invalid { field: &'static str },
    #[error("child age rejected")]
    ChildAgeRejected,
    #[error("body too large")]
    BodyTooLarge,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriterProfileRequest {
    pub pseudonym: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemographicsRequest {
    #[serde(default)]
    pub age_range: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub race: Option<String>,
    #[serde(default)]
    pub economic_status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReaderProfileRequest {
    pub pseudonym: String,
    pub address: PostalAddress,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReaderTipRequest {
    pub match_id: String,
    pub amount_cents: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderTipRequest {
    pub amount_cents: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentRequest {
    pub sent: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRequest {
    pub match_id: String,
    pub reason: String,
}

pub fn validate_pseudonym(value: &str) -> Result<String, ValidationError> {
    let trimmed = value.trim();
    let len = trimmed.chars().count();
    if !(2..=40).contains(&len) {
        return Err(ValidationError::Invalid { field: "pseudonym" });
    }
    let lowered = trimmed.to_ascii_lowercase();
    if trimmed.contains('@')
        || lowered.contains("http")
        || trimmed.chars().any(|c| c.is_control())
        || trimmed.contains("  ")
    {
        return Err(ValidationError::Invalid { field: "pseudonym" });
    }
    let ok = trimmed
        .chars()
        .all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '\'' || c == '’');
    if ok {
        Ok(trimmed.to_string())
    } else {
        Err(ValidationError::Invalid { field: "pseudonym" })
    }
}

/// Age is optional. Child buckets are rejected, including under-13, so the
/// product does not collect data about children.
pub fn parse_age_range(value: &str) -> Result<Option<AgeRange>, ValidationError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || eq_ignore(trimmed, "prefer_not_to_say") {
        return Ok(None);
    }
    let lower = trimmed.to_ascii_lowercase();
    const REJECTED: &[&str] = &[
        "under-13", "under 13", "0-12", "0–12", "5-12", "13-17", "13–17", "child", "children",
        "under-18", "under 18",
    ];
    if REJECTED.contains(&lower.as_str()) || lower.contains("under 13") || lower.contains("under-13")
    {
        return Err(ValidationError::ChildAgeRejected);
    }
    let parsed = match lower.as_str() {
        "18-24" | "18–24" => AgeRange::R18_24,
        "25-34" | "25–34" => AgeRange::R25_34,
        "35-44" | "35–44" => AgeRange::R35_44,
        "45-54" | "45–54" => AgeRange::R45_54,
        "55-64" | "55–64" => AgeRange::R55_64,
        "65+" | "65-plus" => AgeRange::R65Plus,
        _ => return Err(ValidationError::Invalid { field: "age_range" }),
    };
    Ok(Some(parsed))
}

pub fn parse_location(value: &str) -> Result<Option<String>, ValidationError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || eq_ignore(trimmed, "prefer_not_to_say") {
        return Ok(None);
    }
    const ALLOWED: &[&str] = &[
        "United States — Northeast",
        "United States — Midwest",
        "United States — South",
        "United States — West",
        "United States — Pacific",
        "Canada",
        "United Kingdom",
        "Europe",
        "Latin America",
        "Africa",
        "Middle East",
        "Asia",
        "Oceania",
    ];
    if ALLOWED.contains(&trimmed) {
        Ok(Some(trimmed.to_string()))
    } else {
        Err(ValidationError::Invalid { field: "location" })
    }
}

pub fn parse_race(value: &str) -> Result<Option<Race>, ValidationError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed = match trimmed {
        "prefer_not_to_say" => Race::PreferNotToSay,
        "asian" => Race::Asian,
        "black" => Race::Black,
        "hispanic_or_latino" => Race::HispanicOrLatino,
        "middle_eastern_or_north_african" => Race::MiddleEasternOrNorthAfrican,
        "native_american_or_alaska_native" => Race::NativeAmericanOrAlaskaNative,
        "pacific_islander" => Race::PacificIslander,
        "white" => Race::White,
        "two_or_more" => Race::TwoOrMore,
        "another" => Race::Another,
        _ => return Err(ValidationError::Invalid { field: "race" }),
    };
    Ok(Some(parsed))
}

pub fn parse_economic_status(value: &str) -> Result<Option<EconomicStatus>, ValidationError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed = match trimmed {
        "prefer_not_to_say" => EconomicStatus::PreferNotToSay,
        "lower" => EconomicStatus::Lower,
        "middle" => EconomicStatus::Middle,
        "higher" => EconomicStatus::Higher,
        _ => return Err(ValidationError::Invalid { field: "economic_status" }),
    };
    Ok(Some(parsed))
}

pub fn parse_demographics(request: &DemographicsRequest) -> Result<Demographics, ValidationError> {
    Ok(Demographics {
        age_range: request
            .age_range
            .as_deref()
            .map(parse_age_range)
            .transpose()?
            .flatten(),
        location: request
            .location
            .as_deref()
            .map(parse_location)
            .transpose()?
            .flatten(),
        race: request.race.as_deref().map(parse_race).transpose()?.flatten(),
        economic_status: request
            .economic_status
            .as_deref()
            .map(parse_economic_status)
            .transpose()?
            .flatten(),
    })
}

pub fn validate_address(address: &PostalAddress) -> Result<(), ValidationError> {
    check_line(&address.line1, 1, 80, "line1")?;
    if let Some(line2) = &address.line2 {
        if !line2.is_empty() {
            check_line(line2, 1, 80, "line2")?;
        }
    }
    check_line(&address.city, 1, 60, "city")?;
    check_line(&address.region, 1, 40, "region")?;
    check_postal(&address.postal_code)?;
    if address.country.len() != 2 || !address.country.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(ValidationError::Invalid { field: "country" });
    }
    if looks_like_card_number(&address.line1) || looks_like_card_number(&address.line2.clone().unwrap_or_default())
    {
        return Err(ValidationError::Invalid { field: "line1" });
    }
    Ok(())
}

fn check_line(value: &str, min: usize, max: usize, field: &'static str) -> Result<(), ValidationError> {
    let trimmed = value.trim();
    let len = trimmed.chars().count();
    if trimmed != value || !(min..=max).contains(&len) || value.chars().any(|c| c.is_control()) {
        return Err(ValidationError::Invalid { field });
    }
    Ok(())
}

fn check_postal(value: &str) -> Result<(), ValidationError> {
    let len = value.chars().count();
    let ok = (2..=16).contains(&len)
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(ValidationError::Invalid { field: "postal_code" })
    }
}

fn looks_like_card_number(value: &str) -> bool {
    let digits = value.chars().filter(|c| c.is_ascii_digit()).count();
    (13..=19).contains(&digits)
}

fn eq_ignore(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_buckets_are_rejected_and_adult_buckets_parse() {
        assert_eq!(parse_age_range("under-13"), Err(ValidationError::ChildAgeRejected));
        assert_eq!(parse_age_range("0-12"), Err(ValidationError::ChildAgeRejected));
        assert_eq!(parse_age_range("13-17"), Err(ValidationError::ChildAgeRejected));
        assert_eq!(parse_age_range("child"), Err(ValidationError::ChildAgeRejected));
        assert_eq!(parse_age_range("35-44").unwrap(), Some(AgeRange::R35_44));
        assert_eq!(parse_age_range("prefer_not_to_say").unwrap(), None);
        assert!(parse_age_range("100-110").is_err());
    }

    #[test]
    fn location_rejects_a_street() {
        assert!(parse_location("United States — Midwest").is_ok());
        assert!(parse_location("12 King Street").is_err());
        assert!(parse_location("Springfield").is_err());
    }

    #[test]
    fn tip_request_rejects_a_destination_override() {
        let ok: ReaderTipRequest =
            serde_json::from_str(r#"{"match_id":"m-1","amount_cents":500}"#).unwrap();
        assert_eq!(ok.amount_cents, 500);
        let rejected = serde_json::from_str::<ReaderTipRequest>(
            r#"{"match_id":"m-1","amount_cents":500,"reader_id":"reader-2","stripe_account":"acct_other"}"#,
        );
        assert!(rejected.is_err());
    }

    #[test]
    fn address_accepts_a_po_box_and_rejects_a_card_number() {
        let ok = PostalAddress {
            line1: "PO Box 000".into(),
            line2: None,
            city: "Example City".into(),
            region: "ST".into(),
            postal_code: "00000".into(),
            country: "US".into(),
        };
        assert!(validate_address(&ok).is_ok());
        let mut card = ok.clone();
        card.line1 = "4111111111111111".into();
        assert!(validate_address(&card).is_err());
    }
}
