use std::collections::HashMap;

use aws_sdk_dynamodb::operation::put_item::PutItemError;
use aws_sdk_dynamodb::operation::update_item::UpdateItemError;
use aws_sdk_dynamodb::types::{AttributeValue, Put, TransactWriteItem, Update};
use aws_sdk_dynamodb::Client;
use chrono::{DateTime, Utc};
use stranger_domain::address::SealedAddress;
use stranger_domain::ids::{MatchId, ReaderId, WriterId};
use stranger_domain::authz::Role;
use stranger_domain::correspondence::Turn;
use stranger_domain::matching::MatchCandidate;
use stranger_domain::model::MatchRecord;
use stranger_domain::preservation::ArchiveKind;
use stranger_domain::rate_limit::{self, RateLimitError};
use stranger_domain::reports::{self, ReportError};
use stranger_domain::validation::Demographics;

use crate::http::ApiFailure;
use crate::items::{self, attr_bool, attr_n_u64, attr_s, s};

const TIP_PAID_CONDITION: &str = "attribute_exists(tipId) AND checkoutSessionId = :session";

/// A conditional check is a permanent mismatch. Other update failures can be retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipUpdateError {
    Conflict,
    Transient,
}

pub(crate) fn classify_tip_update(conditional_check_failed: bool) -> TipUpdateError {
    if conditional_check_failed {
        TipUpdateError::Conflict
    } else {
        TipUpdateError::Transient
    }
}

/// True when a stored checkout session is the one the paid event names.
#[cfg(test)]
pub(crate) fn tip_paid_condition_matches(stored_session: &str, event_session: &str) -> bool {
    !stored_session.is_empty() && stored_session == event_session
}

#[derive(Debug)]
pub struct StoreError;

pub struct ReaderPayoutRow {
    pub pseudonym: String,
    pub stripe_account_id: Option<String>,
    pub payouts_ready: bool,
}

pub struct Store {
    client: Client,
    users: String,
    readers: String,
    matches: String,
    tips: String,
    correspondence: String,
    preservation: String,
    rates: String,
}

impl Store {
    pub fn from_env(client: Client) -> Self {
        Self {
            client,
            users: std::env::var("USERS_TABLE").unwrap_or_default(),
            readers: std::env::var("READERS_TABLE").unwrap_or_default(),
            matches: std::env::var("MATCHES_TABLE").unwrap_or_default(),
            tips: std::env::var("TIPS_TABLE").unwrap_or_default(),
            correspondence: std::env::var("CORRESPONDENCE_TABLE").unwrap_or_default(),
            preservation: std::env::var("PRESERVATION_TABLE").unwrap_or_default(),
            rates: std::env::var("RATE_TABLE").unwrap_or_default(),
        }
    }

    pub async fn writer_pseudonym(&self, writer_id: &str) -> Result<String, StoreError> {
        let item = self.get_item(&self.users, "userId", writer_id, "sk", "PROFILE", None).await?;
        attr_s(&item.ok_or(StoreError)?, "pseudonym").ok_or(StoreError)
    }

