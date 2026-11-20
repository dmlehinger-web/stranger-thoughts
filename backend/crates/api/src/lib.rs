//! HTTP handlers for the Stranger Thoughts Lambdas.
//!
//! Rules live in `stranger-domain`. This crate loads DynamoDB, KMS, and
//! Secrets Manager, then returns JSON that has already been filtered.

mod aws;
mod http;
mod items;
mod notify;
mod preservation;
mod store;
mod triggers;

pub use notify::handle_notify;
pub use preservation::preservation;

use std::collections::HashMap;

use chrono::Utc;
use lambda_http::http::Method;
use lambda_http::{Body, Request, Response};
use serde_json::json;
use stranger_domain::account::{plan_deletion, REMOVED_PSEUDONYM};
use stranger_domain::address::{open_address, seal_address, PlainDataKey};
use stranger_domain::authz::{self, Role};
use stranger_domain::checkout::{
    account_link_form, checkout_form, enforce_stripe_mode, express_account_form, live_gate_open,
    CheckoutPlan, STRIPE_ACCOUNT_LINK_URL, STRIPE_ACCOUNT_URL, STRIPE_CHECKOUT_URL,
};
use stranger_domain::correspondence::{self, MarkResult};
use stranger_domain::ids::{MatchId, ReaderId, WriterId};
use stranger_domain::model::{reader_expectations, writer_home, ReaderHomeView};
use stranger_domain::preferences::MailAlertPreference;
use stranger_domain::validation::MailAlertsRequest;
use stranger_domain::DEFAULT_MAX_MAIL_MARKS_PER_DAY;
use stranger_domain::rate_limit::RateLimitError;
use stranger_domain::redaction::SafeLog;
use stranger_domain::reports::{self, authorize_report};
use stranger_domain::rotate::{self, RotateInput};
use stranger_domain::tips::{self, TipKind};
use stranger_domain::validation::{
    self, BuilderTipRequest, DemographicsRequest, ReaderProfileRequest, ReaderTipRequest,
    ReportRequest, SentRequest, WriterProfileRequest,
};
use stranger_domain::{
    check_body_len, DEFAULT_MAX_ADDRESS_READS_PER_DAY, DEFAULT_MAX_ROTATES_PER_DAY,
    WEBHOOK_TOLERANCE_SECS,
};
use uuid::Uuid;

use crate::aws::Aws;
use crate::http::{failure, json_response, ApiFailure};
use crate::store::Store;

pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .json()
        .try_init();
}

pub async fn health(request: Request) -> Result<Response<Body>, lambda_http::Error> {
    let response = match health_inner(&request).await {
        Ok(response) => response,
        Err(error) => failure(&error),
    };
    Ok(response)
}

async fn health_inner(request: &Request) -> Result<Response<Body>, ApiFailure> {
    let subject = stranger_domain::rate_limit::hash_subject("health", &http::source_ip(request));
    store()
        .await?
        .bump_limit(&subject, Utc::now(), 60, chrono::Duration::minutes(10))
        .await?;
    Ok(json_response(200, &json!({"ok": true})))
}

pub async fn profile(request: Request) -> Result<Response<Body>, lambda_http::Error> {
    let path = request.uri().path().to_string();
    let response = match profile_inner(&request).await {
        Ok(response) => response,
        Err(error) => failure(&error),
    };
    Ok(noted(&path, response))
}

async fn profile_inner(request: &Request) -> Result<Response<Body>, ApiFailure> {
    let principal = http::principal(request)?;
    authz::require_verified(&principal)?;
    let store = store().await?;
    let method = request.method().as_str();
    let path = request.uri().path();
    match (method, path) {
        ("POST", "/writers/profile") => {
            authz::require_role(&principal, Role::Writer)?;
            let body: WriterProfileRequest = parse_json(request)?;
            let pseudonym = validation::validate_pseudonym(&body.pseudonym)?;
            let writer = authz::writer_id(&principal)?;
            store
                .put_writer_profile(writer.as_str(), &pseudonym)
                .await?;
            Ok(json_response(200, &json!({"pseudonym": pseudonym})))
        }
        ("PUT", "/writers/demographics") => {
            authz::require_role(&principal, Role::Writer)?;
            let body: DemographicsRequest = parse_json(request)?;
            let demographics = validation::parse_demographics(&body)?;
            let writer = authz::writer_id(&principal)?;
            store.put_demographics(writer.as_str(), &demographics).await?;
            Ok(json_response(200, &json!({"saved": true})))
        }
        ("DELETE", "/writers/demographics") => {
            authz::require_role(&principal, Role::Writer)?;
            let writer = authz::writer_id(&principal)?;
            store.delete_demographics(writer.as_str()).await?;
            Ok(json_response(200, &json!({"deleted": true})))
        }
        ("POST", "/readers/profile") => {
            authz::require_role(&principal, Role::Reader)?;
            let body: ReaderProfileRequest = parse_json(request)?;
            let pseudonym = validation::validate_pseudonym(&body.pseudonym)?;
            validation::validate_address(&body.address)?;
            let reader = authz::reader_id(&principal)?;
            let encrypted_key = seal_for_reader(&reader, &body.address).await?;
            store
                .put_reader_profile(reader.as_str(), &pseudonym, true)
                .await?;
            store.put_address(reader.as_str(), &encrypted_key).await?;
            Ok(json_response(200, &json!({"pseudonym": pseudonym})))
        }
        _ => Err(ApiFailure::NotFound),
    }
}

