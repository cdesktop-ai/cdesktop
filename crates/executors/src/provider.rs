//! The surface a provider record and a harness adapter share.
//!
//! The db layer owns the record: storage, credential precedence, which slot
//! belongs to which harness. Adapters own every harness convention: env var
//! names, config shape, model-id form. Neither side names the other, so
//! adding a harness is an adapter module plus profile registration.

use std::{any::Any, collections::HashMap, fmt, sync::Arc};

use serde_json::{Map, Value as JsonValue};
use thiserror::Error;

use crate::executors::BaseCodingAgent;

/// Failures an adapter can report while building its injection.
///
/// Messages match the record-validation errors the db layer raises at save
/// time: the same misconfiguration reads the same either side of a spawn.
#[derive(Debug, Error)]
pub enum ProviderInjectionError {
    #[error("agent {0} is enabled but apiKey is empty")]
    MissingApiKey(BaseCodingAgent),
    #[error("agent {0} is enabled but its baseUrl is empty")]
    MissingBaseUrl(BaseCodingAgent),
    #[error("enabledModels must not be empty")]
    EmptyEnabledModels,
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// One harness's payload slot on a provider record, flattened.
///
/// `base_url` / `api_key` / `env` are the fields every slot carries. Anything
/// else a harness stores stays in `extras` under its own wire key, where only
/// the adapter that declared the slot looks for it.
#[derive(Clone, Default)]
pub struct ProviderPayload {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub env: HashMap<String, String>,
    extras: Map<String, JsonValue>,
}

impl ProviderPayload {
    /// Flatten a slot as the record stores it (camelCase JSON). A missing or
    /// non-object slot flattens to the empty payload — a harness with nothing
    /// configured, which is exactly what an unset slot means.
    ///
    /// Empty strings collapse to `None`: a cleared form field and an unset one
    /// are the same misconfiguration, and only one of them should need a check.
    pub fn from_slot(slot: Option<&JsonValue>) -> Self {
        let Some(JsonValue::Object(slot)) = slot else {
            return Self::default();
        };
        let mut extras = slot.clone();
        let mut take_str = |key: &str| match extras.remove(key) {
            Some(JsonValue::String(s)) if !s.is_empty() => Some(s),
            _ => None,
        };
        let base_url = take_str("baseUrl");
        let api_key = take_str("apiKey");
        let env = extras
            .remove("env")
            .and_then(|env| serde_json::from_value(env).ok())
            .unwrap_or_default();
        Self {
            base_url,
            api_key,
            env,
            extras,
        }
    }

    /// A harness-specific string field, empty treated as unset.
    pub fn extra_str(&self, key: &str) -> Option<&str> {
        match self.extras.get(key) {
            Some(JsonValue::String(s)) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    /// A harness-specific object field, absent treated as empty.
    pub fn extra_object(&self, key: &str) -> Map<String, JsonValue> {
        match self.extras.get(key) {
            Some(JsonValue::Object(map)) => map.clone(),
            _ => Map::new(),
        }
    }
}

/// Everything an adapter may read off the provider record the user picked.
#[derive(Clone, Default)]
pub struct ProviderContext {
    /// Ambient-auth record (cdesktop's Default provider). The harness reads
    /// its own config and credentials, so no injection is built at all.
    pub ambient: bool,
    /// Display name of the record, for harnesses that label their providers.
    pub record_name: String,
    /// Stable record slug: the catalog preset id, or `"custom"`.
    pub slug: String,
    /// Credential, already resolved through the slot override then the
    /// record-level key.
    pub api_key: Option<String>,
    /// This harness's slot on the record.
    pub payload: ProviderPayload,
    /// Model ids the user enabled on the record.
    pub enabled_models: Vec<String>,
    /// The model the picker selected.
    pub model_id: String,
}

impl ProviderContext {
    pub fn require_api_key(&self, agent: BaseCodingAgent) -> Result<&str, ProviderInjectionError> {
        self.api_key
            .as_deref()
            .ok_or(ProviderInjectionError::MissingApiKey(agent))
    }

    pub fn require_base_url(&self, agent: BaseCodingAgent) -> Result<&str, ProviderInjectionError> {
        self.payload
            .base_url
            .as_deref()
            .ok_or(ProviderInjectionError::MissingBaseUrl(agent))
    }
}

/// Manual `Debug` so a logged context can never print a resolved credential.
impl fmt::Debug for ProviderContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderContext")
            .field("ambient", &self.ambient)
            .field("record_name", &self.record_name)
            .field("slug", &self.slug)
            .field("model_id", &self.model_id)
            .finish_non_exhaustive()
    }
}

/// A structured spawn payload owned by exactly one harness.
///
/// The shared spawn env carries it without knowing its shape, and only the
/// owning adapter can read it back — under its own harness key, as its own
/// type. A harness cannot be handed another's payload by mistake.
#[derive(Clone)]
pub struct StructuredInjection {
    owner: BaseCodingAgent,
    value: Arc<dyn Any + Send + Sync>,
}

impl StructuredInjection {
    pub fn new<T: Any + Send + Sync>(owner: BaseCodingAgent, value: T) -> Self {
        Self {
            owner,
            value: Arc::new(value),
        }
    }

    pub fn get<T: Any + Send + Sync>(&self, owner: BaseCodingAgent) -> Option<&T> {
        (self.owner == owner)
            .then(|| self.value.downcast_ref::<T>())
            .flatten()
    }
}

impl fmt::Debug for StructuredInjection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StructuredInjection")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

/// What an adapter contributes to a spawn for the selected provider record.
#[derive(Clone, Default)]
pub struct ProviderInjection {
    /// Process env merged into `ExecutionEnv::provider_vars` at spawn.
    pub env: Option<HashMap<String, String>>,
    /// Anything that does not fit an env var, owned by the emitting harness.
    pub structured: Option<StructuredInjection>,
}

impl ProviderInjection {
    /// Env-only injection; an empty map means "nothing to add".
    pub fn from_env(env: HashMap<String, String>) -> Self {
        Self {
            env: (!env.is_empty()).then_some(env),
            structured: None,
        }
    }

