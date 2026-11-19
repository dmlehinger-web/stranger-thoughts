use aws_sdk_cognitoidentityprovider::Client;
use serde_json::{json, Value};

use crate::aws::Aws;

/// Add the single group named by `custom:role` after the email is confirmed.
/// The event is returned unchanged and is not logged, because it carries the
/// email address.
pub async fn assign_group(event: Value) -> Result<Value, lambda_http::Error> {
    let trigger = event
        .get("triggerSource")
        .and_then(Value::as_str)
        .unwrap_or("");
    if trigger != "PostConfirmation_ConfirmSignUp" {
        return Ok(event);
    }
    let role = event
        .pointer("/request/userAttributes/custom:role")
        .and_then(Value::as_str)
        .unwrap_or("");
    let group = match role {
        "writer" => "writer",
        "reader" => "reader",
        _ => return Ok(event),
    };
    let pool = event.get("userPoolId").and_then(Value::as_str).unwrap_or("");
    let username = event.get("userName").and_then(Value::as_str).unwrap_or("");
    if pool.is_empty() || username.is_empty() {
        return Err("group assignment failed".into());
    }
    add_to_group(&Aws::get().await.map_err(|_| "group assignment failed")?.cognito, pool, username, group)
        .await?;
    Ok(event)
}

async fn add_to_group(
    client: &Client,
    pool: &str,
    username: &str,
    group: &str,
) -> Result<(), lambda_http::Error> {
    client
        .admin_add_user_to_group()
        .user_pool_id(pool)
        .username(username)
        .group_name(group)
        .send()
        .await
        .map_err(|_| -> lambda_http::Error { "group assignment failed".into() })?;
    Ok(())
}

/// Copy `email_verified` onto the access token and keep the email itself off
/// both tokens. Callers must not log the event.
pub fn shape_token(mut event: Value) -> Value {
    let verified = event
        .pointer("/request/userAttributes/email_verified")
        .and_then(Value::as_str)
        .unwrap_or("false");
    let claim = if verified == "true" { "true" } else { "false" };
    event["response"]["claimsAndScopeOverrideDetails"] = json!({
        "accessTokenGeneration": {
            "claimsToAddOrOverride": { "email_verified": claim },
            "claimsToSuppress": ["email"]
        },
        "idTokenGeneration": {
            "claimsToAddOrOverride": { "email_verified": claim },
            "claimsToSuppress": ["email"]
        }
    });
    if let Some(attributes) = event.pointer_mut("/request/userAttributes").and_then(Value::as_object_mut)
    {
        attributes.remove("email");
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_token_does_not_receive_the_email() {
        let event = json!({
            "request": {
                "userAttributes": {
                    "email": "person@example.com",
                    "email_verified": "true",
                    "custom:role": "reader"
                }
            },
            "response": {}
        });
        let shaped = shape_token(event);
        let access = &shaped["response"]["claimsAndScopeOverrideDetails"]["accessTokenGeneration"];
        assert_eq!(access["claimsToAddOrOverride"]["email_verified"], "true");
        assert!(access["claimsToAddOrOverride"].get("email").is_none());
        assert!(access["claimsToSuppress"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "email"));
        let rendered = shaped.to_string();
        assert!(!rendered.contains("person@example.com"));
    }
}