pub async fn matching(request: Request) -> Result<Response<Body>, lambda_http::Error> {
    let path = request.uri().path().to_string();
    let response = match matching_inner(&request).await {
        Ok(response) => response,
        Err(error) => failure(&error),
    };
    Ok(noted(&path, response))
}

async fn matching_inner(request: &Request) -> Result<Response<Body>, ApiFailure> {
    let principal = http::principal(request)?;
    authz::require_verified(&principal)?;
    let store = store().await?;
    let method = request.method().as_str();
    let path = request.uri().path();
    if method == "POST" && (path.ends_with("/mailed") || path.ends_with("/received")) {
        let kind = if path.ends_with("/received") { "received" } else { "mailed" };
        let match_id = path
            .strip_prefix("/matches/")
            .and_then(|rest| rest.strip_suffix(if kind == "received" { "/received" } else { "/mailed" }))
            .ok_or(ApiFailure::NotFound)?;
        return mail_action(principal, &store, match_id, kind).await;
    }
    authz::require_role(&principal, Role::Writer)?;
    let writer = authz::writer_id(&principal)?;
    if method == "GET" && path == "/writers/home" {
        let profile = store.writer_pseudonym(writer.as_str()).await?;
        let history = store.matches_for_writer(&writer).await?;
        let turns = store.turns_for_matches(&history.iter().map(|record| record.match_id.clone()).collect::<Vec<_>>()).await?;
        let view = writer_home(&profile, &history, &turns);
        return Ok(json_response(200, &view));
    }
    if method == "POST" && path == "/writers/rotate" {
        let history = store.matches_for_writer(&writer).await?;
        let candidates = store.active_candidates().await?;
        let profile = store.writer_pseudonym(writer.as_str()).await?;
        let max = env_u32("MAX_ROTATES_PER_DAY", DEFAULT_MAX_ROTATES_PER_DAY);
        let mut rng = rand::rngs::OsRng;
        let outcome = rotate::rotate(
            &mut rng,
            RotateInput {
                writer_id: &writer,
                writer_pseudonym: &profile,
                now: Utc::now(),
                history: &history,
                candidates: &candidates,
                max_per_day: max,
                new_match_id: MatchId::new(Uuid::new_v4().to_string()).map_err(|_| ApiFailure::Invalid)?,
            },
        )?;
        let previous = history.iter().find(|record| record.current);
        store.save_rotation(previous, &outcome.created).await?;
        let turns = store
            .turns_for_matches(
                &outcome
                    .history
                    .iter()
                    .map(|record| record.match_id.clone())
                    .collect::<Vec<_>>(),
            )
            .await?;
        let view = writer_home(&profile, &outcome.history, &turns);
        return Ok(json_response(200, &view));
    }
    if method == "POST" {
        if let Some(match_id) = path
            .strip_prefix("/matches/")
            .and_then(|rest| rest.strip_suffix("/sent"))
        {
            let body: SentRequest = parse_json(request)?;
            let match_id = MatchId::new(match_id).map_err(|_| ApiFailure::NotFound)?;
            let mut history = store.matches_for_writer(&writer).await?;
            rotate::mark_sent(&mut history, &writer, &match_id, body.sent)?;
            store.set_sent(&writer, &match_id, body.sent).await?;
            return Ok(json_response(200, &json!({"sent": body.sent})));
        }
    }
    Err(ApiFailure::NotFound)
}

pub async fn address(request: Request) -> Result<Response<Body>, lambda_http::Error> {
    let path = request.uri().path().to_string();
    let response = match address_inner(&request).await {
        Ok(response) => response,
        Err(error) => failure(&error),
    };
    Ok(noted(&path, response))
}

