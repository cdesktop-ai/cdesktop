use std::{collections::HashMap, fmt, path::Path, sync::Arc};

use async_trait::async_trait;
use enum_dispatch::enum_dispatch;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    actions::{
        coding_agent_follow_up::CodingAgentFollowUpRequest,
        coding_agent_initial::CodingAgentInitialRequest, review::ReviewRequest,
        script::ScriptRequest,
    },
    approvals::ExecutorApprovalService,
    env::ExecutionEnv,
    executors::{BaseCodingAgent, ExecutorError, SpawnedChild},
    provider::{ProviderInjection, StructuredInjection},
};
pub mod coding_agent_follow_up;
pub mod coding_agent_initial;
pub mod review;
pub mod script;

pub use review::RepoReviewContext;

#[enum_dispatch]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(tag = "type")]
pub enum ExecutorActionType {
    CodingAgentInitialRequest,
    CodingAgentFollowUpRequest,
    ScriptRequest,
    ReviewRequest,
}

#[derive(Clone, Serialize, Deserialize, TS)]
pub struct ExecutorAction {
    pub typ: ExecutorActionType,
    pub next_action: Option<Box<ExecutorAction>>,
    /// Provider-resolved env vars to inject at spawn. These are runtime-only
    /// and `serde(skip)`: resolved secrets can never serialize into durable
    /// records, APIs, or snapshots — persistence keeps only opaque
    /// provider/model identifiers.
    #[serde(skip)]
    #[ts(skip)]
    pub provider_env: Option<HashMap<String, String>>,
    /// Structured spawn injection owned by the harness that emitted it,
    /// populated alongside `provider_env` from the same adapter. Runtime-only
    /// and `serde(skip)` for the same reason as `provider_env`.
    #[serde(skip)]
    #[ts(skip)]
    pub provider_structured: Option<StructuredInjection>,
    /// Provider ID selected for this message; persisted to coding_agent_turns
    /// for recents query and transcript markers (§4/§6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub selected_provider_id: Option<String>,
    /// Model ID selected for this message; persisted alongside provider_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub selected_model_id: Option<String>,
}

/// Manual `Debug` so tracing/logging an in-flight action can never print
/// resolved provider secrets: env var names stay visible for diagnostics,
/// values are redacted.
impl fmt::Debug for ExecutorAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutorAction")
            .field("typ", &self.typ)
            .field("next_action", &self.next_action)
            .field(
                "provider_env",
                &self
                    .provider_env
                    .as_ref()
                    .map(|env| env.keys().map(String::as_str).collect::<Vec<_>>()),
            )
            .field("provider_structured", &self.provider_structured)
            .field("selected_provider_id", &self.selected_provider_id)
            .field("selected_model_id", &self.selected_model_id)
            .finish()
    }
}

impl ExecutorAction {
    pub fn new(typ: ExecutorActionType, next_action: Option<Box<ExecutorAction>>) -> Self {
        Self {
            typ,
            next_action,
            provider_env: None,
            provider_structured: None,
            selected_provider_id: None,
            selected_model_id: None,
        }
    }

    /// Carry an adapter's spawn injection onto the action, whatever shape it
    /// took. Call sites never learn which harness produced it.
    pub fn with_provider_injection(mut self, injection: ProviderInjection) -> Self {
        self.provider_env = injection.env;
        self.provider_structured = injection.structured;
        self
    }

    pub fn with_provider_selection(
        mut self,
        provider_id: Option<String>,
        model_id: Option<String>,
    ) -> Self {
        self.selected_provider_id = provider_id;
        self.selected_model_id = model_id;
        self
    }

    pub fn without_provider_bindings(&self) -> Self {
        let mut action = self.clone();
        action.provider_env = None;
        action.provider_structured = None;
        action.next_action = action
            .next_action
            .as_ref()
            .map(|next| Box::new(next.without_provider_bindings()));
        action
    }

    pub fn append_action(mut self, action: ExecutorAction) -> Self {
        if let Some(next) = self.next_action {
            self.next_action = Some(Box::new(next.append_action(action)));
        } else {
            self.next_action = Some(Box::new(action));
        }
        self
    }

    pub fn typ(&self) -> &ExecutorActionType {
        &self.typ
    }

