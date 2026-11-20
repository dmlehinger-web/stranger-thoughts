//! Account preferences that are not an email address.
//!
//! Mailed-letter notes are on until the person turns them off. The stored
//! value is a boolean. The address itself stays in Cognito.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailAlertPreference {
    pub mail_alerts: bool,
}

/// Missing means the person has not opted out.
pub fn mail_alerts_enabled(stored: Option<bool>) -> bool {
    stored.unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alerts_stay_on_until_someone_opts_out() {
        assert!(mail_alerts_enabled(None));
        assert!(mail_alerts_enabled(Some(true)));
        assert!(!mail_alerts_enabled(Some(false)));
        let view = MailAlertPreference { mail_alerts: false };
        let value = serde_json::to_value(view).unwrap();
        assert!(crate::redaction::find_pii_key(&value).is_none(), "{value}");
        let raw = value.to_string();
        assert!(!raw.contains('@'));
        assert!(!raw.contains("email"));
    }
}