async fn address_inner(request: &Request) -> Result<Response<Body>, ApiFailure> {
    if request.method() != Method::GET {
        return Err(ApiFailure::NotFound);
    }
    let Some(match_id) = request
        .uri()
        .path()
        .strip_prefix("/matches/")
        .and_then(|rest| rest.strip_suffix("/address"))
    else {
        return Err(ApiFailure::NotFound);
    };
    let principal = http::principal(request)?;
    let match_id = MatchId::new(match_id).map_err(|_| ApiFailure::NotFound)?;
    let store = store().await?;
    let writer = authz::writer_id(&principal)?;
    let history = store.matches_for_writer(&writer).await?;
    let grant = authz::authorize_address_read(&principal, &match_id, &history)?;
    let record = history
        .iter()
        .find(|record| record.match_id == grant.match_id)
        .ok_or(ApiFailure::NotFound)?;
    if std::env::var("MAILING_DESK_READY").ok().as_deref() != Some("true") {
        return Ok(json_response(
            200,
            &json!({
                "match_id": record.match_id.as_str(),
                "reader_pseudonym": record.reader_pseudonym,
                "desk_ready": false,
            }),
        ));
    }
    let max = env_u32("MAX_ADDRESS_READS_PER_DAY", DEFAULT_MAX_ADDRESS_READS_PER_DAY);
    store.bump_address_reads(writer.as_str(), Utc::now(), max).await?;
    let sealed = store
        .sealed_address(grant.reader_id.as_str())
        .await?
        .ok_or(ApiFailure::NotFound)?;
    let opened = unseal(&grant.reader_id, &sealed).await?;
    let record = history
        .iter()
        .find(|record| record.match_id == grant.match_id)
        .ok_or(ApiFailure::NotFound)?;
    Ok(json_response(
        200,
        &json!({
            "match_id": record.match_id.as_str(),
            "reader_pseudonym": record.reader_pseudonym,
            "address": opened,
        }),
    ))
}

pub async fn payments(request: Request) -> Result<Response<Body>, lambda_http::Error> {
    let path = request.uri().path().to_string();
    let response = match payments_inner(&request).await {
        Ok(response) => response,
        Err(error) => failure(&error),
    };
    Ok(noted(&path, response))
}

async fn payments_inner(request: &Request) -> Result<Response<Body>, ApiFailure> {
    let principal = http::principal(request)?;
    authz::require_verified(&principal)?;
    let store = store().await?;
    let aws = Aws::get().await?;
    let secret = stripe_parameter(aws, "STRIPE_API_KEY_PARAMETER").await?;
    let allow_live = live_gate_open(std::env::var("STRIPE_ALLOW_LIVE").ok().as_deref());
    enforce_stripe_mode(&secret, allow_live)?;
    let app_base = std::env::var("APP_BASE_URL").map_err(|_| ApiFailure::Invalid)?;
    let method = request.method().as_str();
    let path = request.uri().path();
    if method == "POST" && path == "/readers/payouts/link" {
        authz::require_role(&principal, Role::Reader)?;
        let reader = authz::reader_id(&principal)?;
        let profile = store
            .reader_payout(reader.as_str())
            .await?
            .ok_or(ApiFailure::NotFound)?;
        let account = if let Some(existing) = profile.stripe_account_id {
            existing
        } else {
            let created = stripe_post(
                aws,
                &secret,
                STRIPE_ACCOUNT_URL,
                &express_account_form(reader.as_str(), "US"),
            )
            .await?;
            let account_id = created
                .get("id")
                .and_then(|value| value.as_str())
                .filter(|id| id.starts_with("acct_"))
                .ok_or(ApiFailure::Invalid)?
                .to_string();
            store
                .set_stripe_account(reader.as_str(), &account_id)
                .await?;
            account_id
        };
        let return_url = format!("{}/read/home/", app_base.trim_end_matches('/'));
        let link = stripe_post(
            aws,
            &secret,
            STRIPE_ACCOUNT_LINK_URL,
            &account_link_form(&account, &return_url, &return_url),
        )
        .await?;
        let url = hosted_url(&link, "https://connect.stripe.com/")?;
        return Ok(json_response(200, &json!({"url": url})));
    }
    if method == "POST" && path == "/tips/reader" {
        authz::require_role(&principal, Role::Writer)?;
        let body: ReaderTipRequest = parse_json(request)?;
        let writer = authz::writer_id(&principal)?;
        let match_id = MatchId::new(body.match_id).map_err(|_| ApiFailure::NotFound)?;
        let history = store.matches_for_writer(&writer).await?;
        let record = history
            .iter()
            .find(|record| record.match_id == match_id)
            .ok_or(ApiFailure::NotFound)?;
        let payout = store
            .reader_payout(record.reader_id.as_str())
            .await?
            .ok_or(ApiFailure::PayoutsUnavailable)?;
        let mut accounts = HashMap::new();
        if let Some(account) = payout.stripe_account_id.clone() {
            accounts.insert(record.reader_id.clone(), account);
        }
        let routed = tips::route_reader_tip(&writer, &match_id, body.amount_cents, &history, &accounts)?;
        let tip_id = Uuid::new_v4().to_string();
        store
            .put_tip(
                &tip_id,
                "reader",
                principal.user_id.as_str(),
                Some(routed.reader_id.as_str()),
                Some(routed.match_id.as_str()),
                routed.amount_cents,
            )
            .await?;
        let home = format!("{}/write/home/", app_base.trim_end_matches('/'));
        let fields = checkout_form(
            &CheckoutPlan {
                tip_id: tip_id.clone(),
                user_id: principal.user_id.clone(),
                amount_cents: routed.amount_cents,
                kind: TipKind::Reader,
                destination_account: Some(routed.destination_account),
                reader_id: Some(routed.reader_id.as_str().to_string()),
                match_id: Some(routed.match_id.as_str().to_string()),
                success_url: home.clone(),
                cancel_url: home,
            },
            &app_base,
        )?;
        let session = stripe_post(aws, &secret, STRIPE_CHECKOUT_URL, &fields).await?;
        let url = hosted_url(&session, "https://checkout.stripe.com/")?;
        if let Some(session_id) = checkout_session_id(&session) {
            store.set_checkout_session(&tip_id, &session_id).await?;
        }
        return Ok(json_response(200, &json!({"url": url})));
    }
    if method == "POST" && path == "/tips/builder" {
        let body: BuilderTipRequest = parse_json(request)?;
        let routed = tips::route_builder_tip(&principal.user_id, body.amount_cents)?;
        let tip_id = Uuid::new_v4().to_string();
        store
            .put_tip(&tip_id, "builder", &routed.user_id, None, None, routed.amount_cents)
            .await?;
        let back = format!("{}/", app_base.trim_end_matches('/'));
        let fields = checkout_form(
            &CheckoutPlan {
                tip_id: tip_id.clone(),
                user_id: routed.user_id,
                amount_cents: routed.amount_cents,
                kind: TipKind::Builder,
                destination_account: None,
                reader_id: None,
                match_id: None,
                success_url: back.clone(),
                cancel_url: back,
            },
            &app_base,
        )?;
        let session = stripe_post(aws, &secret, STRIPE_CHECKOUT_URL, &fields).await?;
        let url = hosted_url(&session, "https://checkout.stripe.com/")?;
        if let Some(session_id) = checkout_session_id(&session) {
            store.set_checkout_session(&tip_id, &session_id).await?;
        }
        return Ok(json_response(200, &json!({"url": url})));
    }
    Err(ApiFailure::NotFound)
}

