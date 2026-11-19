use std::collections::HashMap;

use lambda_http::request::RequestContext;
use lambda_http::{Body, Request, RequestExt, Response};
use serde::Serialize;
use stranger_domain::authz::{self, Principal};

#[derive(Debug)]
pub enum ApiFailure {
    Unauthorized,
    Forbidden,
    EmailUnverified,
    NotFound,
    Invalid,
    ChildAge,
    BodyTooLarge,
    RateLimited,
    NoReader,
    PayoutsUnavailable,
    LiveBlocked,
}

pub fn json_response(status: u16, body: &impl Serialize) -> Response<Body> {
    let payload = serde_json::to_string(body).unwrap_or_else(|_| "{\"error\":\"encode\"}".to_string());
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(payload))
        .unwrap_or_else(|_| Response::new(Body::from("{\"error\":\"encode\"}")))
}

pub fn failure(error: &ApiFailure) -> Response<Body> {
    let (status, code) = match error {
        ApiFailure::Unauthorized => (401, "unauthorized"),
        ApiFailure::Forbidden => (403, "forbidden"),
        ApiFailure::EmailUnverified => (403, "email_unverified"),
        ApiFailure::NotFound => (404, "not_found"),
        ApiFailure::Invalid => (400, "invalid_request"),
        ApiFailure::ChildAge => (400, "child_age_rejected"),
        ApiFailure::BodyTooLarge => (413, "body_too_large"),
        ApiFailure::RateLimited => (429, "rate_limited"),
        ApiFailure::NoReader => (409, "no_reader"),
        ApiFailure::PayoutsUnavailable => (409, "payouts_unavailable"),
        ApiFailure::LiveBlocked => (403, "live_mode_blocked"),
    };
    json_response(status, &serde_json::json!({"error": code}))
}

pub fn body_bytes(body: &Body) -> &[u8] {
    match body {
        Body::Empty => &[],
        Body::Text(text) => text.as_bytes(),
        Body::Binary(bytes) => bytes,
        _ => &[],
    }
}

pub fn principal(request: &Request) -> Result<Principal, ApiFailure> {
    let claims = jwt_claims(request).ok_or(ApiFailure::Unauthorized)?;
    authz::principal_from_claims(&claims).map_err(ApiFailure::from)
}

pub fn claim(request: &Request, name: &str) -> Option<String> {
    jwt_claims(request)?.get(name).cloned()
}

fn jwt_claims(request: &Request) -> Option<HashMap<String, String>> {
    match request.request_context_ref()? {
        RequestContext::ApiGatewayV2(context) => context
            .authorizer
            .as_ref()
            .and_then(|authorizer| authorizer.jwt.as_ref())
            .map(|jwt| jwt.claims.clone()),
        _ => None,
    }
}

impl From<authz::AuthError> for ApiFailure {
    fn from(value: authz::AuthError) -> Self {
        match value {
            authz::AuthError::Unauthorized => ApiFailure::Unauthorized,
            authz::AuthError::Forbidden => ApiFailure::Forbidden,
        }
    }
}
