//! Domain rules for Stranger Thoughts.
//!
//! Matching, authorization, envelope encryption, and tip routing live here so
//! they can be tested without AWS. Lambda handlers call these functions and
//! do not reimplement the rules.

pub mod account;
pub mod address;
pub mod authz;
pub mod checkout;
pub mod ids;
pub mod matching;
pub mod model;
pub mod rate_limit;
pub mod redaction;
pub mod reports;
pub mod rotate;
pub mod tips;
pub mod validation;
pub mod webhook;

/// Largest JSON body a handler will read. API Gateway's own cap is higher;
/// this is the application limit.
pub const MAX_JSON_BODY: usize = 8 * 1024;

/// Platform cut on a writer-to-reader tip, in cents.
///
/// Zero means the connected account receives the full charge. Stripe rejects
/// a non-positive application fee, so a zero fee is expressed by omitting
/// the field. This is a function, not a switch: live mode is a separate gate.
pub fn platform_fee_cents() -> u32 {
    0
}

pub const DEFAULT_MAX_ROTATES_PER_DAY: u32 = 3;
pub const DEFAULT_MAX_ADDRESS_READS_PER_DAY: u32 = 40;
pub const MIN_TIP_CENTS: u32 = 100;
pub const MAX_TIP_CENTS: u32 = 20_000;
pub const WEBHOOK_TOLERANCE_SECS: i64 = 300;

pub fn check_body_len(len: usize) -> Result<(), validation::ValidationError> {
    if len > MAX_JSON_BODY {
        Err(validation::ValidationError::BodyTooLarge)
    } else {
        Ok(())
    }
}