pub async fn webhook(request: Request) -> Result<Response<Body>, lambda_http::Error> {
    let path = request.uri().path().to_string();
    let response = match webhook_inner(&request).await {
        Ok(response) => response,
        Err(error) => failure(&error),
    };
    Ok(noted(&path, response))
}

async fn webhook_inner(request: &Request) -> Result<Response<Body>, ApiFailure> {
    if request.method() == Method::GET && request.uri().path() == "/health" {
        return Ok(json_response(200, &json!({"ok": true})));
    }
    if request.method() != Method::POST || request.uri().path() != "/webhooks/stripe" {
        return Err(ApiFailure::NotFound);
    }
    let subject = stranger_domain::rate_limit::hash_subject("webhook", &http::source_ip(request));
    store()
        .await?
        .bump_limit(&subject, Utc::now(), 300, chrono::Duration::minutes(10))
        .await?;
    let bytes = http::body_bytes(request.body());
    if bytes.len() > 64 * 1024 {
        return Err(ApiFailure::BodyTooLarge);
    }
    let header = request
        .headers()
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let aws = Aws::get().await?;
    let secret = stripe_parameter(aws, "STRIPE_WEBHOOK_SECRET_PARAMETER").await?;
    stranger_domain::webhook::verify_stripe_signature(
        &secret,
        header,
        bytes,
        Utc::now().timestamp(),
        WEBHOOK_TOLERANCE_SECS,
    )?;
    let event = stranger_domain::webhook::parse_event(bytes)?;
    let store = store().await?;
    if !store.claim_stripe_event(&event.id).await? {
        return Ok(json_response(200, &json!({"duplicate": true})));
    }
    let write = match &event.action {
        stranger_domain::webhook::WebhookAction::MarkTipPaid { tip_id, session_id } => {
            match store.mark_tip_paid(tip_id, session_id).await {
                Ok(()) => stranger_domain::webhook::ClaimedWrite::Done,
                Err(store::TipUpdateError::Conflict) => stranger_domain::webhook::ClaimedWrite::Conflict,
                Err(store::TipUpdateError::Transient) => stranger_domain::webhook::ClaimedWrite::Transient,
            }
        }
        stranger_domain::webhook::WebhookAction::UpdatePayouts { reader_id, ready } => {
            match store.set_payouts_ready(reader_id, *ready).await {
                Ok(()) => stranger_domain::webhook::ClaimedWrite::Done,
                Err(_) => stranger_domain::webhook::ClaimedWrite::Transient,
            }
        }
        stranger_domain::webhook::WebhookAction::Ignore => stranger_domain::webhook::ClaimedWrite::Done,
    };
    let disposition = stranger_domain::webhook::disposition_after_claim(write);
    if matches!(write, stranger_domain::webhook::ClaimedWrite::Conflict) {
        let log = SafeLog::new("tip update conflict").field("event_id", &event.id);
        tracing::warn!("{}", serde_json::to_string(&log).unwrap_or_else(|_| "{}".into()));
    }
    if disposition.release_claim {
        let _ = store.release_stripe_event(&event.id).await;
    }
    let body = if disposition.status == 503 {
        json!({"error": "retry"})
    } else {
        json!({"received": true})
    };
    Ok(json_response(disposition.status, &body))
}

