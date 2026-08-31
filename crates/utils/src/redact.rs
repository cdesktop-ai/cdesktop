//! Secret redaction primitives.
//!
//! Resolved credentials exist only in memory between binding resolution and
//! executor spawn. These helpers make accidental exposure a type error rather
//! than a review problem: `Redacted<T>` never prints or serializes its inner
//! value, and `redact_text` scrubs known secret material out of free-form
//! text (error messages, log lines) before it leaves the launch path.

use std::fmt;

pub const REDACTED_PLACEHOLDER: &str = "[REDACTED]";

/// Wrapper whose `Debug`/`Display` always print `[REDACTED]` and which
/// intentionally implements neither `Serialize` nor `Deserialize`. The inner
/// value is reachable only through [`Redacted::expose`], keeping every
/// accidental formatting or serialization path safe by construction.
#[derive(Clone, PartialEq, Eq)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Deliberately explicit accessor: call sites name the exposure.
    pub fn expose(&self) -> &T {
        &self.0
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED_PLACEHOLDER)
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED_PLACEHOLDER)
    }
}

/// Replace every occurrence of each secret in `text` with `[REDACTED]`.
/// Empty or whitespace-only secrets are ignored so a blank credential can
/// never turn the whole string into placeholder noise.
pub fn redact_text<'a>(text: &str, secrets: impl IntoIterator<Item = &'a str>) -> String {
    let mut redacted = text.to_string();
    for secret in secrets {
        if secret.trim().is_empty() {
            continue;
        }
        redacted = redacted.replace(secret, REDACTED_PLACEHOLDER);
    }
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_never_prints_inner_value() {
        let secret = Redacted::new("sk-live-1234".to_string());
        assert_eq!(format!("{secret:?}"), REDACTED_PLACEHOLDER);
        assert_eq!(format!("{secret}"), REDACTED_PLACEHOLDER);
        assert_eq!(secret.expose(), "sk-live-1234");
    }

    #[test]
    fn redact_text_scrubs_every_occurrence() {
        let message = "auth failed for key sk-live-1234 (sk-live-1234 rejected)";
        let scrubbed = redact_text(message, ["sk-live-1234"]);
        assert_eq!(
            scrubbed,
            "auth failed for key [REDACTED] ([REDACTED] rejected)"
        );
        assert!(!scrubbed.contains("sk-live-1234"));
    }

    #[test]
    fn redact_text_ignores_blank_secrets() {
        let message = "nothing to hide";
        assert_eq!(redact_text(message, ["", "  "]), message);
    }
}
