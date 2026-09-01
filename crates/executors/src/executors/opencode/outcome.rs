//! Classify OpenCode's failure surface into the normalized outcome contract.
//!
//! `session.error` carries a closed union (opencode 1.15.10 `EventSessionError`):
//! `ProviderAuthError`, `UnknownError`, `MessageOutputLengthError`,
//! `MessageAbortedError`, `StructuredOutputError`, `ContextOverflowError`,
//! `APIError`. Classification reads only `name` and `APIError.data.statusCode`,
//! both stable structured fields. Provider message text never classifies, so an
//! error OpenCode itself could not name stays `Unknown` rather than guessed.
//!
//! Fixtures backing the tests are verbatim live-server captures; see
//! `fixtures/README.md`, which also documents why there is no rate-limit branch.

use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::outcome::{ExecutionOutcomeClass, NormalizedExecutionOutcome};

/// Terminal outcome observed while a session runs, read by the spawn task when
/// the run ends in failure.
///
/// The first stable signal wins: later events are consequences of it (an auth
/// failure ends the turn, and ending the turn reports an abort), so keeping the
/// earliest keeps the reported cause the actual cause.
#[derive(Clone, Default)]
pub(super) struct OutcomeSink(Arc<Mutex<Option<NormalizedExecutionOutcome>>>);

impl OutcomeSink {
    pub(super) fn record(&self, outcome: NormalizedExecutionOutcome) {
        let mut slot = self.0.lock().expect("outcome sink poisoned");
        if slot.is_none() {
            *slot = Some(outcome);
        }
    }

    pub(super) fn take(&self) -> Option<NormalizedExecutionOutcome> {
        self.0.lock().expect("outcome sink poisoned").take()
    }
}

/// Classify the `error` object of a `session.error` event.
pub(super) fn normalized_session_failure(error: Option<&Value>) -> NormalizedExecutionOutcome {
    let Some(error) = error else {
        return NormalizedExecutionOutcome::new(ExecutionOutcomeClass::Unknown);
    };

    let classified =
        |class, code: &str| NormalizedExecutionOutcome::new(class).with_provider_code(code);

    match error
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        // The AI SDK reports the upstream provider status verbatim. When the
        // request never reached the provider there is no status, and
        // `from_http_status(None)` is already the transport-failure mapping.
        "APIError" => NormalizedExecutionOutcome::from_http_status(
            error
                .pointer("/data/statusCode")
                .and_then(Value::as_u64)
                .and_then(|status| u16::try_from(status).ok()),
        ),
        "ProviderAuthError" => {
            classified(ExecutionOutcomeClass::AuthInvalid, "provider_auth_error")
        }
        "MessageAbortedError" => classified(ExecutionOutcomeClass::UserStopped, "message_aborted"),
        "ContextOverflowError" => classified(ExecutionOutcomeClass::TaskFailed, "context_overflow"),
        "MessageOutputLengthError" => {
            classified(ExecutionOutcomeClass::TaskFailed, "message_output_length")
        }
        "StructuredOutputError" => {
            classified(ExecutionOutcomeClass::TaskFailed, "structured_output")
        }
        _ => NormalizedExecutionOutcome::new(ExecutionOutcomeClass::Unknown),
    }
}

/// The OpenCode server never reached the point of announcing its listening URL.
/// Nothing about the turn was rejected, so this is retryable like any other
/// failure to reach the route.
pub(super) fn startup_failure() -> NormalizedExecutionOutcome {
    NormalizedExecutionOutcome::new(ExecutionOutcomeClass::NetworkTransient)
        .with_provider_code("opencode_server_start_failed")
}