pub async fn account(request: Request) -> Result<Response<Body>, lambda_http::Error> {
    let path = request.uri().path().to_string();
    let response = match account_inner(&request).await {
        Ok(response) => response,
        Err(error) => failure(&error),
    };
    Ok(noted(&path, response))
}

async fn account_inner(request: &Request) -> Result<Response<Body>, ApiFailure> {
    let principal = http::principal(request)?;
    authz::require_verified(&principal)?;
    let store = store().await?;
    let method = request.method().as_str();
    let path = request.uri().path();
    if path == "/account/settings" {
        return match method {
            "GET" => {
                let enabled = store.mail_alerts_enabled(principal.role, &principal.user_id).await?;
                Ok(json_response(200, &MailAlertPreference { mail_alerts: enabled }))
            }
            "PUT" => {
                let body: MailAlertsRequest = parse_json(request)?;
                store
                    .set_mail_alerts(principal.role, &principal.user_id, body.mail_alerts)
                    .await?;
                Ok(json_response(
                    200,
                    &MailAlertPreference {
                        mail_alerts: body.mail_alerts,
                    },
                ))
            }
            _ => Err(ApiFailure::NotFound),
        };
    }
    if method == "GET" && path == "/readers/home" {
        authz::require_role(&principal, Role::Reader)?;
        let reader = authz::reader_id(&principal)?;
        let profile = store
            .reader_payout(reader.as_str())
            .await?
            .ok_or(ApiFailure::NotFound)?;
        let history = store.matches_for_reader(&reader).await?;
        let turns = store
            .turns_for_matches(&history.iter().map(|record| record.match_id.clone()).collect::<Vec<_>>())
            .await?;
        let tips = store.tips_received_cents(reader.as_str()).await?;
        let view = ReaderHomeView {
            pseudonym: profile.pseudonym,
            letters: reader_expectations(&history, &turns),
            tips_received_cents: tips,
            payouts_ready: profile.payouts_ready,
        };
        return Ok(json_response(200, &view));
    }
    if method == "POST" && path == "/readers/reports" {
        authz::require_role(&principal, Role::Reader)?;
        let body: ReportRequest = parse_json(request)?;
        reports::parse_reason(&body.reason)?;
        let reader = authz::reader_id(&principal)?;
        let match_id = MatchId::new(body.match_id).map_err(|_| ApiFailure::NotFound)?;
        let history = store.matches_for_reader(&reader).await?;
        let record = history
            .iter()
            .find(|record| record.match_id == match_id)
            .ok_or(ApiFailure::NotFound)?;
        let writer = authorize_report(&principal, record)?;
        store.block_writer(reader.as_str(), writer.as_str()).await?;
        return Ok(json_response(200, &json!({"blocked": true})));
    }
    if method == "DELETE" && path == "/account" {
        let plan = plan_deletion(principal.role);
        if plan.delete_demographics {
            store.delete_demographics(&principal.user_id).await?;
        }
        if plan.delete_address_ciphertext {
            store.delete_address(&principal.user_id).await?;
        }
        if plan.deactivate_reader {
            store.deactivate_reader(&principal.user_id).await?;
        }
        if plan.anonymize_pseudonym {
            match principal.role {
                Role::Writer => {
                    let writer = WriterId::new(principal.user_id.clone()).map_err(|_| ApiFailure::Invalid)?;
                    store.anonymize_writer(&writer, REMOVED_PSEUDONYM).await?;
                }
                Role::Reader => {
                    let reader = ReaderId::new(principal.user_id.clone()).map_err(|_| ApiFailure::Invalid)?;
                    store.anonymize_reader(&reader, REMOVED_PSEUDONYM).await?;
                }
            }
        }
        if plan.delete_profile {
            store.delete_owner_preservation(&principal.user_id).await?;
            store.delete_profile(&principal.user_id, principal.role).await?;
        }
        let pool = std::env::var("USER_POOL_ID").map_err(|_| ApiFailure::Invalid)?;
        // `username` is the Cognito username claim. It is passed to
        // AdminDeleteUser and never written to a log.
        let username = http::claim(request, "username").unwrap_or_default();
        if !username.is_empty() {
            Aws::get()
                .await?
                .cognito
                .admin_delete_user()
                .user_pool_id(pool)
                .username(username)
                .send()
                .await
                .map_err(|_| ApiFailure::Invalid)?;
        }
        return Ok(json_response(200, &json!({"deleted": true})));
    }
    Err(ApiFailure::NotFound)
}

pub async fn handle_post_confirmation(
    event: serde_json::Value,
) -> Result<serde_json::Value, lambda_http::Error> {
    triggers::assign_group(event).await
}

pub fn handle_pre_token(event: serde_json::Value) -> serde_json::Value {
    triggers::shape_token(event)
}

