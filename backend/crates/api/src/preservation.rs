//! Authenticated opt-in copies. There is no public read of this module.

use chrono::Utc;
use lambda_http::{Body, Request, RequestExt, Response};
use serde_json::json;
use stranger_domain::authz::{self, Role};
use stranger_domain::ids::{MatchId, ReaderId, WriterId};
use stranger_domain::model::MatchRecord;
use stranger_domain::preservation::{self, ArchiveKind, ArchiveMineView, PrivateCopyView};
use stranger_domain::validation::PreservationRequest;
use stranger_domain::DEFAULT_MAX_MAIL_MARKS_PER_DAY;

use crate::http::{self, failure, json_response, ApiFailure};
use crate::items::attr_s;
use crate::{noted, parse_json, store};

pub async fn preservation(request: Request) -> Result<Response<Body>, lambda_http::Error> {
    let path = request.uri().path().to_string();
    let response = match preservation_inner(&request).await {
        Ok(response) => response,
        Err(error) => failure(&error),
    };
    Ok(noted(&path, response))
}

async fn preservation_inner(request: &Request) -> Result<Response<Body>, ApiFailure> {
    let principal = http::principal(request)?;
    authz::require_verified(&principal)?;
    let store = store().await?;
    let method = request.method().as_str();
    let path = request.uri().path();
    let params = request.query_string_parameters();
    let limit = preservation::page_limit(params.first("limit"))?;
    let cursor = preservation::page_cursor(params.first("cursor"))?;

    if method == "GET" && path == "/preservation/private" {
        let (rows, next) = store
            .list_owned(&principal.user_id, "copy#", limit, cursor.as_deref())
            .await?;
        let items = rows
            .iter()
            .filter_map(private_view)
            .collect::<Vec<_>>();
        return Ok(json_response(
            200,
            &json!({"items": items, "next_cursor": next}),
        ));
    }
    if method == "GET" && path == "/preservation/mine" {
        let (rows, next) = store
            .list_owned(&principal.user_id, "archive#", limit, cursor.as_deref())
            .await?;
        let mut items = Vec::new();
        for row in &rows {
            if let Some(view) = archive_view(&store, row).await? {
                items.push(view);
            }
        }
        return Ok(json_response(
            200,
            &json!({"items": items, "next_cursor": next}),
        ));
    }
    if method == "DELETE" {
        if let Some(match_id) = path.strip_prefix("/preservation/private/") {
            let match_id = MatchId::new(match_id).map_err(|_| ApiFailure::NotFound)?;
            confirm_party(&store, &principal, &match_id).await?;
            store
                .delete_owned_sk(&principal.user_id, &format!("copy#{}", match_id.as_str()))
                .await?;
            return Ok(json_response(200, &json!({"deleted": true})));
        }
    }
    if method == "POST" && (path == "/preservation/private" || path == "/preservation/archive") {
        let body: PreservationRequest = parse_json(request)?;
        let text = preservation::screen_preserved_text(&body.body)?;
        let match_id = MatchId::new(body.match_id).map_err(|_| ApiFailure::NotFound)?;
        let record = confirm_party(&store, &principal, &match_id).await?;
        let subject = stranger_domain::rate_limit::hash_subject("preserve", &principal.user_id);
        store
            .bump_limit(
                &subject,
                Utc::now(),
                DEFAULT_MAX_MAIL_MARKS_PER_DAY,
                chrono::Duration::hours(24),
            )
            .await?;
        let now = Utc::now();
        if path.ends_with("/private") {
            store
                .save_private_copy(principal.user_id.as_str(), match_id.as_str(), &text, now)
                .await?;
            return Ok(json_response(200, &json!({"saved": true, "published": false})));
        }
        let kind = match principal.role {
            Role::Writer => ArchiveKind::Thought,
            Role::Reader => ArchiveKind::Reflection,
        };
        store
            .save_archive_submission(principal.user_id.as_str(), match_id.as_str(), kind, &text, now)
            .await?;
        let other_id = match principal.role {
            Role::Writer => record.reader_id.as_str(),
            Role::Reader => record.writer_id.as_str(),
        };
        if let Some(other) = store.archive_body(other_id, match_id.as_str()).await? {
            let (thought, reflection) = match principal.role {
                Role::Writer => (text, other),
                Role::Reader => (other, text),
            };
            store
                .queue_pair(
                    match_id.as_str(),
                    &thought,
                    &reflection,
                    record.writer_id.as_str(),
                    record.reader_id.as_str(),
                    now,
                )
                .await?;
        }
        return Ok(json_response(
            200,
            &json!({"saved": true, "status": "pending", "curated": false, "published": false}),
        ));
    }
    Err(ApiFailure::NotFound)
}

async fn confirm_party(
    store: &crate::store::Store,
    principal: &stranger_domain::authz::Principal,
    match_id: &MatchId,
) -> Result<MatchRecord, ApiFailure> {
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
    history
        .into_iter()
        .find(|record| &record.match_id == match_id)
        .ok_or(ApiFailure::NotFound)
}

fn private_view(item: &std::collections::HashMap<String, aws_sdk_dynamodb::types::AttributeValue>) -> Option<PrivateCopyView> {
    let match_id = attr_s(item, "matchId")?;
    let body = attr_s(item, "body")?;
    let saved = attr_s(item, "savedAt")?;
    let saved_at = chrono::DateTime::parse_from_rfc3339(&saved).ok()?.with_timezone(&Utc);
    Some(PrivateCopyView {
        copy_id: match_id.clone(),
        match_id,
        body,
        saved_at,
    })
}

async fn archive_view(
    store: &crate::store::Store,
    item: &std::collections::HashMap<String, aws_sdk_dynamodb::types::AttributeValue>,
) -> Result<Option<ArchiveMineView>, ApiFailure> {
    let Some(match_id) = attr_s(item, "matchId") else {
        return Ok(None);
    };
    let kind = match attr_s(item, "kind").as_deref() {
        Some("thought") => ArchiveKind::Thought,
        Some("reflection") => ArchiveKind::Reflection,
        _ => return Ok(None),
    };
    let paired = store.pair_exists(&match_id).await?;
    Ok(Some(preservation::archive_mine_view(&match_id, kind, paired)))
}
