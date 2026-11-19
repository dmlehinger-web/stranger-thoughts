use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use std::collections::HashSet;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebhookError {
    #[error("bad secret")]
    BadSecret,
    #[error("missing timestamp")]
    MissingTimestamp,
    #[error("missing signature")]
    MissingSignature,
    #[error("stale")]
    Stale,
    #[error("bad signature")]
    BadSignature,
    #[error("bad payload")]
    BadPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookAction {
    MarkTipPaid { tip_id: String, session_id: String },
    UpdatePayouts { reader_id: String, ready: bool },
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEvent {
    pub id: String,
    pub action: WebhookAction,
}

pub fn verify_stripe_signature(
    secret: &str,
    header: &str,
    body: &[u8],
    now_unix: i64,
    tolerance_secs: i64,
) -> Result<i64, WebhookError> {
    if secret.is_empty() || !secret.starts_with("whsec_") || tolerance_secs < 0 {
        return Err(WebhookError::BadSecret);
    }
    let mut timestamp: Option<i64> = None;
    let mut signatures: Vec<Vec<u8>> = Vec::new();
    if header.trim().is_empty() {
        return Err(WebhookError::MissingSignature);
    }
    for part in header.split(',') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        if key == "t" {
            timestamp = value.parse().ok();
        } else if key == "v1" {
            if let Ok(bytes) = hex::decode(value) {
                if !bytes.is_empty() {
                    signatures.push(bytes);
                }
            }
        }
    }
    let timestamp = timestamp.ok_or(WebhookError::MissingTimestamp)?;
    if signatures.is_empty() {
        return Err(WebhookError::MissingSignature);
    }
    if (now_unix - timestamp).abs() > tolerance_secs {
        return Err(WebhookError::Stale);
    }
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| WebhookError::BadSecret)?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let matched = signatures.iter().any(|sig| mac.clone().verify_slice(sig).is_ok());
    if matched {
        Ok(timestamp)
    } else {
        Err(WebhookError::BadSignature)
    }
}

/// Returns true the first time an event id is seen.
pub fn claim_event(seen: &mut HashSet<String>, event_id: &str) -> bool {
    seen.insert(event_id.to_string())
}

pub fn parse_event(body: &[u8]) -> Result<ParsedEvent, WebhookError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| WebhookError::BadPayload)?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| id.starts_with("evt_") && id.len() > 4 && id.len() <= 80)
        .ok_or(WebhookError::BadPayload)?
        .to_string();
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let object = value
        .get("data")
        .and_then(|data| data.get("object"))
        .cloned()
        .unwrap_or(Value::Null);
    let action = match event_type {
        "checkout.session.completed" => checkout_action(&object),
        "account.updated" => account_action(&object),
        _ => WebhookAction::Ignore,
    };
    Ok(ParsedEvent { id, action })
}

fn checkout_action(object: &Value) -> WebhookAction {
    let tip_id = object
        .pointer("/metadata/tip_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let session_id = object.get("id").and_then(Value::as_str).unwrap_or("");
    if tip_id.is_empty() || !session_id.starts_with("cs_") {
        return WebhookAction::Ignore;
    }
    WebhookAction::MarkTipPaid {
        tip_id: tip_id.to_string(),
        session_id: session_id.to_string(),
    }
}

fn account_action(object: &Value) -> WebhookAction {
    let reader_id = object
        .pointer("/metadata/reader_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let account_id = object.get("id").and_then(Value::as_str).unwrap_or("");
    if reader_id.is_empty() || !account_id.starts_with("acct_") {
        return WebhookAction::Ignore;
    }
    let ready = object.get("charges_enabled").and_then(Value::as_bool).unwrap_or(false)
        && object.get("payouts_enabled").and_then(Value::as_bool).unwrap_or(false);
    WebhookAction::UpdatePayouts {
        reader_id: reader_id.to_string(),
        ready,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(secret: &str, timestamp: i64, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        let bytes = mac.finalize().into_bytes();
        format!("t={timestamp},v1={}", hex::encode(bytes))
    }

    #[test]
    fn signature_rejects_unsigned_stale_and_tampered_bodies() {
        let secret = "whsec_test_secret_value";
        let body = br#"{"id":"evt_1","type":"checkout.session.completed"}"#;
        let header = sign(secret, 1_700_000_000, body);
        assert!(verify_stripe_signature(secret, &header, body, 1_700_000_100, 300).is_ok());
        assert_eq!(
            verify_stripe_signature(secret, "", body, 1_700_000_100, 300),
            Err(WebhookError::MissingSignature)
        );
        assert_eq!(
            verify_stripe_signature(secret, &header, body, 1_700_000_000 + 301, 300),
            Err(WebhookError::Stale)
        );
        assert_eq!(
            verify_stripe_signature(secret, &header, b"{}", 1_700_000_100, 300),
            Err(WebhookError::BadSignature)
        );
    }

    #[test]
    fn rotation_accepts_either_v1_and_replay_is_idempotent() {
        let secret = "whsec_test_secret_value";
        let body = br#"{"id":"evt_1"}"#;
        let good = sign(secret, 50, body);
        let v1 = good.split(',').nth(1).unwrap();
        let header = format!("t=50,v1=abcd,{v1}");
        assert!(verify_stripe_signature(secret, &header, body, 50, 300).is_ok());

        let mut seen = HashSet::new();
        assert!(claim_event(&mut seen, "evt_1"));
        assert!(!claim_event(&mut seen, "evt_1"));
    }

    #[test]
    fn parser_ignores_address_fields_and_keeps_only_the_action() {
        let body = br#"{
            "id":"evt_123",
            "type":"checkout.session.completed",
            "data":{"object":{
                "id":"cs_test_123",
                "customer_details":{"email":"person@example.com","address":{"line1":"PO Box 9"}},
                "metadata":{"tip_id":"tip-1","kind":"reader"}
            }}
        }"#;
        let parsed = parse_event(body).unwrap();
        match &parsed.action {
            WebhookAction::MarkTipPaid { tip_id, session_id } => {
                assert_eq!(tip_id, "tip-1");
                assert_eq!(session_id, "cs_test_123");
            }
            other => panic!("unexpected {other:?}"),
        }
        let rendered = format!("{:?}", parsed.action);
        assert!(!rendered.contains("example.com"));
        assert!(!rendered.contains("PO Box"));
    }
}