pub(crate) fn checkout_session_id(session: &serde_json::Value) -> Option<String> {
    session
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

pub(crate) fn noted(path: &str, response: Response<Body>) -> Response<Body> {
    let status = response.status().as_u16();
    let log = SafeLog::new("handled")
        .field("path", path)
        .field("status", &status.to_string());
    tracing::info!("{}", serde_json::to_string(&log).unwrap_or_else(|_| "{}".into()));
    response
}

pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(request: &Request) -> Result<T, ApiFailure> {
    let bytes = http::body_bytes(request.body());
    check_body_len(bytes.len())?;
    serde_json::from_slice(bytes).map_err(|_| ApiFailure::Invalid)
}

pub(crate) async fn store() -> Result<Store, ApiFailure> {
    let aws = Aws::get().await?;
    Ok(Store::from_env(aws.dynamo.clone()))
}

async fn seal_for_reader(
    reader: &ReaderId,
    address: &stranger_domain::address::PostalAddress,
) -> Result<stranger_domain::address::SealedAddress, ApiFailure> {
    let aws = Aws::get().await?;
    let key_id = std::env::var("ADDRESS_KEY_ID").map_err(|_| ApiFailure::Invalid)?;
    let generated = aws
        .kms
        .generate_data_key()
        .key_id(key_id)
        .key_spec(aws_sdk_kms::types::DataKeySpec::Aes256)
        .encryption_context("purpose", "reader-mailing-address")
        .encryption_context("readerId", reader.as_str())
        .send()
        .await
        .map_err(|_| ApiFailure::Invalid)?;
    let plaintext = generated.plaintext.ok_or(ApiFailure::Invalid)?;
    let mut plain = plaintext.as_ref().to_vec();
    let data_key = PlainDataKey::new(std::mem::take(&mut plain)).map_err(|_| ApiFailure::Invalid)?;
    let wrapped = generated.ciphertext_blob.ok_or(ApiFailure::Invalid)?;
    seal_address(reader, address, &data_key, wrapped.as_ref()).map_err(|_| ApiFailure::Invalid)
}

async fn unseal(
    reader: &ReaderId,
    sealed: &stranger_domain::address::SealedAddress,
) -> Result<stranger_domain::address::PostalAddress, ApiFailure> {
    let aws = Aws::get().await?;
    let key_id = std::env::var("ADDRESS_KEY_ID").map_err(|_| ApiFailure::Invalid)?;
    let wrapped = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &sealed.encrypted_data_key,
    )
    .map_err(|_| ApiFailure::Invalid)?;
    let decrypted = aws
        .kms
        .decrypt()
        .key_id(key_id)
        .ciphertext_blob(wrapped.into())
        .encryption_context("purpose", "reader-mailing-address")
        .encryption_context("readerId", reader.as_str())
        .send()
        .await
        .map_err(|_| ApiFailure::Invalid)?;
    let plaintext = decrypted.plaintext.ok_or(ApiFailure::Invalid)?;
    let data_key = PlainDataKey::new(plaintext.as_ref().to_vec()).map_err(|_| ApiFailure::Invalid)?;
    open_address(reader, sealed, &data_key).map_err(|_| ApiFailure::Invalid)
}

async fn stripe_parameter(aws: &Aws, env_name: &str) -> Result<String, ApiFailure> {
    let name = std::env::var(env_name).map_err(|_| ApiFailure::Invalid)?;
    if name != "/st/stripe/api-key" && name != "/st/stripe/webhook-secret" {
        return Err(ApiFailure::Invalid);
    }
    let output = aws
        .ssm
        .get_parameter()
        .name(name)
        .with_decryption(true)
        .send()
        .await
        .map_err(|_| ApiFailure::Invalid)?;
    let raw = output
        .parameter()
        .and_then(|parameter| parameter.value())
        .ok_or(ApiFailure::Invalid)?;
    if raw.contains('\n') || raw.contains('\r') {
        return Err(ApiFailure::Invalid);
    }
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) {
        for key in ["api_key", "secret_key", "webhook_secret"] {
            if let Some(value) = parsed.get(key).and_then(|item| item.as_str()) {
                return Ok(value.trim().to_string());
            }
        }
        return Err(ApiFailure::Invalid);
    }
    Ok(raw.trim().to_string())
}