/// The OpenCode server became unreachable mid-turn (event stream dropped, health
/// lost, process died). Route-scoped and retryable; the turn itself is intact.
pub(super) fn transport_failure() -> NormalizedExecutionOutcome {
    NormalizedExecutionOutcome::new(ExecutionOutcomeClass::NetworkTransient)
        .with_provider_code("opencode_server_unreachable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::OutcomeBindingScope;

    fn fixture(name: &str) -> Value {
        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/executors/opencode/fixtures")
                .join(name),
        )
        .expect("fixture readable");
        serde_json::from_str(&raw).expect("fixture is valid JSON")
    }

    #[test]
    fn invalid_provider_key_is_account_scoped_auth_failure() {
        let outcome = normalized_session_failure(Some(&fixture("session_error_api_401.json")));

        assert_eq!(outcome.class, ExecutionOutcomeClass::AuthExpired);
        assert_eq!(outcome.provider_code.as_deref(), Some("http_401"));
        assert_eq!(outcome.binding_scope, Some(OutcomeBindingScope::Account));
    }

    #[test]
    fn aborted_turn_is_user_stopped() {
        let outcome = normalized_session_failure(Some(&fixture("session_error_aborted.json")));

        assert_eq!(outcome.class, ExecutionOutcomeClass::UserStopped);
        assert_eq!(outcome.provider_code.as_deref(), Some("message_aborted"));
        assert_eq!(outcome.binding_scope, Some(OutcomeBindingScope::Task));
    }

    #[test]
    fn error_opencode_could_not_name_stays_unknown() {
        // OpenCode reports an unresolvable provider/model as `UnknownError` and
        // puts the detail in prose. Guessing `ModelUnavailable` off that text is
        // exactly what the contract forbids.
        let outcome = normalized_session_failure(Some(&fixture("session_error_unknown.json")));

        assert_eq!(outcome.class, ExecutionOutcomeClass::Unknown);
        assert_eq!(outcome.provider_code, None);
        assert_eq!(outcome.binding_scope, None);
    }

    #[test]
    fn provider_status_drives_the_remaining_classes() {
        // Same `APIError` envelope as the captured 401, walked across the
        // statuses OpenCode forwards verbatim from the provider.
        let with_status = |status: u64| {
            let mut error = fixture("session_error_api_401.json");
            error["data"]["statusCode"] = status.into();
            normalized_session_failure(Some(&error))
        };

        assert_eq!(
            with_status(429).class,
            ExecutionOutcomeClass::RateLimitedTransient
        );
        assert_eq!(
            with_status(503).class,
            ExecutionOutcomeClass::NetworkTransient
        );
        assert_eq!(
            with_status(404).class,
            ExecutionOutcomeClass::ModelUnavailable
        );
    }

    #[test]
    fn api_error_without_a_status_never_reached_the_provider() {
        let mut error = fixture("session_error_api_401.json");
        error["data"]
            .as_object_mut()
            .expect("data object")
            .remove("statusCode");

        let outcome = normalized_session_failure(Some(&error));

        assert_eq!(outcome.class, ExecutionOutcomeClass::NetworkTransient);
    }

    #[test]
    fn no_classification_carries_provider_text() {
        // Every fixture holds provider prose; none of it may reach the contract.
        for name in [
            "session_error_api_401.json",
            "session_error_aborted.json",
            "session_error_unknown.json",
        ] {
            let outcome = normalized_session_failure(Some(&fixture(name)));
            let rendered = serde_json::to_string(&outcome).expect("serializable");

            assert!(!rendered.contains("API key"), "{name} leaked provider text");
            assert!(!rendered.contains("Aborted"), "{name} leaked provider text");
            assert!(
                !rendered.contains("Model not found"),
                "{name} leaked provider text"
            );
        }
    }

    #[test]
    fn missing_error_object_is_unclassified() {
        assert_eq!(
            normalized_session_failure(None).class,
            ExecutionOutcomeClass::Unknown
        );
    }

    #[test]
    fn sink_keeps_the_first_signal_not_the_last() {
        let sink = OutcomeSink::default();
        sink.record(normalized_session_failure(Some(&fixture(
            "session_error_api_401.json",
        ))));
        // The turn ends, and ending it reports an abort. The auth failure is
        // still the cause.
        sink.record(normalized_session_failure(Some(&fixture(
            "session_error_aborted.json",
        ))));

        assert_eq!(
            sink.take().map(|outcome| outcome.class),
            Some(ExecutionOutcomeClass::AuthExpired)
        );
    }

    #[test]
    fn lost_server_is_route_scoped_and_transient() {
        let outcome = transport_failure();

        assert_eq!(outcome.class, ExecutionOutcomeClass::NetworkTransient);
        assert_eq!(outcome.binding_scope, Some(OutcomeBindingScope::Route));
    }
}
