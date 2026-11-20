use crate::tips::TipKind;
use crate::platform_fee_cents;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutPlan {
    pub tip_id: String,
    pub user_id: String,
    pub amount_cents: u32,
    pub kind: TipKind,
    pub destination_account: Option<String>,
    pub reader_id: Option<String>,
    pub match_id: Option<String>,
    pub success_url: String,
    pub cancel_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripeMode {
    Test,
    Live,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckoutError {
    #[error("bad redirect")]
    BadRedirect,
    #[error("missing destination")]
    MissingDestination,
    #[error("live mode blocked")]
    LiveBlocked,
    #[error("unusable key")]
    UnusableKey,
}

/// Live mode stays closed unless the environment value is exactly `true`.
pub fn live_gate_open(env_value: Option<&str>) -> bool {
    env_value == Some("true")
}

fn key_body_ok(rest: &str) -> bool {
    rest.len() >= 8 && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Accept a test secret or a test restricted key. Live keys of either shape
/// stay closed unless `allow_live` is exactly the environment gate.
pub fn enforce_stripe_mode(secret_key: &str, allow_live: bool) -> Result<StripeMode, CheckoutError> {
    let key = secret_key.trim();
    for prefix in ["rk_test_", "sk_test_"] {
        if let Some(rest) = key.strip_prefix(prefix) {
            if key_body_ok(rest) {
                return Ok(StripeMode::Test);
            }
            return Err(CheckoutError::UnusableKey);
        }
    }
    for prefix in ["rk_live_", "sk_live_"] {
        if let Some(rest) = key.strip_prefix(prefix) {
            if !key_body_ok(rest) {
                return Err(CheckoutError::UnusableKey);
            }
            if allow_live {
                return Ok(StripeMode::Live);
            }
            return Err(CheckoutError::LiveBlocked);
        }
    }
    Err(CheckoutError::UnusableKey)
}

/// Form fields for a hosted Checkout Session. No card, CVC, or PAN fields exist.
pub fn checkout_form(plan: &CheckoutPlan, app_base: &str) -> Result<Vec<(String, String)>, CheckoutError> {
    if !app_base.starts_with("https://") {
        return Err(CheckoutError::BadRedirect);
    }
    if !redirect_ok(app_base, &plan.success_url) || !redirect_ok(app_base, &plan.cancel_url) {
        return Err(CheckoutError::BadRedirect);
    }
    if plan.kind == TipKind::Reader {
        match plan.destination_account.as_deref() {
            Some(account) if account.starts_with("acct_") && account.len() > 5 => {}
            _ => return Err(CheckoutError::MissingDestination),
        }
    }
    let name = match plan.kind {
        TipKind::Reader => "Tip for a letter",
        TipKind::Builder => "Pay what you think this is worth",
    };
    let mut fields = vec![
        ("mode".into(), "payment".into()),
        ("success_url".into(), plan.success_url.clone()),
        ("cancel_url".into(), plan.cancel_url.clone()),
        ("client_reference_id".into(), plan.tip_id.clone()),
        ("line_items[0][quantity]".into(), "1".into()),
        ("line_items[0][price_data][currency]".into(), "usd".into()),
        (
            "line_items[0][price_data][unit_amount]".into(),
            plan.amount_cents.to_string(),
        ),
        ("line_items[0][price_data][product_data][name]".into(), name.into()),
        ("metadata[tip_id]".into(), plan.tip_id.clone()),
        ("metadata[user_id]".into(), plan.user_id.clone()),
        ("metadata[kind]".into(), plan.kind.as_str().into()),
    ];
    if let Some(match_id) = &plan.match_id {
        fields.push(("metadata[match_id]".into(), match_id.clone()));
    }
    if let Some(reader_id) = &plan.reader_id {
        fields.push(("metadata[reader_id]".into(), reader_id.clone()));
    }
    if plan.kind == TipKind::Reader {
        if let Some(destination) = &plan.destination_account {
            fields.push((
                "payment_intent_data[transfer_data][destination]".into(),
                destination.clone(),
            ));
        }
    }
    let fee = platform_fee_cents();
    if fee > 0 {
        fields.push((
            "payment_intent_data[application_fee_amount]".into(),
            fee.to_string(),
        ));
    }
    Ok(fields)
}

fn redirect_ok(app_base: &str, url: &str) -> bool {
    let base = app_base.trim_end_matches('/');
    url.starts_with("https://")
        && (url == base || url.starts_with(&format!("{base}/")))
        && !url.contains('\\')
        && !url.contains("@")
}

pub const STRIPE_CHECKOUT_URL: &str = "https://api.stripe.com/v1/checkout/sessions";
pub const STRIPE_ACCOUNT_URL: &str = "https://api.stripe.com/v1/accounts";
pub const STRIPE_ACCOUNT_LINK_URL: &str = "https://api.stripe.com/v1/account_links";

pub fn express_account_form(reader_id: &str, country: &str) -> Vec<(String, String)> {
    vec![
        ("type".into(), "express".into()),
        ("country".into(), country.into()),
        ("capabilities[transfers][requested]".into(), "true".into()),
        ("metadata[reader_id]".into(), reader_id.into()),
        ("business_type".into(), "individual".into()),
    ]
}

pub fn account_link_form(
    account_id: &str,
    refresh_url: &str,
    return_url: &str,
) -> Vec<(String, String)> {
    vec![
        ("account".into(), account_id.into()),
        ("refresh_url".into(), refresh_url.into()),
        ("return_url".into(), return_url.into()),
        ("type".into(), "account_onboarding".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(kind: TipKind, destination: Option<&str>) -> CheckoutPlan {
        CheckoutPlan {
            tip_id: "tip-1".into(),
            user_id: "writer-1".into(),
            amount_cents: 500,
            kind,
            destination_account: destination.map(str::to_string),
            reader_id: Some("reader-1".into()),
            match_id: Some("match-1".into()),
            success_url: "https://letters.example/write/home/".into(),
            cancel_url: "https://letters.example/write/home/".into(),
        }
    }

    #[test]
    fn reader_tip_uses_the_routed_account_and_charges_no_fee() {
        let fields = checkout_form(&plan(TipKind::Reader, Some("acct_cedar")), "https://letters.example").unwrap();
        let dest = fields
            .iter()
            .find(|(key, _)| key == "payment_intent_data[transfer_data][destination]")
            .unwrap();
        assert_eq!(dest.1, "acct_cedar");
        assert!(fields.iter().all(|(key, _)| key != "payment_intent_data[application_fee_amount]"));
        assert_eq!(platform_fee_cents(), 0);
        for banned in ["card", "cvc", "pan", "number"] {
            assert!(fields.iter().all(|(key, _)| !key.contains(banned)));
        }
    }

    #[test]
    fn builder_tip_has_no_destination() {
        let mut plan = plan(TipKind::Builder, None);
        plan.reader_id = None;
        plan.match_id = None;
        let fields = checkout_form(&plan, "https://letters.example").unwrap();
        assert!(fields.iter().all(|(key, _)| !key.contains("destination")));
        assert!(fields.iter().any(|(key, value)| key == "line_items[0][price_data][product_data][name]" && value.contains("worth")));
    }

    #[test]
    fn redirects_must_stay_on_the_app_origin() {
        let mut plan = plan(TipKind::Reader, Some("acct_cedar"));
        plan.success_url = "https://evil.example/phish".into();
        assert_eq!(
            checkout_form(&plan, "https://letters.example"),
            Err(CheckoutError::BadRedirect)
        );
    }

    #[test]
    fn live_keys_are_blocked_unless_the_gate_is_exactly_true() {
        assert_eq!(
            enforce_stripe_mode("sk_test_12345678", false).unwrap(),
            StripeMode::Test
        );
        assert_eq!(
            enforce_stripe_mode("sk_live_12345678", false),
            Err(CheckoutError::LiveBlocked)
        );
        assert_eq!(
            enforce_stripe_mode("sk_live_12345678", true).unwrap(),
            StripeMode::Live
        );
        assert!(!live_gate_open(Some("TRUE")));
        assert!(!live_gate_open(Some("1")));
        assert!(!live_gate_open(None));
        assert!(live_gate_open(Some("true")));
        assert!(enforce_stripe_mode("pk_test_12345678", false).is_err());
        assert_eq!(
            enforce_stripe_mode("rk_test_12345678", false).unwrap(),
            StripeMode::Test
        );
        assert_eq!(
            enforce_stripe_mode("rk_live_12345678", false),
            Err(CheckoutError::LiveBlocked)
        );
        assert_eq!(
            enforce_stripe_mode("rk_live_12345678", true).unwrap(),
            StripeMode::Live
        );
    }
}