async fn mail_action(
    principal: stranger_domain::authz::Principal,
    store: &Store,
    match_id: &str,
    kind: &str,
) -> Result<Response<Body>, ApiFailure> {
    let match_id = MatchId::new(match_id).map_err(|_| ApiFailure::NotFound)?;
    let history = match principal.role {
        Role::Writer => {
            let writer = WriterId::new(principal.user_id.clone()).map_err(|_| ApiFailure::Invalid)?;
            store.matches_for_writer(&writer).await?
        }
        Role::Reader => {
            let reader = ReaderId::new(principal.user_id.clone()).map_err(|_| ApiFailure::Invalid)?;
            store.matches_for_reader(&reader).await?
        }
    };
    let record = history
        .iter()
        .find(|record| record.match_id == match_id)
        .ok_or(ApiFailure::NotFound)?;
    let mut turns = store.turns_for_match(&match_id).await?;
    let now = Utc::now();
    if kind == "received" {
        match correspondence::mark_received(principal.role, &turns, now)? {
            MarkResult::Received(turn) => {
                let subject = stranger_domain::rate_limit::hash_subject("mail", &principal.user_id);
                store
                    .bump_limit(&subject, now, DEFAULT_MAX_MAIL_MARKS_PER_DAY, chrono::Duration::hours(24))
                    .await?;
                if store.mark_turn_received(&match_id, turn.sequence, now).await? {
                    if let Some(slot) = turns.iter_mut().find(|item| item.sequence == turn.sequence) {
                        slot.received_at = Some(now);
                    }
                }
            }
            MarkResult::AlreadyReceived(_) => {}
            MarkResult::Created(_) | MarkResult::AlreadyMailed(_) => return Err(ApiFailure::Invalid),
        }
    } else {
        let turn_id = Uuid::new_v4().to_string();
        match correspondence::mark_mailed(principal.role, &turns, now, &turn_id)? {
            MarkResult::Created(turn) => {
                let subject = stranger_domain::rate_limit::hash_subject("mail", &principal.user_id);
                store
                    .bump_limit(&subject, now, DEFAULT_MAX_MAIL_MARKS_PER_DAY, chrono::Duration::hours(24))
                    .await?;
                if store.put_turn_if_absent(&match_id, &turn).await? {
                    let recipient_role = correspondence::recipient(turn.direction);
                    let recipient_id = match recipient_role {
                        Role::Writer => record.writer_id.as_str(),
                        Role::Reader => record.reader_id.as_str(),
                    };
                    let notice_kind = match turn.direction {
                        correspondence::Direction::WriterToReader => "letter",
                        correspondence::Direction::ReaderToWriter => "reply",
                    };
                    let _ = invoke_notice(recipient_id, recipient_role, notice_kind, &turn, match_id.as_str()).await;
                    turns.push(turn);
                }
            }
            MarkResult::AlreadyMailed(_) => {}
            MarkResult::Received(_) | MarkResult::AlreadyReceived(_) => return Err(ApiFailure::Invalid),
        }
    }
    let exchange = correspondence::exchange_for(principal.role, &turns);
    Ok(json_response(200, &exchange))
}

async fn invoke_notice(
    recipient_id: &str,
    role: Role,
    kind: &str,
    turn: &stranger_domain::correspondence::Turn,
    match_id: &str,
) -> Result<(), ApiFailure> {
    let name = std::env::var("NOTIFY_FUNCTION_NAME").unwrap_or_default();
    if name.is_empty() {
        return Ok(());
    }
    let payload = json!({
        "recipient_sub": recipient_id,
        "recipient_role": match role { Role::Writer => "writer", Role::Reader => "reader" },
        "kind": kind,
        "turn_id": turn.turn_id,
        "match_id": match_id,
        "sequence": turn.sequence,
    });
    let bytes = serde_json::to_vec(&payload).map_err(|_| ApiFailure::Invalid)?;
    let aws = Aws::get().await?;
    let _ = aws
        .lambda
        .invoke()
        .function_name(name)
        .invocation_type(aws_sdk_lambda::types::InvocationType::Event)
        .payload(aws_sdk_lambda::primitives::Blob::new(bytes))
        .send()
        .await;
    Ok(())
}

async fn stripe_post(
    aws: &Aws,
    secret: &str,
    url: &str,
    fields: &[(String, String)],
) -> Result<serde_json::Value, ApiFailure> {
    if !matches!(
        url,
        STRIPE_CHECKOUT_URL | STRIPE_ACCOUNT_URL | STRIPE_ACCOUNT_LINK_URL
    ) {
        return Err(ApiFailure::Invalid);
    }
    let response = aws
        .http
        .post(url)
        .basic_auth(secret, Some(""))
        .form(fields)
        .send()
        .await
        .map_err(|_| ApiFailure::Invalid)?;
    if !response.status().is_success() {
        return Err(ApiFailure::Invalid);
    }
    response.json().await.map_err(|_| ApiFailure::Invalid)
}