    pub fn with_structured<T: Any + Send + Sync>(
        mut self,
        owner: BaseCodingAgent,
        value: T,
    ) -> Self {
        self.structured = Some(StructuredInjection::new(owner, value));
        self
    }
}

/// Manual `Debug` so a logged injection can never print resolved credential
/// values: env var names stay visible, values do not.
impl fmt::Debug for ProviderInjection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderInjection")
            .field(
                "env",
                &self
                    .env
                    .as_ref()
                    .map(|env| env.keys().cloned().collect::<Vec<_>>()),
            )
            .field("structured", &self.structured)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::{
        env::ExecutionEnv,
        executors::{ExecutorError, SpawnedChild, StandardCodingAgentExecutor},
        profile::ExecutorConfig,
    };

    /// A harness that exists only here — nothing outside this module, and in
    /// particular nothing in the db crate, knows it exists.
    ///
    /// That it compiles is the point: a new harness supplies an adapter and
    /// gets a working provider slot, model-id form and injection from the
    /// trait defaults, with no record-side change to route it.
    struct FourthHarness;

    #[async_trait]
    impl StandardCodingAgentExecutor for FourthHarness {
        async fn spawn(
            &self,
            _current_dir: &Path,
            _prompt: &str,
            _env: &ExecutionEnv,
        ) -> Result<SpawnedChild, ExecutorError> {
            unimplemented!("not spawned in tests")
        }

        async fn spawn_follow_up(
            &self,
            _current_dir: &Path,
            _prompt: &str,
            _session_id: &str,
            _reset_to_message_id: Option<&str>,
            _env: &ExecutionEnv,
        ) -> Result<SpawnedChild, ExecutorError> {
            unimplemented!("not spawned in tests")
        }

        fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
            None
        }

        fn get_preset_options(&self) -> ExecutorConfig {
            unimplemented!("not configured in tests")
        }
    }

    fn context() -> ProviderContext {
        ProviderContext {
            record_name: "Test Provider".to_string(),
            slug: "openrouter".to_string(),
            api_key: Some("sk-real".to_string()),
            payload: ProviderPayload::from_slot(Some(&json!({
                "baseUrl": "https://openrouter.ai/api/v1",
                "apiKey": "",
                "env": { "VENDOR_QUIRK": "1" },
                "npm": "@ai-sdk/anthropic",
            }))),
            enabled_models: vec!["anthropic/claude-opus-4.7".to_string()],
            model_id: "anthropic/claude-opus-4.7".to_string(),
            ..ProviderContext::default()
        }
    }

    #[test]
    fn a_new_harness_needs_no_record_side_code() {
        let harness = FourthHarness;
        let ctx = context();

        assert_eq!(harness.provider_slot(), "claude");
        assert!(!harness.brokers_approvals());
        // No convention of its own: the picker's id reaches the harness as-is.
        assert_eq!(harness.provider_model_id(&ctx), ctx.model_id);

        let env = harness
            .build_provider_injection(&ctx)
            .expect("default applier builds")
            .env
            .expect("non-ambient record emits env");
        assert_eq!(
            env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(
            env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str),
            Some("sk-real")
        );
        assert_eq!(env.get("VENDOR_QUIRK").map(String::as_str), Some("1"));
    }

    #[test]
    fn payload_splits_common_fields_from_harness_extras() {
        let payload = context().payload;
        assert_eq!(
            payload.base_url.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
        // An empty string is a cleared field, not a credential.
        assert_eq!(payload.api_key, None);
        assert_eq!(
            payload.env.get("VENDOR_QUIRK").map(String::as_str),
            Some("1")
        );
        // Everything else stays addressable only by wire key.
        assert_eq!(payload.extra_str("npm"), Some("@ai-sdk/anthropic"));
        assert_eq!(payload.extra_str("baseUrl"), None);
    }

    /// The spawn env carries a structured payload without knowing its shape,
    /// and hands it back only to the harness that put it there. A harness
    /// cannot be handed another's config by mistake.
    #[test]
    fn structured_payloads_only_resolve_for_their_owner() {
        #[derive(Debug, PartialEq)]
        struct CodexShaped(&'static str);

        let injection = ProviderInjection::default()
            .with_structured(BaseCodingAgent::Codex, CodexShaped("cdt"));

        let mut env = ExecutionEnv::new(Default::default(), false, String::new());
        env.provider_structured = injection.structured;

        assert_eq!(
            env.structured::<CodexShaped>(BaseCodingAgent::Codex),
            Some(&CodexShaped("cdt"))
        );
        assert_eq!(
            env.structured::<CodexShaped>(BaseCodingAgent::Opencode),
            None
        );
        // Right owner, wrong shape: still nothing.
        assert_eq!(env.structured::<String>(BaseCodingAgent::Codex), None);
    }

    /// Neither the spawn env nor an action in flight may print a credential.
    #[test]
    fn debug_output_redacts_resolved_material() {
        let ctx = context();
        let debugged = format!("{ctx:?}");
        assert!(debugged.contains("openrouter"));
        assert!(!debugged.contains("sk-real"));

        let injection = FourthHarness.build_provider_injection(&ctx).unwrap();
        let debugged = format!("{injection:?}");
        assert!(debugged.contains("ANTHROPIC_AUTH_TOKEN"));
        assert!(!debugged.contains("sk-real"));
    }
}
