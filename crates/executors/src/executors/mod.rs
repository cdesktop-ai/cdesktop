use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use command_group::AsyncGroupChild;
use enum_dispatch::enum_dispatch;
use futures::stream::BoxStream;
use futures_io::Error as FuturesIoError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::Type;
use strum_macros::{Display, EnumDiscriminants, EnumString, VariantNames};
use thiserror::Error;
use tokio::task::JoinHandle;
use ts_rs::TS;
use workspace_utils::msg_store::MsgStore;

#[cfg(feature = "qa-mode")]
use crate::executors::qa_mock::QaMockExecutor;
use crate::{
    actions::{ExecutorAction, review::RepoReviewContext},
    approvals::ExecutorApprovalService,
    command::CommandBuildError,
    env::ExecutionEnv,
    executors::{
        amp::Amp, claude::ClaudeCode, codex::Codex, copilot::Copilot, cursor::CursorAgent,
        deepseek_tui::DeepseekTui, droid::Droid, gemini::Gemini, hermes::Hermes,
        opencode::Opencode, qwen::QwenCode,
    },
    logs::utils::patch,
    mcp_config::McpConfig,
    profile::{ExecutorConfig, ExecutorConfigs},
    provider::{ProviderContext, ProviderInjection, ProviderInjectionError},
};