    pub fn next_action(&self) -> Option<&ExecutorAction> {
        self.next_action.as_deref()
    }

    pub fn base_executor(&self) -> Option<BaseCodingAgent> {
        match self.typ() {
            ExecutorActionType::CodingAgentInitialRequest(request) => Some(request.base_executor()),
            ExecutorActionType::CodingAgentFollowUpRequest(request) => {
                Some(request.base_executor())
            }
            ExecutorActionType::ReviewRequest(request) => Some(request.base_executor()),
            ExecutorActionType::ScriptRequest(_) => None,
        }
    }
}

#[async_trait]
#[enum_dispatch(ExecutorActionType)]
pub trait Executable {
    async fn spawn(
        &self,
        current_dir: &Path,
        approvals: Arc<dyn ExecutorApprovalService>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError>;
}

#[async_trait]
impl Executable for ExecutorAction {
    async fn spawn(
        &self,
        current_dir: &Path,
        approvals: Arc<dyn ExecutorApprovalService>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        self.typ.spawn(current_dir, approvals, env).await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        actions::{
            coding_agent_initial::{CodingAgentInitialRequest, PromptKind},
            script::{ScriptContext, ScriptRequest, ScriptRequestLanguage},
        },
        executors::codex::CodexProviderInjection,
        profile::ExecutorConfig,
    };

    #[test]
    fn storage_action_keeps_opaque_provider_ref_without_runtime_bindings() {
        let action = ExecutorAction::new(
            ExecutorActionType::CodingAgentInitialRequest(CodingAgentInitialRequest {
                prompt: "ship it".to_string(),
                prompt_kind: PromptKind::User,
                executor_config: ExecutorConfig::new(BaseCodingAgent::Codex),
                working_dir: None,
            }),
            Some(Box::new(ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: "echo done".to_string(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::CleanupScript,
                    working_dir: None,
                }),
                None,
            ))),
        )
        .with_provider_injection(
            ProviderInjection::from_env(HashMap::from([(
                "OPENAI_API_KEY".to_string(),
                "secret".to_string(),
            )]))
            .with_structured(
                BaseCodingAgent::Codex,
                CodexProviderInjection {
                    model_provider_id: "cdt".to_string(),
                    config_overrides: HashMap::from([(
                        "model_providers.cdt.env_key".to_string(),
                        json!("OPENAI_API_KEY"),
                    )]),
                },
            ),
        )
        .with_provider_selection(
            Some("2f6dd8b2-5ce0-42c6-9e23-c8ecab684716".to_string()),
            Some("gpt-5.1".to_string()),
        );

        let storage = action.without_provider_bindings();

        assert!(storage.provider_env.is_none());
        assert!(storage.provider_structured.is_none());
        assert_eq!(
            storage.selected_provider_id.as_deref(),
            Some("2f6dd8b2-5ce0-42c6-9e23-c8ecab684716")
        );
        let serialized = serde_json::to_value(storage).unwrap();
        assert_eq!(
            serialized["selected_provider_id"],
            "2f6dd8b2-5ce0-42c6-9e23-c8ecab684716"
        );
        assert!(serialized.get("provider_env").is_none());
        assert!(serialized.get("provider_structured").is_none());
    }

    #[test]
    fn runtime_action_with_resolved_secrets_never_serializes_or_debugs_them() {
        let action = ExecutorAction::new(
            ExecutorActionType::CodingAgentInitialRequest(CodingAgentInitialRequest {
                prompt: "ship it".to_string(),
                prompt_kind: PromptKind::User,
                executor_config: ExecutorConfig::new(BaseCodingAgent::ClaudeCode),
                working_dir: None,
            }),
            None,
        )
        .with_provider_injection(ProviderInjection::from_env(HashMap::from([(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            "sk-live-secret".to_string(),
        )])));

        // Even the unstripped runtime action must not serialize secrets.
        let serialized = serde_json::to_string(&action).unwrap();
        assert!(!serialized.contains("sk-live-secret"));
        assert!(!serialized.contains("provider_env"));

        // Debug keeps the env var name for diagnostics but never the value.
        let debugged = format!("{action:?}");
        assert!(debugged.contains("ANTHROPIC_AUTH_TOKEN"));
        assert!(!debugged.contains("sk-live-secret"));
    }
}
