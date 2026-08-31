//! Normalized terminal outcome contract for execution attempts.
//!
//! Live adapters classify their real failure surface into a stable enum plus
//! safe structured fields (plan §9/§14). Raw provider text is never a durable
//! classifier when a stable provider code exists; where a real provider signal
//! is missing, adapters map to `Unknown` rather than guessing. `safe_message`
//! is always a fixed cdesktop-owned string, never raw provider output, so the
//! contract is redaction-safe by construction.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum ExecutionOutcomeClass {
    QuotaExhausted,
    AuthExpired,
    AuthInvalid,
    ModelUnavailable,
    RateLimitedTransient,
    NetworkTransient,
    UserStopped,
    TaskFailed,
    Unknown,
}

/// Scope a failure binds to: `account` failures cool one auth binding,
/// `route` failures affect a provider/model route, `task` failures are
/// terminal for the logical command regardless of routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum OutcomeBindingScope {
    Account,
    Route,
    Task,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct NormalizedExecutionOutcome {
    pub class: ExecutionOutcomeClass,
    /// Stable provider error code (e.g. `usage_limit_exceeded`), never raw
    /// provider message text.
    #[serde(default)]
    #[ts(optional)]
    pub provider_code: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub retry_after_seconds: Option<i64>,
    #[serde(default)]
    #[ts(optional)]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    #[ts(optional)]
    pub binding_scope: Option<OutcomeBindingScope>,
    /// Fixed, cdesktop-owned description. Never contains provider text,
    /// credentials, headers, or account identifiers.
    pub safe_message: String,
}

impl NormalizedExecutionOutcome {
    pub fn new(class: ExecutionOutcomeClass) -> Self {
        let (binding_scope, safe_message) = match class {
            ExecutionOutcomeClass::QuotaExhausted => (
                Some(OutcomeBindingScope::Account),
                "Subscription quota exhausted",
            ),
            ExecutionOutcomeClass::AuthExpired => (
                Some(OutcomeBindingScope::Account),
                "Authentication expired; reauthentication required",
            ),
            ExecutionOutcomeClass::AuthInvalid => (
                Some(OutcomeBindingScope::Account),
                "Authentication rejected",
            ),
            ExecutionOutcomeClass::ModelUnavailable => (
                Some(OutcomeBindingScope::Route),
                "Requested model unavailable",
            ),
            ExecutionOutcomeClass::RateLimitedTransient => (
                Some(OutcomeBindingScope::Account),
                "Rate limited; retry later",
            ),
            ExecutionOutcomeClass::NetworkTransient => (
                Some(OutcomeBindingScope::Route),
                "Transient provider or network failure",
            ),
            ExecutionOutcomeClass::UserStopped => {
                (Some(OutcomeBindingScope::Task), "Stopped by user")
            }
            ExecutionOutcomeClass::TaskFailed => (Some(OutcomeBindingScope::Task), "Task failed"),
            ExecutionOutcomeClass::Unknown => (None, "Unclassified failure"),
        };
        Self {
            class,
            provider_code: None,
            retry_after_seconds: None,
            resets_at: None,
            binding_scope,
            safe_message: safe_message.to_string(),
        }
    }

    pub fn with_provider_code(mut self, code: impl Into<String>) -> Self {
        self.provider_code = Some(code.into());
        self
    }

    pub fn with_retry_after_seconds(mut self, seconds: i64) -> Self {
        self.retry_after_seconds = Some(seconds);
        self
    }

    pub fn with_resets_at(mut self, resets_at: DateTime<Utc>) -> Self {
        self.resets_at = Some(resets_at);
        self
    }

    /// Classify an HTTP status observed on a provider transport failure.
    /// Only stable, header-independent facts are preserved.
    pub fn from_http_status(status: Option<u16>) -> Self {
        match status {
            Some(401) => {
                Self::new(ExecutionOutcomeClass::AuthExpired).with_provider_code("http_401")
            }
            Some(403) => {
                Self::new(ExecutionOutcomeClass::AuthInvalid).with_provider_code("http_403")
            }
            Some(404) => {
                Self::new(ExecutionOutcomeClass::ModelUnavailable).with_provider_code("http_404")
            }
            Some(429) => Self::new(ExecutionOutcomeClass::RateLimitedTransient)
                .with_provider_code("http_429"),
            Some(code) if (500..=599).contains(&code) => {
                Self::new(ExecutionOutcomeClass::NetworkTransient)
                    .with_provider_code(format!("http_{code}"))
            }
            _ => Self::new(ExecutionOutcomeClass::NetworkTransient),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_message_is_fixed_per_class() {
        let outcome = NormalizedExecutionOutcome::new(ExecutionOutcomeClass::QuotaExhausted);
        assert_eq!(outcome.safe_message, "Subscription quota exhausted");
        assert_eq!(outcome.binding_scope, Some(OutcomeBindingScope::Account));
    }

    #[test]
    fn http_status_maps_to_stable_codes() {
        let unauthorized = NormalizedExecutionOutcome::from_http_status(Some(401));
        assert_eq!(unauthorized.class, ExecutionOutcomeClass::AuthExpired);
        assert_eq!(unauthorized.provider_code.as_deref(), Some("http_401"));

        let throttled = NormalizedExecutionOutcome::from_http_status(Some(429));
        assert_eq!(throttled.class, ExecutionOutcomeClass::RateLimitedTransient);

        let unavailable = NormalizedExecutionOutcome::from_http_status(Some(503));
        assert_eq!(unavailable.class, ExecutionOutcomeClass::NetworkTransient);
        assert_eq!(unavailable.provider_code.as_deref(), Some("http_503"));

        let unknown_transport = NormalizedExecutionOutcome::from_http_status(None);
        assert_eq!(
            unknown_transport.class,
            ExecutionOutcomeClass::NetworkTransient
        );
        assert_eq!(unknown_transport.provider_code, None);
    }

    #[test]
    fn serializes_with_snake_case_class() {
        let outcome = NormalizedExecutionOutcome::new(ExecutionOutcomeClass::RateLimitedTransient)
            .with_retry_after_seconds(30);
        let value = serde_json::to_value(&outcome).unwrap();
        assert_eq!(value["class"], "rate_limited_transient");
        assert_eq!(value["retry_after_seconds"], 30);
        assert_eq!(value["binding_scope"], "account");
    }
}