pub mod acp;
pub mod amp;
pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod deepseek_tui;
pub mod droid;
pub mod gemini;
pub mod hermes;
pub mod opencode;
#[cfg(feature = "qa-mode")]
pub mod qa_mock;
pub mod qwen;
pub mod utils;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct SlashCommandDescription {
    /// Command name without the leading slash, e.g. `help` for `/help`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(use_ts_enum)]
pub enum BaseAgentCapability {
    SessionFork,
    /// Agent requires a setup script before it can run (e.g., login, installation)
    SetupHelper,
    /// Agent reports context/token usage information
    ContextUsage,
}

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("Follow-up is not supported: {0}")]
    FollowUpNotSupported(String),
    #[error(transparent)]
    SpawnError(#[from] FuturesIoError),
    #[error("Unknown executor type: {0}")]
    UnknownExecutorType(String),
    #[error("I/O error: {0}")]
    Io(std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    TomlSerialize(#[from] toml::ser::Error),
    #[error(transparent)]
    TomlDeserialize(#[from] toml::de::Error),
    #[error(transparent)]
    ExecutorApprovalError(#[from] crate::approvals::ExecutorApprovalError),
    #[error(transparent)]
    CommandBuild(#[from] CommandBuildError),
    #[error("Executable `{program}` not found in PATH")]
    ExecutableNotFound { program: String },
    #[error("Setup helper not supported")]
    SetupHelperNotSupported,
    #[error("Auth required: {0}")]
    AuthRequired(String),
}

#[enum_dispatch]
#[derive(
    Debug, Clone, Serialize, Deserialize, PartialEq, TS, Display, EnumDiscriminants, VariantNames,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[strum_discriminants(
    name(BaseCodingAgent),
    // Only add Hash; Eq/PartialEq are already provided by EnumDiscriminants.
    derive(EnumString, Hash, strum_macros::Display, Serialize, Deserialize, TS, Type),
    strum(serialize_all = "SCREAMING_SNAKE_CASE"),
    ts(use_ts_enum),
    serde(rename_all = "SCREAMING_SNAKE_CASE"),
    sqlx(type_name = "TEXT", rename_all = "SCREAMING_SNAKE_CASE")
)]
pub enum CodingAgent {
    ClaudeCode,
    Amp,
    Gemini,
    Codex,
    Opencode,
    #[serde(alias = "CURSOR")]
    #[strum_discriminants(serde(alias = "CURSOR"))]
    #[strum_discriminants(strum(serialize = "CURSOR", serialize = "CURSOR_AGENT"))]
    CursorAgent,
    QwenCode,
    Copilot,
    Droid,
    DeepseekTui,
    Hermes,
    #[cfg(feature = "qa-mode")]
    QaMock(QaMockExecutor),
}

impl CodingAgent {
    /// The adapter registered for a harness, at its default variant.
    ///
    /// The profile registry is the single place a harness is registered, so
    /// it is also the single place anything downstream resolves one. A
    /// harness with no profile has no adapter, and therefore no provider
    /// injection, no model-id convention, and no approval brokering.
    pub fn registered(base: BaseCodingAgent) -> Option<Self> {
        let configs = ExecutorConfigs::get_cached();
        let profile = configs.executors.get(&base)?;
        profile
            .get_variant("DEFAULT")
            .or_else(|| profile.configurations.values().next())
            .cloned()
    }

    pub fn get_mcp_config(&self) -> McpConfig {
        match self {
            Self::Codex(_) => McpConfig::new(
                vec!["mcp_servers".to_string()],
                serde_json::json!({
                    "mcp_servers": {}
                }),
                self.preconfigured_mcp(),
                true,
            ),
            Self::Amp(_) => McpConfig::new(
                vec!["amp.mcpServers".to_string()],
                serde_json::json!({
                    "amp.mcpServers": {}
                }),
                self.preconfigured_mcp(),
                false,
            ),
            Self::Opencode(_) => McpConfig::new(
                vec!["mcp".to_string()],
                serde_json::json!({
                    "mcp": {},
                    "$schema": "https://opencode.ai/config.json"
                }),
                self.preconfigured_mcp(),
                false,
            ),
            Self::Droid(_) => McpConfig::new(
                vec!["mcpServers".to_string()],
                serde_json::json!({
                    "mcpServers": {}
                }),
                self.preconfigured_mcp(),
                false,
            ),
            _ => McpConfig::new(
                vec!["mcpServers".to_string()],
                serde_json::json!({
                    "mcpServers": {}
                }),
                self.preconfigured_mcp(),
                false,
            ),
        }
    }

    pub fn supports_mcp(&self) -> bool {
        self.default_mcp_config_path().is_some()
    }

    pub fn capabilities(&self) -> Vec<BaseAgentCapability> {
        match self {
            Self::ClaudeCode(_) => vec![
                BaseAgentCapability::SessionFork,
                BaseAgentCapability::ContextUsage,
            ],
            Self::Opencode(_) => vec![
                BaseAgentCapability::SessionFork,
                BaseAgentCapability::ContextUsage,
            ],
            Self::Codex(_) => vec![
                BaseAgentCapability::SessionFork,
                BaseAgentCapability::SetupHelper,
                BaseAgentCapability::ContextUsage,
            ],
            Self::Gemini(_) | Self::QwenCode(_) => {
                vec![BaseAgentCapability::SessionFork]
            }
            Self::Hermes(_) => vec![BaseAgentCapability::ContextUsage],
            Self::CursorAgent(_) => vec![BaseAgentCapability::SetupHelper],
            Self::Amp(_) | Self::Copilot(_) | Self::Droid(_) | Self::DeepseekTui(_) => vec![],
            #[cfg(feature = "qa-mode")]
            Self::QaMock(_) => vec![], // QA mock doesn't need special capabilities
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AvailabilityInfo {
    LoginDetected { last_auth_timestamp: i64 },
    InstallationFound,
    NotFound,
}

impl AvailabilityInfo {
    pub fn is_available(&self) -> bool {
        matches!(
            self,
            AvailabilityInfo::LoginDetected { .. } | AvailabilityInfo::InstallationFound
        )
    }
}

#[async_trait]
#[enum_dispatch(CodingAgent)]
pub trait StandardCodingAgentExecutor {
    fn apply_overrides(&mut self, _executor_config: &ExecutorConfig) {}

    fn use_approvals(&mut self, _approvals: Arc<dyn ExecutorApprovalService>) {}

    /// Whether this harness routes tool approvals back through cdesktop.
    ///
    /// True exactly when [`Self::use_approvals`] keeps the service it is
    /// handed; a harness that drops it never asks, and gets the no-op
    /// service instead of a live bridge. `approval_wiring_matches_declaration`
    /// holds the two in step.
    fn brokers_approvals(&self) -> bool {
        false
    }

    /// Wire key of this harness's payload slot on a provider record.
    ///
    /// The default is Claude's slot, which is what
    /// [`Self::build_provider_injection`]'s default applier reads. A harness
    /// that has not grown its own applier is gated out of the picker by the
    /// record's `perAgentEnabled` until it does.
    fn provider_slot(&self) -> &'static str {
        "claude"
    }

    /// Spawn-time injection for the provider record the user picked.
    ///
    /// Only called for a record that carries its own credentials; an ambient
    /// record never reaches an adapter, so no adapter can leak an injection
    /// into a spawn that was meant to use the harness's own auth.
    ///
    /// Defaults to the Anthropic-compatible env applier — every harness that
    /// speaks the Anthropic wire protocol needs nothing else.
    fn build_provider_injection(
        &self,
        ctx: &ProviderContext,
    ) -> Result<ProviderInjection, ProviderInjectionError> {
        Ok(claude::anthropic_env_injection(ctx))
    }

    /// The picker-selected model id in this harness's own id form.
    ///
    /// Harnesses that address models by the vendor's raw id need no override.
    fn provider_model_id(&self, ctx: &ProviderContext) -> String {
        ctx.model_id.clone()
    }

    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError>;

    /// Continue a session, optionally resetting to a specific message.
    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        reset_to_message_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError>;

    async fn spawn_review(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        match session_id {
            Some(id) => {
                self.spawn_follow_up(current_dir, prompt, id, None, env)
                    .await
            }
            None => self.spawn(current_dir, prompt, env).await,
        }
    }

    fn normalize_logs(
        &self,
        _raw_logs_event_store: Arc<MsgStore>,
        _worktree_path: &Path,
    ) -> Vec<JoinHandle<()>> {
        vec![]
    }

    // MCP configuration methods
    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf>;

    async fn get_setup_helper_action(&self) -> Result<ExecutorAction, ExecutorError> {
        Err(ExecutorError::SetupHelperNotSupported)
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        let config_files_found = self
            .default_mcp_config_path()
            .map(|path| path.exists())
            .unwrap_or(false);

        if config_files_found {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }

    /// Returns a stream of executor discovered options updates.
    async fn discover_options(
        &self,
        _workdir: Option<&Path>,
        _repo_path: Option<&Path>,
    ) -> Result<BoxStream<'static, json_patch::Patch>, ExecutorError> {
        let options = crate::executor_discovery::ExecutorDiscoveredOptions::default();
        Ok(Box::pin(futures::stream::once(async move {
            patch::executor_discovered_options(options)
        })))
    }

    /// Returns the default overrides defined by this preset/variant.
    fn get_preset_options(&self) -> ExecutorConfig;
}

/// Result communicated through the exit signal
#[derive(Debug, Clone)]
pub enum ExecutorExitResult {
    /// Process completed successfully (exit code 0)
    Success,
    /// Process should be marked as failed (non-zero exit). Carries the
    /// normalized outcome when the executor observed a stable provider
    /// signal; `None` means unclassified.
    Failure(Option<crate::outcome::NormalizedExecutionOutcome>),
}

/// Optional exit notification from an executor.
/// When this receiver resolves, the container should gracefully stop the process
/// and mark it according to the result.
pub type ExecutorExitSignal = tokio::sync::oneshot::Receiver<ExecutorExitResult>;

/// Cancellation token for requesting graceful shutdown of an executor.
/// When cancelled, the executor should attempt to cancel gracefully before being killed.
pub type CancellationToken = tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct SpawnedChild {
    pub child: AsyncGroupChild,
    /// Executor → Container: signals when executor wants to exit
    pub exit_signal: Option<ExecutorExitSignal>,
    /// Container → Executor: signals when container wants to cancel the execution
    pub cancel: Option<CancellationToken>,
}

impl From<AsyncGroupChild> for SpawnedChild {
    fn from(child: AsyncGroupChild) -> Self {
        Self {
            child,
            exit_signal: None,
            cancel: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema)]
#[serde(transparent)]
#[schemars(
    title = "Append Prompt",
    description = "Extra text appended to the prompt",
    extend("format" = "textarea")
)]
#[derive(Default)]
pub struct AppendPrompt(pub Option<String>);

impl AppendPrompt {
    pub fn get(&self) -> Option<String> {
        self.0.clone()
    }

    pub fn combine_prompt(&self, prompt: &str) -> String {
        match self {
            AppendPrompt(Some(value)) => format!("{prompt}{value}"),
            AppendPrompt(None) => prompt.to_string(),
        }
    }
}

pub fn build_review_prompt(
    context: Option<&[RepoReviewContext]>,
    additional_prompt: Option<&str>,
) -> String {
    let mut prompt = String::from("Please review the code changes.\n\n");

    if let Some(repos) = context {
        for repo in repos {
            prompt.push_str(&format!("Repository: {}\n", repo.repo_name));
            prompt.push_str(&format!(
                "Review all changes from base commit {} to HEAD.\n",
                repo.base_commit
            ));
            prompt.push_str(&format!(
                "Use `git diff {}..HEAD` to see the changes.\n",
                repo.base_commit
            ));
            prompt.push('\n');
        }
    }

    if let Some(additional) = additional_prompt {
        prompt.push_str(additional);
    }

    prompt
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_cursor_agent_deserialization() {
        // Test that CURSOR_AGENT is accepted
        let result = BaseCodingAgent::from_str("CURSOR_AGENT");
        assert!(result.is_ok(), "CURSOR_AGENT should be valid");
        assert_eq!(result.unwrap(), BaseCodingAgent::CursorAgent);

        // Test that legacy CURSOR is still accepted for backwards compatibility
        let result = BaseCodingAgent::from_str("CURSOR");
        assert!(
            result.is_ok(),
            "CURSOR should be valid for backwards compatibility"
        );
        assert_eq!(result.unwrap(), BaseCodingAgent::CursorAgent);

        // Test serde deserialization for CURSOR_AGENT
        let result: Result<BaseCodingAgent, _> = serde_json::from_str(r#""CURSOR_AGENT""#);
        assert!(result.is_ok(), "CURSOR_AGENT should deserialize via serde");
        assert_eq!(result.unwrap(), BaseCodingAgent::CursorAgent);

        // Test serde deserialization for legacy CURSOR
        let result: Result<BaseCodingAgent, _> = serde_json::from_str(r#""CURSOR""#);
        assert!(result.is_ok(), "CURSOR should deserialize via serde");
        assert_eq!(result.unwrap(), BaseCodingAgent::CursorAgent);
    }
}

#[cfg(test)]
mod adapter_surface_tests {
    use std::{collections::HashMap, sync::Arc};

    use serde_json::json;
    use strum::VariantNames;

    use super::*;
    use crate::approvals::NoopExecutorApprovalService;

    /// Every adapter, at its default settings — built from the enum's own
    /// variant list so a harness added tomorrow is covered without editing
    /// this test.
    fn all_adapters() -> Vec<(&'static str, CodingAgent)> {
        CodingAgent::VARIANTS
            .iter()
            .map(|name| {
                let agent = serde_json::from_value(json!({ *name: {} }))
                    .unwrap_or_else(|e| panic!("{name} must build from defaults: {e}"));
                (*name, agent)
            })
            .collect()
    }

    /// The approval bridge is handed out on `brokers_approvals`, so that answer
    /// has to be the same one `use_approvals` gives by keeping the service.
    /// A harness that stores it but declares `false` would ask into a no-op;
    /// one that declares `true` and drops it would hold a bridge open for
    /// nothing.
    #[test]
    fn approval_wiring_matches_declaration() {
        for (name, agent) in all_adapters() {
            let probe: Arc<dyn ExecutorApprovalService> = Arc::new(NoopExecutorApprovalService);
            let mut agent = agent;
            agent.use_approvals(probe.clone());
            let kept = Arc::strong_count(&probe) > 1;
            assert_eq!(
                kept,
                agent.brokers_approvals(),
                "{name}: use_approvals {} the service but brokers_approvals() is {}",
                if kept { "keeps" } else { "drops" },
                agent.brokers_approvals()
            );
        }
    }

    /// A harness reads its own slot and nobody else's. Two harnesses sharing a
    /// slot would silently spend one record's credentials on the other's
    /// endpoint.
    #[test]
    fn declared_slots_are_unique_per_harness() {
        let mut seen: HashMap<&'static str, &'static str> = HashMap::new();
        for (name, agent) in all_adapters() {
            let slot = agent.provider_slot();
            // The default slot is shared by every harness without its own
            // applier; only declared slots have to be exclusive.
            if slot == "claude" && name != "CLAUDE_CODE" {
                continue;
            }
            if let Some(other) = seen.insert(slot, name) {
                panic!("{name} and {other} both claim provider slot '{slot}'");
            }
        }
    }
}
