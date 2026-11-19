use std::collections::HashMap;

use aws_sdk_dynamodb::operation::put_item::PutItemError;
use aws_sdk_dynamodb::types::{AttributeValue, Put, TransactWriteItem, Update};
use aws_sdk_dynamodb::Client;
use chrono::{DateTime, Utc};
use stranger_domain::address::SealedAddress;
use stranger_domain::ids::{MatchId, ReaderId, WriterId};
use stranger_domain::matching::MatchCandidate;
use stranger_domain::model::MatchRecord;
use stranger_domain::rate_limit::{self, RateLimitError};
use stranger_domain::reports::{self, ReportError};
use stranger_domain::validation::Demographics;

use crate::http::ApiFailure;
use crate::items::{self, attr_bool, attr_n_u64, attr_s, s};

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
}

impl Store {
    pub fn from_env(client: Client) -> Self {
        Self {
            client,
            users: std::env::var("USERS_TABLE").unwrap_or_default(),
            readers: std::env::var("READERS_TABLE").unwrap_or_default(),
            matches: std::env::var("MATCHES_TABLE").unwrap_or_default(),
            tips: std::env::var("TIPS_TABLE").unwrap_or_default(),
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

    pub async fn mark_tip_paid(&self, tip_id: &str, session_id: &str) -> Result<(), StoreError> {
        self.client
            .update_item()
            .table_name(&self.tips)
            .key("tipId", s(tip_id))
            .update_expression("SET #status = :paid, checkoutSessionId = :session")
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":paid", s("paid"))
            .expression_attribute_values(":session", s(session_id))
            .send()
            .await
            .map_err(|_| StoreError)?;
        Ok(())
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