    pub async fn put_writer_profile(&self, writer_id: &str, pseudonym: &str) -> Result<(), StoreError> {
        self.client
            .update_item()
            .table_name(&self.users)
            .key("userId", s(writer_id))
            .key("sk", s("PROFILE"))
            .update_expression(
                "SET pseudonym = :name, #role = if_not_exists(#role, :role), createdAt = if_not_exists(createdAt, :now)",
            )
            .expression_attribute_names("#role", "role")
            .expression_attribute_values(":name", s(pseudonym))
            .expression_attribute_values(":role", s("writer"))
            .expression_attribute_values(":now", s(Utc::now().to_rfc3339()))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn put_demographics(&self, writer_id: &str, demographics: &Demographics) -> Result<(), StoreError> {
        let mut request = self
            .client
            .update_item()
            .table_name(&self.users)
            .key("userId", s(writer_id))
            .key("sk", s("PROFILE"));
        let mut sets = Vec::new();
        let mut names = Vec::new();
        for (name, value) in items::demographic_values(demographics) {
            let token = format!("#{name}");
            let placeholder = format!(":{name}");
            if let Some(value) = value {
                sets.push(format!("{token} = {placeholder}"));
                request = request
                    .expression_attribute_names(&token, name)
                    .expression_attribute_values(placeholder, s(value));
            } else {
                names.push(token);
                request = request.expression_attribute_names(format!("#{name}"), name);
            }
        }
        let mut expression = String::new();
        if !sets.is_empty() {
            expression.push_str("SET ");
            expression.push_str(&sets.join(", "));
        }
        if !names.is_empty() {
            if !expression.is_empty() {
                expression.push(' ');
            }
            expression.push_str("REMOVE ");
            expression.push_str(&names.join(", "));
        }
        if expression.is_empty() {
            return Ok(());
        }
        request
            .update_expression(expression)
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn delete_demographics(&self, user_id: &str) -> Result<(), StoreError> {
        self.client
            .update_item()
            .table_name(&self.users)
            .key("userId", s(user_id))
            .key("sk", s("PROFILE"))
            .update_expression("REMOVE ageRange, locationBucket, race, economicStatus")
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn put_reader_profile(&self, reader_id: &str, pseudonym: &str, active: bool) -> Result<(), StoreError> {
        let active_value = if active { "ACTIVE" } else { "INACTIVE" };
        self.client
            .update_item()
            .table_name(&self.readers)
            .key("readerId", s(reader_id))
            .key("sk", s("PROFILE"))
            .update_expression(
                "SET pseudonym = :name, activePk = :active, payoutsReady = if_not_exists(payoutsReady, :ready), createdAt = if_not_exists(createdAt, :now)",
            )
            .expression_attribute_values(":name", s(pseudonym))
            .expression_attribute_values(":active", s(active_value))
            .expression_attribute_values(":ready", AttributeValue::Bool(false))
            .expression_attribute_values(":now", s(Utc::now().to_rfc3339()))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn put_address(&self, reader_id: &str, sealed: &SealedAddress) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.readers)
            .item("readerId", s(reader_id))
            .item("sk", s("ADDRESS"))
            .item("ciphertext", s(&sealed.ciphertext))
            .item("encryptedDataKey", s(&sealed.encrypted_data_key))
            .item("nonce", s(&sealed.nonce))
            .item("updatedAt", s(Utc::now().to_rfc3339()))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn sealed_address(&self, reader_id: &str) -> Result<Option<SealedAddress>, StoreError> {
        let projection = "readerId, sk, ciphertext, encryptedDataKey, nonce, updatedAt";
        let item = self
            .get_item(&self.readers, "readerId", reader_id, "sk", "ADDRESS", Some(projection))
            .await?;
        item.map(|item| items::sealed_from_item(reader_id, &item)).transpose()
    }

    pub async fn matches_for_writer(&self, writer: &WriterId) -> Result<Vec<MatchRecord>, StoreError> {
        self.query_matches("writerId", writer.as_str(), None).await
    }

    pub async fn matches_for_reader(&self, reader: &ReaderId) -> Result<Vec<MatchRecord>, StoreError> {
        self.query_matches("readerId", reader.as_str(), Some("ByReader")).await
    }

    pub async fn active_candidates(&self) -> Result<Vec<MatchCandidate>, StoreError> {
        let mut items = Vec::new();
        let mut start: Option<HashMap<String, AttributeValue>> = None;
        for _ in 0..20 {
            let mut request = self
                .client
                .query()
                .table_name(&self.readers)
                .index_name("ActiveReaders")
                .key_condition_expression("activePk = :active")
                .expression_attribute_values(":active", s("ACTIVE"))
                .projection_expression("readerId, pseudonym, blockedWriterIds, activePk");
            if let Some(key) = start.clone() {
                request = request.set_exclusive_start_key(Some(key));
            }
            let page = request.send().await.map_err(|_| StoreError)?;
            for item in page.items() {
                items.push(items::candidate_from_item(item)?);
            }
            match page.last_evaluated_key() {
                Some(key) if !key.is_empty() => start = Some(key.clone()),
                _ => return Ok(items),
            }
        }
        Err(StoreError)
    }

    pub async fn save_rotation(
        &self,
        previous: Option<&MatchRecord>,
        created: &MatchRecord,
    ) -> Result<(), StoreError> {
        let put = Put::builder()
            .table_name(&self.matches)
            .set_item(Some(items::match_item(created)))
            .condition_expression("attribute_not_exists(writerId)")
            .build()
            .map_err(|_| StoreError)?;
        let mut write = self.client.transact_write_items().transact_items(
            TransactWriteItem::builder().put(put).build(),
        );
        if let Some(previous) = previous {
            let update = Update::builder()
                .table_name(&self.matches)
                .key("writerId", s(previous.writer_id.as_str()))
                .key("matchedAt", s(previous.matched_at.to_rfc3339()))
                .update_expression("SET current = :current")
                .expression_attribute_values(":current", AttributeValue::Bool(false))
                .build()
                .map_err(|_| StoreError)?;
            write = write.transact_items(TransactWriteItem::builder().update(update).build());
        }
        write.send().await.map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn set_sent(&self, writer: &WriterId, match_id: &MatchId, sent: bool) -> Result<(), StoreError> {
        let record = self
            .matches_for_writer(writer)
            .await?
            .into_iter()
            .find(|record| &record.match_id == match_id)
            .ok_or(StoreError)?;
        self.client
            .update_item()
            .table_name(&self.matches)
            .key("writerId", s(writer.as_str()))
            .key("matchedAt", s(record.matched_at.to_rfc3339()))
            .update_expression("SET sent = :sent")
            .expression_attribute_values(":sent", AttributeValue::Bool(sent))
            .condition_expression("matchId = :matchId")
            .expression_attribute_values(":matchId", s(match_id.as_str()))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn reader_payout(&self, reader_id: &str) -> Result<Option<ReaderPayoutRow>, StoreError> {
        let projection = "readerId, sk, pseudonym, stripeAccountId, payoutsReady, activePk";
        let item = self
            .get_item(&self.readers, "readerId", reader_id, "sk", "PROFILE", Some(projection))
            .await?;
        Ok(item.map(|item| ReaderPayoutRow {
            pseudonym: attr_s(&item, "pseudonym").unwrap_or_default(),
            stripe_account_id: attr_s(&item, "stripeAccountId"),
            payouts_ready: attr_bool(&item, "payoutsReady"),
        }))
    }

    pub async fn set_stripe_account(&self, reader_id: &str, account_id: &str) -> Result<(), StoreError> {
        self.client
            .update_item()
            .table_name(&self.readers)
            .key("readerId", s(reader_id))
            .key("sk", s("PROFILE"))
            .update_expression("SET stripeAccountId = :account")
            .expression_attribute_values(":account", s(account_id))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn set_payouts_ready(&self, reader_id: &str, ready: bool) -> Result<(), StoreError> {
        self.client
            .update_item()
            .table_name(&self.readers)
            .key("readerId", s(reader_id))
            .key("sk", s("PROFILE"))
            .update_expression("SET payoutsReady = :ready")
            .expression_attribute_values(":ready", AttributeValue::Bool(ready))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn put_tip(
        &self,
        tip_id: &str,
        kind: &str,
        user_id: &str,
        reader_id: Option<&str>,
        match_id: Option<&str>,
        amount_cents: u32,
    ) -> Result<(), StoreError> {
        let mut request = self
            .client
            .put_item()
            .table_name(&self.tips)
            .item("tipId", s(tip_id))
            .item("kind", s(kind))
            .item("userId", s(user_id))
            .item("amountCents", AttributeValue::N(amount_cents.to_string()))
            .item("status", s("pending"))
            .item("createdAt", s(Utc::now().to_rfc3339()));
        if let Some(reader_id) = reader_id {
            request = request.item("readerId", s(reader_id));
        }
        if let Some(match_id) = match_id {
            request = request.item("matchId", s(match_id));
        }
        request.send().await.map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn set_checkout_session(&self, tip_id: &str, session_id: &str) -> Result<(), StoreError> {
        self.client
            .update_item()
            .table_name(&self.tips)
            .key("tipId", s(tip_id))
            .update_expression("SET checkoutSessionId = :session")
            .expression_attribute_values(":session", s(session_id))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn mark_tip_paid(&self, tip_id: &str, session_id: &str) -> Result<(), TipUpdateError> {
        match self
            .client
            .update_item()
            .table_name(&self.tips)
            .key("tipId", s(tip_id))
            .update_expression("SET #status = :paid")
            .condition_expression(TIP_PAID_CONDITION)
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":paid", s("paid"))
            .expression_attribute_values(":session", s(session_id))
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(error) => {
                let conflict = error
                    .as_service_error()
                    .map(UpdateItemError::is_conditional_check_failed_exception)
                    .unwrap_or(false);
                Err(classify_tip_update(conflict))
            }
        }
    }

    pub async fn tips_received_cents(&self, reader_id: &str) -> Result<u64, StoreError> {
        let page = self
            .client
            .query()
            .table_name(&self.tips)
            .index_name("ByReader")
            .key_condition_expression("readerId = :reader")
            .expression_attribute_values(":reader", s(reader_id))
            .filter_expression("#status = :paid")
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":paid", s("paid"))
            .projection_expression("amountCents")
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(page.items().iter().map(|item| attr_n_u64(item, "amountCents")).sum())
    }

    /// Returns true when this invocation claimed the event.
    pub async fn claim_stripe_event(&self, event_id: &str) -> Result<bool, StoreError> {
        let result = self
            .client
            .put_item()
            .table_name(&self.tips)
            .item("tipId", s(format!("event#{event_id}")))
            .item("kind", s("event"))
            .item("status", s("seen"))
            .condition_expression("attribute_not_exists(tipId)")
            .send()
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(error) => {
                let duplicate = error
                    .as_service_error()
                    .map(PutItemError::is_conditional_check_failed_exception)
                    .unwrap_or(false);
                if duplicate {
                    Ok(false)
                } else {
                    Err(StoreError)
                }
            }
        }
    }

    /// Drop a claim so Stripe can retry after a transient write failure.
    pub async fn release_stripe_event(&self, event_id: &str) -> Result<(), StoreError> {
        self.client
            .delete_item()
            .table_name(&self.tips)
            .key("tipId", s(format!("event#{event_id}")))
            .condition_expression("attribute_exists(tipId)")
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn bump_address_reads(
        &self,
        writer_id: &str,
        now: DateTime<Utc>,
        max: u32,
    ) -> Result<(), ApiFailure> {
        let existing = self
            .get_item(&self.users, "userId", writer_id, "sk", "RATE", None)
            .await
            .map_err(|_| ApiFailure::Invalid)?;
        let parsed = existing.as_ref().and_then(|item| {
            let start = attr_s(item, "addressWindowStart")?;
            let count = attr_n_u64(item, "addressReadCount") as u32;
            let start = DateTime::parse_from_rfc3339(&start).ok()?.with_timezone(&Utc);
            Some((start, count))
        });
        let (start, count) = rate_limit::bump_daily_counter(parsed, now, max).map_err(|error| match error {
            RateLimitError::Limited => ApiFailure::RateLimited,
        })?;
        self.client
            .put_item()
            .table_name(&self.users)
            .item("userId", s(writer_id))
            .item("sk", s("RATE"))
            .item("addressWindowStart", s(start.to_rfc3339()))
            .item("addressReadCount", AttributeValue::N(count.to_string()))
            .send()
            .await
            .map_err(|_| ApiFailure::Invalid)?;
        Ok(())
    }

    pub async fn block_writer(&self, reader_id: &str, writer_id: &str) -> Result<(), ApiFailure> {
        let projection = "readerId, sk, pseudonym, blockedWriterIds, activePk";
        let item = self
            .get_item(&self.readers, "readerId", reader_id, "sk", "PROFILE", Some(projection))
            .await
            .map_err(|_| ApiFailure::Invalid)?
            .ok_or(ApiFailure::NotFound)?;
        let mut blocked = item
            .get("blockedWriterIds")
            .and_then(|value| value.as_ss().ok())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| WriterId::new(value).ok())
            .collect();
        let writer = WriterId::new(writer_id).map_err(|_| ApiFailure::Invalid)?;
        reports::block_writer(&mut blocked, writer).map_err(|error| match error {
            ReportError::Limit => ApiFailure::RateLimited,
            ReportError::NotFound | ReportError::InvalidReason => ApiFailure::Invalid,
        })?;
        let ids: Vec<String> = blocked.iter().map(|id| id.as_str().to_string()).collect();
        self.client
            .update_item()
            .table_name(&self.readers)
            .key("readerId", s(reader_id))
            .key("sk", s("PROFILE"))
            .update_expression("SET blockedWriterIds = :blocked")
            .expression_attribute_values(":blocked", AttributeValue::Ss(ids))
            .send()
            .await
            .map_err(|_| ApiFailure::Invalid)?;
        Ok(())
    }

    pub async fn delete_address(&self, reader_id: &str) -> Result<(), StoreError> {
        self.client
            .delete_item()
            .table_name(&self.readers)
            .key("readerId", s(reader_id))
            .key("sk", s("ADDRESS"))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn deactivate_reader(&self, reader_id: &str) -> Result<(), StoreError> {
        self.client
            .update_item()
            .table_name(&self.readers)
            .key("readerId", s(reader_id))
            .key("sk", s("PROFILE"))
            .update_expression("REMOVE activePk")
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn delete_profile(&self, user_id: &str, role: stranger_domain::authz::Role) -> Result<(), StoreError> {
        let (table, key_name) = match role {
            stranger_domain::authz::Role::Writer => (&self.users, "userId"),
            stranger_domain::authz::Role::Reader => (&self.readers, "readerId"),
        };
        self.client
            .delete_item()
            .table_name(table)
            .key(key_name, s(user_id))
            .key("sk", s("PROFILE"))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn anonymize_writer(&self, writer: &WriterId, pseudonym: &str) -> Result<(), StoreError> {
        for record in self.matches_for_writer(writer).await? {
            self.client
                .update_item()
                .table_name(&self.matches)
                .key("writerId", s(writer.as_str()))
                .key("matchedAt", s(record.matched_at.to_rfc3339()))
                .update_expression("SET writerPseudonym = :name, current = :current")
                .expression_attribute_values(":name", s(pseudonym))
                .expression_attribute_values(":current", AttributeValue::Bool(false))
                .send()
                .await
                .map_err(|_| StoreError)?;
        }
        Ok(())
    }

    pub async fn anonymize_reader(&self, reader: &ReaderId, pseudonym: &str) -> Result<(), StoreError> {
        for record in self.matches_for_reader(reader).await? {
            self.client
                .update_item()
                .table_name(&self.matches)
                .key("writerId", s(record.writer_id.as_str()))
                .key("matchedAt", s(record.matched_at.to_rfc3339()))
                .update_expression("SET readerPseudonym = :name")
                .expression_attribute_values(":name", s(pseudonym))
                .send()
                .await
                .map_err(|_| StoreError)?;
        }
        Ok(())
    }

    pub async fn turns_for_match(&self, match_id: &MatchId) -> Result<Vec<Turn>, StoreError> {
        let page = self
            .client
            .query()
            .table_name(&self.correspondence)
            .key_condition_expression("matchId = :match AND begins_with(sk, :prefix)")
            .expression_attribute_values(":match", s(match_id.as_str()))
            .expression_attribute_values(":prefix", s("turn#"))
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| StoreError)?;
        let mut turns: Vec<Turn> = page
            .items()
            .iter()
            .map(items::turn_from_item)
            .collect::<Result<_, _>>()?;
        turns.sort_by_key(|turn| turn.sequence);
        Ok(turns)
    }

    pub async fn turns_for_matches(
        &self,
        ids: &[MatchId],
    ) -> Result<std::collections::HashMap<String, Vec<Turn>>, StoreError> {
        let mut map = std::collections::HashMap::new();
        for id in ids {
            map.insert(id.as_str().to_string(), self.turns_for_match(id).await?);
        }
        Ok(map)
    }

    /// Returns true when this call created the turn. A false result means the
    /// turn already existed, so the caller must not send another notice.
    pub async fn put_turn_if_absent(&self, match_id: &MatchId, turn: &Turn) -> Result<bool, StoreError> {
        let result = self
            .client
            .put_item()
            .table_name(&self.correspondence)
            .set_item(Some(items::turn_item(match_id.as_str(), turn)))
            .condition_expression("attribute_not_exists(sk)")
            .send()
            .await;
        conditional_put(result)
    }

    pub async fn mark_turn_received(
        &self,
        match_id: &MatchId,
        sequence: u32,
        at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let result = self
            .client
            .update_item()
            .table_name(&self.correspondence)
            .key("matchId", s(match_id.as_str()))
            .key("sk", s(items::turn_sk(sequence)))
            .update_expression("SET receivedAt = :at")
            .condition_expression("attribute_exists(mailedAt) AND attribute_not_exists(receivedAt)")
            .expression_attribute_values(":at", s(at.to_rfc3339()))
            .send()
            .await;
        conditional_update(result)
    }

    /// One dispatch per turn. A second caller loses the conditional write and must not send.
    pub async fn claim_notice_dispatch(
        &self,
        match_id: &str,
        sequence: u32,
        at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let result = self
            .client
            .update_item()
            .table_name(&self.correspondence)
            .key("matchId", s(match_id))
            .key("sk", s(items::turn_sk(sequence)))
            .update_expression("SET noticeDispatchAt = :at")
            .condition_expression("attribute_exists(noticeClaimedAt) AND attribute_not_exists(noticeDispatchAt)")
            .expression_attribute_values(":at", s(at.to_rfc3339()))
            .send()
            .await;
        conditional_update(result)
    }

    pub async fn mark_notice_sent(
        &self,
        match_id: &str,
        sequence: u32,
        at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let result = self
            .client
            .update_item()
            .table_name(&self.correspondence)
            .key("matchId", s(match_id))
            .key("sk", s(items::turn_sk(sequence)))
            .update_expression("SET noticeSentAt = :at")
            .condition_expression("attribute_exists(noticeClaimedAt) AND attribute_not_exists(noticeSentAt)")
            .expression_attribute_values(":at", s(at.to_rfc3339()))
            .send()
            .await;
        conditional_update(result)
    }

    pub async fn bump_limit(
        &self,
        subject: &str,
        now: DateTime<Utc>,
        max: u32,
        window: chrono::Duration,
    ) -> Result<(), ApiFailure> {
        if subject.contains('@') || subject.contains(' ') || subject.len() > 80 {
            return Err(ApiFailure::Invalid);
        }
        if max == 0 || window <= chrono::Duration::zero() {
            return Err(ApiFailure::RateLimited);
        }
        // Epoch seconds, so the condition compares numbers. This matches bump_window:
        // inside the window the count increments until max; otherwise it starts over at 1.
        let oldest = (now - window).timestamp().to_string();
        let now_secs = now.timestamp().to_string();
        let ttl = (now + window + chrono::Duration::hours(1)).timestamp().to_string();
        let increment = self
            .client
            .update_item()
            .table_name(&self.rates)
            .key("subject", s(subject))
            .key("sk", s("window"))
            .update_expression("ADD #count :one")
            .condition_expression(
                "attribute_exists(sk) AND windowStart > :oldest AND windowStart <= :now AND #count < :max",
            )
            .expression_attribute_names("#count", "count")
            .expression_attribute_values(":one", AttributeValue::N("1".into()))
            .expression_attribute_values(":oldest", AttributeValue::N(oldest.clone()))
            .expression_attribute_values(":now", AttributeValue::N(now_secs.clone()))
            .expression_attribute_values(":max", AttributeValue::N(max.to_string()))
            .send()
            .await;
        match conditional_update(increment) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(_) => return Err(ApiFailure::Invalid),
        }
        let reset = self
            .client
            .update_item()
            .table_name(&self.rates)
            .key("subject", s(subject))
            .key("sk", s("window"))
            .update_expression("SET #count = :one, windowStart = :now, #ttl = :ttl")
            .condition_expression("attribute_not_exists(sk) OR windowStart <= :oldest OR windowStart > :now")
            .expression_attribute_names("#count", "count")
            .expression_attribute_names("#ttl", "ttl")
            .expression_attribute_values(":one", AttributeValue::N("1".into()))
            .expression_attribute_values(":now", AttributeValue::N(now_secs))
            .expression_attribute_values(":ttl", AttributeValue::N(ttl))
            .expression_attribute_values(":oldest", AttributeValue::N(oldest))
            .send()
            .await;
        match conditional_update(reset) {
            Ok(true) => Ok(()),
            Ok(false) => Err(ApiFailure::RateLimited),
            Err(_) => Err(ApiFailure::Invalid),
        }
    }

    pub async fn save_private_copy(
        &self,
        owner_id: &str,
        match_id: &str,
        body: &str,
        saved_at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.preservation)
            .item("ownerId", s(owner_id))
            .item("sk", s(format!("copy#{match_id}")))
            .item("matchId", s(match_id))
            .item("body", s(body))
            .item("savedAt", s(saved_at.to_rfc3339()))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn list_owned(
        &self,
        owner_id: &str,
        prefix: &str,
        limit: i32,
        cursor: Option<&str>,
    ) -> Result<(Vec<HashMap<String, AttributeValue>>, Option<String>), StoreError> {
        if let Some(cursor) = cursor {
            if !cursor.starts_with(prefix) {
                return Err(StoreError);
            }
        }
        let mut request = self
            .client
            .query()
            .table_name(&self.preservation)
            .key_condition_expression("ownerId = :owner AND begins_with(sk, :prefix)")
            .expression_attribute_values(":owner", s(owner_id))
            .expression_attribute_values(":prefix", s(prefix))
            .limit(limit)
            .consistent_read(true);
        if let Some(cursor) = cursor {
            request = request
                .exclusive_start_key("ownerId", s(owner_id))
                .exclusive_start_key("sk", s(cursor));
        }
        let page = request.send().await.map_err(|_| StoreError)?;
        let next = page.last_evaluated_key().and_then(|key| attr_s(key, "sk"));
        Ok((page.items().to_vec(), next))
    }

    pub async fn delete_owned_sk(&self, owner_id: &str, sk: &str) -> Result<(), StoreError> {
        self.client
            .delete_item()
            .table_name(&self.preservation)
            .key("ownerId", s(owner_id))
            .key("sk", s(sk))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn archive_body(&self, owner_id: &str, match_id: &str) -> Result<Option<String>, StoreError> {
        let item = self
            .get_item(
                &self.preservation,
                "ownerId",
                owner_id,
                "sk",
                &format!("archive#{match_id}"),
                Some("body"),
            )
            .await?;
        Ok(item.as_ref().and_then(|row| attr_s(row, "body")))
    }

    pub async fn save_archive_submission(
        &self,
        owner_id: &str,
        match_id: &str,
        kind: ArchiveKind,
        body: &str,
        saved_at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.preservation)
            .item("ownerId", s(owner_id))
            .item("sk", s(format!("archive#{match_id}")))
            .item("matchId", s(match_id))
            .item("kind", s(kind.as_str()))
            .item("body", s(body))
            .item("status", s("pending"))
            .item("curated", AttributeValue::Bool(false))
            .item("savedAt", s(saved_at.to_rfc3339()))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn pair_exists(&self, match_id: &str) -> Result<bool, StoreError> {
        let item = self
            .get_item(&self.preservation, "ownerId", "PAIR", "sk", match_id, Some("status"))
            .await?;
        Ok(item.is_some())
    }

    /// Queue a pair for curation. Both texts are already screened. The row is
    /// never served by a public route. `curated` stays false.
    pub async fn queue_pair(
        &self,
        match_id: &str,
        thought: &str,
        reflection: &str,
        writer_id: &str,
        reader_id: &str,
        saved_at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let result = self
            .client
            .put_item()
            .table_name(&self.preservation)
            .item("ownerId", s("PAIR"))
            .item("sk", s(match_id))
            .item("thought", s(thought))
            .item("reflection", s(reflection))
            .item("status", s("pending"))
            .item("curated", AttributeValue::Bool(false))
            .item("writerOwnerId", s(writer_id))
            .item("readerOwnerId", s(reader_id))
            .item("savedAt", s(saved_at.to_rfc3339()))
            .condition_expression("attribute_not_exists(sk)")
            .send()
            .await;
        let _ = conditional_put(result)?;
        Ok(())
    }

    pub async fn mail_alerts_enabled(&self, role: Role, user_id: &str) -> Result<bool, StoreError> {
        let (table, key_name) = match role {
            Role::Writer => (self.users.as_str(), "userId"),
            Role::Reader => (self.readers.as_str(), "readerId"),
        };
        let item = self
            .get_item(table, key_name, user_id, "sk", "PROFILE", Some("mailAlerts"))
            .await?;
        let stored = item
            .as_ref()
            .and_then(|row| row.get("mailAlerts"))
            .and_then(|value| value.as_bool().ok())
            .copied();
        Ok(stranger_domain::preferences::mail_alerts_enabled(stored))
    }

    pub async fn set_mail_alerts(&self, role: Role, user_id: &str, enabled: bool) -> Result<(), StoreError> {
        let (table, key_name) = match role {
            Role::Writer => (self.users.as_str(), "userId"),
            Role::Reader => (self.readers.as_str(), "readerId"),
        };
        self.client
            .update_item()
            .table_name(table)
            .key(key_name, s(user_id))
            .key("sk", s("PROFILE"))
            .update_expression("SET mailAlerts = :enabled")
            .condition_expression("attribute_exists(sk)")
            .expression_attribute_values(":enabled", AttributeValue::Bool(enabled))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
    }

    pub async fn delete_owner_preservation(&self, owner_id: &str) -> Result<(), StoreError> {
        for prefix in ["copy#", "archive#"] {
            let mut cursor = None;
            loop {
                let (items, next) = self.list_owned(owner_id, prefix, 20, cursor.as_deref()).await?;
                for item in &items {
                    let sk = attr_s(item, "sk").ok_or(StoreError)?;
                    self.delete_owned_sk(owner_id, &sk).await?;
                    if let Some(match_id) = sk.strip_prefix("archive#") {
                        self.client
                            .delete_item()
                            .table_name(&self.preservation)
                            .key("ownerId", s("PAIR"))
                            .key("sk", s(match_id))
                            .send()
                            .await
                            .map_err(|_| StoreError)?;
                    }
                }
                match next {
                    Some(value) => cursor = Some(value),
                    None => break,
                }
            }
        }
        Ok(())
    }

    async fn query_matches(
        &self,
        key_name: &str,
        key_value: &str,
        index: Option<&str>,
    ) -> Result<Vec<MatchRecord>, StoreError> {
        let mut request = self
            .client
            .query()
            .table_name(&self.matches)
            .key_condition_expression(format!("{key_name} = :value"))
            .expression_attribute_values(":value", s(key_value))
            .consistent_read(index.is_none());
        if let Some(index) = index {
            request = request.index_name(index);
        }
        let page = request.send().await.map_err(|_| StoreError)?;
        page.items().iter().map(items::match_from_item).collect()
    }

    async fn get_item(
        &self,
        table: &str,
        key_name: &str,
        key_value: &str,
        sort_name: &str,
        sort_value: &str,
        projection: Option<&str>,
    ) -> Result<Option<HashMap<String, AttributeValue>>, StoreError> {
        let mut request = self
            .client
            .get_item()
            .table_name(table)
            .key(key_name, s(key_value))
            .key(sort_name, s(sort_value))
            .consistent_read(true);
        if let Some(projection) = projection {
            request = request.projection_expression(projection);
        }
        let output = request.send().await.map_err(|_| StoreError)?;
        Ok(output.item)
    }
}

fn conditional_put(
    result: Result<
        aws_sdk_dynamodb::operation::put_item::PutItemOutput,
        aws_sdk_dynamodb::error::SdkError<PutItemError, aws_sdk_dynamodb::config::http::HttpResponse>,
    >,
) -> Result<bool, StoreError> {
    match result {
        Ok(_) => Ok(true),
        Err(error) => {
            let duplicate = error
                .as_service_error()
                .map(PutItemError::is_conditional_check_failed_exception)
                .unwrap_or(false);
            if duplicate {
                Ok(false)
            } else {
                Err(StoreError)
            }
        }
    }
}

fn conditional_update(
    result: Result<
        aws_sdk_dynamodb::operation::update_item::UpdateItemOutput,
        aws_sdk_dynamodb::error::SdkError<UpdateItemError, aws_sdk_dynamodb::config::http::HttpResponse>,
    >,
) -> Result<bool, StoreError> {
    match result {
        Ok(_) => Ok(true),
        Err(error) => {
            let duplicate = error
                .as_service_error()
                .map(UpdateItemError::is_conditional_check_failed_exception)
                .unwrap_or(false);
            if duplicate {
                Ok(false)
            } else {
                Err(StoreError)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TIP_PAID_CONDITION;

    #[test]
    fn tip_paid_guard_requires_the_row_and_the_checkout_session() {
        assert!(TIP_PAID_CONDITION.contains("attribute_exists(tipId)"));
        assert!(TIP_PAID_CONDITION.contains("checkoutSessionId = :session"));
    }

    #[test]
    fn builder_session_matches_mark_tip_paid_and_a_mismatch_is_a_conflict() {
        let stored = "cs_test_builder";
        assert!(super::tip_paid_condition_matches(stored, stored));
        assert!(!super::tip_paid_condition_matches(stored, "cs_test_other"));
        assert!(!super::tip_paid_condition_matches("", stored));
        assert_eq!(super::classify_tip_update(true), super::TipUpdateError::Conflict);
        assert_eq!(super::classify_tip_update(false), super::TipUpdateError::Transient);
    }
}
