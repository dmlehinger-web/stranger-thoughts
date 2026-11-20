//! One plain-text note when a stranger marks a letter mailed.
//!
//! The message is a static template. The recipient address is read from
//! Cognito after the opt-out check, and it is never logged or returned.

use lambda_http::Error;
use serde::Deserialize;
use serde_json::{json, Value};
use stranger_domain::authz::Role;
use stranger_domain::notice::{self, NoticeKind};
use stranger_domain::redaction::SafeLog;

use crate::store;
use crate::Aws;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoticeJob {
    recipient_sub: String,
    recipient_role: String,
    kind: String,
    turn_id: String,
    match_id: String,
    sequence: u32,
}

pub async fn handle_notify(payload: Value) -> Result<Value, Error> {
    let job: NoticeJob = serde_json::from_value(payload).map_err(|_| Error::from("invalid notice"))?;
    if !safe_token(&job.recipient_sub)
        || !safe_token(&job.turn_id)
        || !safe_token(&job.match_id)
        || job.sequence == 0
        || job.sequence > 30
    {
        return Ok(json!({"skipped": "invalid"}));
    }
    let role = match job.recipient_role.as_str() {
        "writer" => Role::Writer,
        "reader" => Role::Reader,
        _ => return Ok(json!({"skipped": "invalid"})),
    };
    let kind = match job.kind.as_str() {
        "letter" => NoticeKind::LetterOnTheWay,
        "reply" => NoticeKind::ReplyOnTheWay,
        _ => return Ok(json!({"skipped": "invalid"})),
    };
    let aws = Aws::get().await.map_err(|_| Error::from("aws"))?;
    let store = store::Store::from_env(aws.dynamo.clone());
    let alerts_on = preference_allows_send(
        store
            .mail_alerts_enabled(role, &job.recipient_sub)
            .await
            .map_err(|_| ()),
    );
    if !alerts_on {
        tracing::info!(
            "{}",
            serde_json::to_string(&SafeLog::new("notice opted out").field("turn_id", &job.turn_id))
                .unwrap_or_else(|_| "{}".into())
        );
        return Ok(json!({"skipped": "opt_out"}));
    }
    if std::env::var("SES_ENABLED").ok().as_deref() != Some("true") {
        tracing::info!(
            "{}",
            serde_json::to_string(&SafeLog::new("notice skipped").field("turn_id", &job.turn_id))
                .unwrap_or_else(|_| "{}".into())
        );
        return Ok(json!({"skipped": "ses_disabled"}));
    }
    let claimed = store
        .claim_notice_dispatch(&job.match_id, job.sequence, chrono::Utc::now())
        .await
        .unwrap_or(false);
    if !claimed {
        return Ok(json!({"skipped": "already_sent"}));
    }
    let origin = std::env::var("SITE_ORIGIN").unwrap_or_default();
    let from = std::env::var("SES_FROM_ADDRESS").unwrap_or_default();
    let notice = notice::compose(kind, &origin).map_err(|_| Error::from("notice template"))?;
    if !from_address_ok(&from) {
        return Ok(json!({"skipped": "from_address"}));
    }
    let email = match lookup_email(aws, &job.recipient_sub).await {
        Ok(email) => email,
        Err(_) => {
            tracing::info!(
                "{}",
                serde_json::to_string(&SafeLog::new("notice lookup failed").field("turn_id", &job.turn_id))
                    .unwrap_or_else(|_| "{}".into())
            );
            return Ok(json!({"skipped": "lookup"}));
        }
    };
    if send_static(aws, &from, &email, &notice.subject, &notice.text_body)
        .await
        .is_err()
    {
        tracing::info!(
            "{}",
            serde_json::to_string(&SafeLog::new("notice send failed").field("turn_id", &job.turn_id))
                .unwrap_or_else(|_| "{}".into())
        );
        return Ok(json!({"skipped": "send"}));
    }
    let _ = store
        .mark_notice_sent(&job.match_id, job.sequence, chrono::Utc::now())
        .await;
    Ok(json!({"sent": true}))
}

/// A failed preference read skips the email. A missing attribute still means alerts
/// are on; that default is applied inside the store before this result is returned.
fn preference_allows_send(read: Result<bool, ()>) -> bool {
    read.unwrap_or(false)
}

fn safe_token(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && !value.contains('@')
        && !value.contains('\n')
        && !value.contains('\r')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
}

fn from_address_ok(value: &str) -> bool {
    let Some((local, host)) = value.split_once('@') else {
        return false;
    };
    host == "astrangerreads.com"
        && !local.is_empty()
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
        && !value.contains('\n')
        && !value.contains('\r')
}

async fn lookup_email(aws: &Aws, sub: &str) -> Result<String, ()> {
    let pool = std::env::var("USER_POOL_ID").map_err(|_| ())?;
    let listed = aws
        .cognito
        .list_users()
        .user_pool_id(pool)
        .filter(format!("sub = \"{sub}\""))
        .limit(1)
        .send()
        .await
        .map_err(|_| ())?;
    let user = listed.users().first().ok_or(())?;
    let email = user
        .attributes()
        .iter()
        .find(|attribute| attribute.name() == "email")
        .and_then(|attribute| attribute.value())
        .ok_or(())?;
    let verified = user
        .attributes()
        .iter()
        .find(|attribute| attribute.name() == "email_verified")
        .and_then(|attribute| attribute.value())
        .unwrap_or("false");
    if verified != "true" || email.contains('\n') || email.contains('\r') || !email.contains('@') {
        return Err(());
    }
    Ok(email.to_string())
}

async fn send_static(aws: &Aws, from: &str, to: &str, subject: &str, body: &str) -> Result<(), ()> {
    use aws_sdk_ses::types::{Body, Content, Destination, Message};
    let content = |value: &str| Content::builder().data(value).charset("UTF-8").build().map_err(|_| ());
    aws.ses
        .send_email()
        .source(from)
        .destination(Destination::builder().to_addresses(to).build())
        .message(
            Message::builder()
                .subject(content(subject)?)
                .body(Body::builder().text(content(body)?).build())
                .build(),
        )
        .send()
        .await
        .map_err(|_| ())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::preference_allows_send;

    #[test]
    fn opt_out_read_failure_does_not_send() {
        assert!(!preference_allows_send(Err(())));
        assert!(!preference_allows_send(Ok(false)));
        assert!(preference_allows_send(Ok(true)));
    }
}