fn hosted_url(value: &serde_json::Value, prefix: &str) -> Result<String, ApiFailure> {
    let url = value
        .get("url")
        .and_then(|item| item.as_str())
        .ok_or(ApiFailure::Invalid)?;
    if url.starts_with(prefix) {
        Ok(url.to_string())
    } else {
        Err(ApiFailure::Invalid)
    }
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

impl From<authz::AuthzError> for ApiFailure {
    fn from(value: authz::AuthzError) -> Self {
        match value {
            authz::AuthzError::EmailUnverified => ApiFailure::EmailUnverified,
            authz::AuthzError::NotFound => ApiFailure::NotFound,
        }
    }
}

impl From<stranger_domain::validation::ValidationError> for ApiFailure {
    fn from(value: stranger_domain::validation::ValidationError) -> Self {
        match value {
            stranger_domain::validation::ValidationError::ChildAgeRejected => ApiFailure::ChildAge,
            stranger_domain::validation::ValidationError::BodyTooLarge => ApiFailure::BodyTooLarge,
            stranger_domain::validation::ValidationError::Invalid { .. } => ApiFailure::Invalid,
        }
    }
}

impl From<stranger_domain::correspondence::CorrespondenceError> for ApiFailure {
    fn from(value: stranger_domain::correspondence::CorrespondenceError) -> Self {
        match value {
            stranger_domain::correspondence::CorrespondenceError::Full => ApiFailure::RateLimited,
            stranger_domain::correspondence::CorrespondenceError::NotYourTurn
            | stranger_domain::correspondence::CorrespondenceError::Waiting => ApiFailure::NotFound,
        }
    }
}

impl From<stranger_domain::rotate::RotateError> for ApiFailure {
    fn from(value: stranger_domain::rotate::RotateError) -> Self {
        match value {
            stranger_domain::rotate::RotateError::RateLimited => ApiFailure::RateLimited,
            stranger_domain::rotate::RotateError::NoEligibleReaders => ApiFailure::NoReader,
            stranger_domain::rotate::RotateError::NotFound
            | stranger_domain::rotate::RotateError::CorruptHistory => ApiFailure::NotFound,
        }
    }
}

impl From<stranger_domain::tips::TipError> for ApiFailure {
    fn from(value: stranger_domain::tips::TipError) -> Self {
        match value {
            stranger_domain::tips::TipError::NotFound => ApiFailure::NotFound,
            stranger_domain::tips::TipError::PayoutsUnavailable => ApiFailure::PayoutsUnavailable,
            stranger_domain::tips::TipError::InvalidAmount => ApiFailure::Invalid,
        }
    }
}

impl From<stranger_domain::checkout::CheckoutError> for ApiFailure {
    fn from(value: stranger_domain::checkout::CheckoutError) -> Self {
        match value {
            stranger_domain::checkout::CheckoutError::LiveBlocked => ApiFailure::LiveBlocked,
            _ => ApiFailure::Invalid,
        }
    }
}

impl From<stranger_domain::webhook::WebhookError> for ApiFailure {
    fn from(value: stranger_domain::webhook::WebhookError) -> Self {
        match value {
            stranger_domain::webhook::WebhookError::Stale
            | stranger_domain::webhook::WebhookError::BadSignature
            | stranger_domain::webhook::WebhookError::MissingSignature
            | stranger_domain::webhook::WebhookError::MissingTimestamp => ApiFailure::Unauthorized,
            _ => ApiFailure::Invalid,
        }
    }
}

impl From<stranger_domain::reports::ReportError> for ApiFailure {
    fn from(_: stranger_domain::reports::ReportError) -> Self {
        ApiFailure::Invalid
    }
}

impl From<RateLimitError> for ApiFailure {
    fn from(_: RateLimitError) -> Self {
        ApiFailure::RateLimited
    }
}

impl From<store::StoreError> for ApiFailure {
    fn from(_: store::StoreError) -> Self {
        ApiFailure::Invalid
    }
}

#[cfg(test)]
mod tests {
    use super::checkout_session_id;
    use crate::store::{classify_tip_update, tip_paid_condition_matches, TipUpdateError};
    use stranger_domain::webhook::{disposition_after_claim, ClaimedWrite};

    #[test]
    fn builder_checkout_stores_the_session_mark_tip_paid_compares() {
        let session = serde_json::json!({
            "id": "cs_test_builder",
            "url": "https://checkout.stripe.com/c/pay/cs_test_builder"
        });
        let stored = checkout_session_id(&session).expect("session id");
        assert_eq!(stored, "cs_test_builder");
        assert!(tip_paid_condition_matches(&stored, "cs_test_builder"));
        assert!(!tip_paid_condition_matches(&stored, "cs_test_other"));
        assert!(checkout_session_id(&serde_json::json!({"id": ""})).is_none());

        let source = include_str!("lib.rs");
        let builder = source
            .split("path == \"/tips/builder\"")
            .nth(1)
            .expect("builder route");
        let body = builder.split("Err(ApiFailure::NotFound)").next().unwrap();
        assert!(
            body.contains("set_checkout_session"),
            "builder tips must store checkoutSessionId"
        );
    }

    #[test]
    fn webhook_keeps_the_claim_when_mark_tip_paid_conflicts() {
        let write = match classify_tip_update(true) {
            TipUpdateError::Conflict => ClaimedWrite::Conflict,
            TipUpdateError::Transient => ClaimedWrite::Transient,
        };
        let disposition = disposition_after_claim(write);
        assert_eq!(disposition.status, 200);
        assert!(!disposition.release_claim);
    }

    #[test]
    fn webhook_releases_the_claim_on_a_transient_tip_write() {
        let write = match classify_tip_update(false) {
            TipUpdateError::Conflict => ClaimedWrite::Conflict,
            TipUpdateError::Transient => ClaimedWrite::Transient,
        };
        let disposition = disposition_after_claim(write);
        assert_eq!(disposition.status, 503);
        assert!(disposition.release_claim);
    }
}

