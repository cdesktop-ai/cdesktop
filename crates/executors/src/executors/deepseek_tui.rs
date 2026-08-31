use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use derivative::Derivative;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use super::acp::AcpAgentHarness;
use crate::{
    approvals::ExecutorApprovalService,
    command::{CmdOverrides, CommandBuildError, CommandBuilder, apply_overrides},
    env::ExecutionEnv,
    executor_discovery::ExecutorDiscoveredOptions,
    executors::{
        AppendPrompt, AvailabilityInfo, BaseCodingAgent, ExecutorError, SpawnedChild,
        StandardCodingAgentExecutor,
    },
    logs::utils::patch,
    model_selector::{ModelInfo, ModelSelectorConfig, PermissionPolicy},
    profile::ExecutorConfig,
    provider::{ProviderContext, ProviderInjection, ProviderInjectionError},
};

/// Stderr lines DeepSeek TUI emits in verbose mode that aren't actionable
/// for the cdesktop UI. The runtime auto-enables verbose logging when any
/// of `RUST_LOG` / `DEEPSEEK_LOG_LEVEL` carry an `info`-or-finer level
/// (`related/DeepSeek-TUI/crates/tui/src/logging.rs:17-36`); cdesktop's
/// server typically runs with `RUST_LOG=info,...` and that leaks down to
/// every spawned child. `with_approval_env` clamps the log level for the
/// child so these never appear in the first place, but if a profile-level
/// env override re-enables verbose mode the suppression list keeps the
/// output clean.
const SUPPRESSED_STDERR_PATTERNS: &[&str] = &[
    "API provider:",
    "API base URL:",
    "Retry policy:",
    "SSL_CERT_FILE=",
];

/// Permission policy → DeepSeek TUI approval policy string. Maps onto the
/// runtime's `DEEPSEEK_APPROVAL_POLICY` env var (config-file values listed at
/// `related/DeepSeek-TUI/docs/CONFIGURATION.md:367`).
fn approval_policy_value(policy: &PermissionPolicy) -> &'static str {
    match policy {
        PermissionPolicy::BypassPermissions => "auto",
        PermissionPolicy::Plan => "never",
        // Default + AcceptEdits + Supervised + Auto all map to "suggest" —
        // the runtime's default mode, which prompts before risky operations.
        _ => "suggest",
    }
}

#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct DeepseekTui {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Bypass-all-approvals toggle. Serializable so `default_profiles.json`
    /// can express "DEFAULT means run-without-approvals" without the picker.
    /// Mirrors Gemini's `yolo` / OpenCode's `auto_approve` field. Maps to
    /// `DEEPSEEK_APPROVAL_POLICY=auto` at spawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yolo: Option<bool>,
    #[serde(flatten)]
    pub cmd: CmdOverrides,
    /// Per-message picker selection — full 3-tier fidelity (Bypass / Plan /
    /// Supervised → auto / never / suggest). Runtime-only: not serialized
    /// onto the profile JSON because the profile-level `yolo` already
    /// covers the bypass case; the per-message override flows in via
    /// `apply_overrides` and wins over `yolo` when set.
    #[serde(skip)]
    #[ts(skip)]
    #[schemars(skip)]
    #[derivative(PartialEq = "ignore")]
    pub approval_policy: Option<PermissionPolicy>,
    #[serde(skip)]
    #[ts(skip)]
    #[schemars(skip)]
    #[derivative(Debug = "ignore", PartialEq = "ignore")]
    pub approvals: Option<Arc<dyn ExecutorApprovalService>>,
}

impl DeepseekTui {
    /// Spawn `deepseek-tui serve --acp`. Per-message provider/model/approval
    /// configuration arrives via env vars (see `build_deepseek_tui_injection`
    /// in `crates/db/src/models/provider.rs`) — the runtime's Cli struct
    /// intentionally does not accept `--provider/--base-url/--api-key/--model`
    /// flags (those exist only on the dispatcher binary).
    fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
        let builder = CommandBuilder::new("deepseek-tui").extend_params(["serve", "--acp"]);
        apply_overrides(builder, &self.cmd)
    }

    /// Resolve the approval-policy env value for this spawn. Per-message
    /// picker override wins; otherwise `yolo=true` → "auto", `yolo=false` →
    /// "suggest". Returns `None` when neither is set so the spawn inherits
    /// the runtime's own default.
    fn resolved_approval_policy(&self) -> Option<&'static str> {
        if let Some(policy) = &self.approval_policy {
            return Some(approval_policy_value(policy));
        }
        match self.yolo {
            Some(true) => Some("auto"),
            Some(false) => Some("suggest"),
            None => None,
        }
    }

    /// Merge the resolved approval-policy + profile-level model onto the env
    /// passed to the child. Mutates a fresh clone so the caller isn't
    /// side-effected.
    fn with_approval_env(&self, env: &ExecutionEnv) -> ExecutionEnv {
        let mut env = env.clone();
        if let Some(value) = self.resolved_approval_policy() {
            env.vars
                .insert("DEEPSEEK_APPROVAL_POLICY".to_string(), value.to_string());
        }
        // The picker model arrives via the provider applier as OPENAI_MODEL;
        // also mirror profile-level `model` if set (rare — picker covers this).
        if let Some(model) = &self.model
            && !env.vars.contains_key("OPENAI_MODEL")
            && !env.provider_vars.contains_key("OPENAI_MODEL")
        {
            env.vars.insert("OPENAI_MODEL".to_string(), model.clone());
        }
        // Silence the runtime's startup-info chatter. cdesktop's server
        // typically runs with `RUST_LOG=info,...`, which the runtime
        // inherits and treats as a request to print API provider / base
        // URL / retry policy / SSL cert lines to stderr on every spawn
        // (`related/DeepSeek-TUI/crates/tui/src/logging.rs:17-36`). Clamp
        // to `warn` so the verbosity check fails and the runtime keeps
        // quiet. A future profile/provider env entry can opt back in by
        // overriding either var via the higher-precedence env layers.
        env.vars
            .entry("RUST_LOG".to_string())
            .or_insert_with(|| "warn".to_string());
        env.vars
            .entry("DEEPSEEK_LOG_LEVEL".to_string())
            .or_insert_with(|| "warn".to_string());
        env
    }

    fn bypass_approvals(&self) -> bool {
        matches!(
            self.approval_policy,
            Some(PermissionPolicy::BypassPermissions)
        ) || self.yolo.unwrap_or(false)
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for DeepseekTui {
    fn apply_overrides(&mut self, executor_config: &ExecutorConfig) {
        if let Some(model_id) = &executor_config.model_id
            && !model_id.is_empty()
        {
            self.model = Some(model_id.clone());
        }
        if let Some(policy) = executor_config.permission_policy.clone() {
            // Mirror Gemini's pattern: PermissionPolicy collapses onto the
            // serializable `yolo` toggle (so settings + DEFAULT profile can
            // express bypass), while the full 3-way picker selection is
            // preserved on `approval_policy` so the spawn env picks the
            // right "auto" / "suggest" / "never" value.
            self.yolo = Some(matches!(policy, PermissionPolicy::BypassPermissions));
            self.approval_policy = Some(policy);
        }
    }

    fn use_approvals(&mut self, approvals: Arc<dyn ExecutorApprovalService>) {
        self.approvals = Some(approvals);
    }

    fn brokers_approvals(&self) -> bool {
        true
    }

    fn provider_slot(&self) -> &'static str {
        "deepseekTui"
    }

    /// The `deepseek-tui` runtime reads provider, credentials and model from
    /// env only — the `--provider` / `--base-url` / `--api-key` / `--model`
    /// flags live on the `deepseek` dispatcher, which forwards them as env
    /// vars anyway.
    ///
    /// Every cdesktop preset routes through the runtime's `openai` provider
    /// mode, and in that mode it reads exclusively the `openai` slots
    /// (`crates/secrets/src/lib.rs:460-487` maps `"openai" => &["OPENAI_API_KEY"]`),
    /// so `DEEPSEEK_API_KEY` / `DEEPSEEK_BASE_URL` are not consulted and not
    /// set. Credential and endpoint go in after the slot's own `env`.
    fn build_provider_injection(
        &self,
        ctx: &ProviderContext,
    ) -> Result<ProviderInjection, ProviderInjectionError> {
        let api_key = ctx.require_api_key(BaseCodingAgent::DeepseekTui)?;
        let base_url = ctx.require_base_url(BaseCodingAgent::DeepseekTui)?;

        let mut env = ctx.payload.env.clone();
        env.insert("DEEPSEEK_PROVIDER".to_string(), "openai".to_string());
        env.insert("OPENAI_BASE_URL".to_string(), base_url.to_string());
        env.insert("OPENAI_API_KEY".to_string(), api_key.to_string());
        if !ctx.model_id.is_empty() {
            env.insert("OPENAI_MODEL".to_string(), ctx.model_id.clone());
        }
        Ok(ProviderInjection::from_env(env))
    }

    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let harness = AcpAgentHarness::new();
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let deepseek_command = self.build_command_builder()?.build_initial()?;
        let env = self.with_approval_env(env);
        // debug! (not info!) — env.provider_vars contains API keys.
        tracing::debug!(
            command = ?deepseek_command,
            cwd = %current_dir.display(),
            env = ?env.vars,
            provider_env = ?env.provider_vars,
            "Spawning DeepSeek TUI"
        );
        let approvals = if self.bypass_approvals() {
            None
        } else {
            self.approvals.clone()
        };
        harness
            .spawn_with_command(
                current_dir,
                combined_prompt,
                deepseek_command,
                &env,
                &self.cmd,
                approvals,
            )
            .await
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        _reset_to_message_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let harness = AcpAgentHarness::new();
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let deepseek_command = self.build_command_builder()?.build_follow_up(&[])?;
        let env = self.with_approval_env(env);
        // debug! (not info!) — env.provider_vars contains API keys.
        tracing::debug!(
            command = ?deepseek_command,
            cwd = %current_dir.display(),
            session_id = %session_id,
            env = ?env.vars,
            provider_env = ?env.provider_vars,
            "Spawning DeepSeek TUI follow-up"
        );
        let approvals = if self.bypass_approvals() {
            None
        } else {
            self.approvals.clone()
        };
        harness
            .spawn_follow_up_with_command(
                current_dir,
                combined_prompt,
                session_id,
                deepseek_command,
                &env,
                &self.cmd,
                approvals,
            )
            .await
    }

    fn normalize_logs(
        &self,
        msg_store: Arc<workspace_utils::msg_store::MsgStore>,
        worktree_path: &Path,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        super::acp::normalize_logs_with_suppressed_stderr_patterns(
            msg_store,
            worktree_path,
            SUPPRESSED_STDERR_PATTERNS,
        )
    }

    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        dirs::home_dir().map(|home| home.join(".deepseek").join("mcp.json"))
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        let mcp_config_found = self
            .default_mcp_config_path()
            .map(|p| p.exists())
            .unwrap_or(false);

        let installation_indicator_found = dirs::home_dir()
            .map(|home| home.join(".deepseek").join("config.toml").exists())
            .unwrap_or(false);

        if mcp_config_found || installation_indicator_found {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }

    fn get_preset_options(&self) -> ExecutorConfig {
        ExecutorConfig {
            executor: BaseCodingAgent::DeepseekTui,
            variant: None,
            model_id: self.model.clone(),
            agent_id: None,
            reasoning_id: None,
            permission_policy: self.approval_policy.clone().or_else(|| {
                if self.yolo.unwrap_or(false) {
                    Some(PermissionPolicy::BypassPermissions)
                } else {
                    Some(PermissionPolicy::Supervised)
                }
            }),
        }
    }

    async fn discover_options(
        &self,
        _workdir: Option<&std::path::Path>,
        _repo_path: Option<&std::path::Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        // Hardcoded fallback list shown when `deepseek-tui models` discovery
        // hasn't been wired up yet (or the binary is missing). The runtime's
        // own registry covers ~80 IDs across 9 providers; the picker stays
        // useful with these two officially-documented IDs and the free-text
        // input as escape hatch.
        let options = ExecutorDiscoveredOptions {
            model_selector: ModelSelectorConfig {
                models: vec![
                    ModelInfo {
                        id: "deepseek-v4-pro".to_string(),
                        name: "DeepSeek V4 Pro".to_string(),
                        provider_id: None,
                        reasoning_options: vec![],
                    },
                    ModelInfo {
                        id: "deepseek-v4-flash".to_string(),
                        name: "DeepSeek V4 Flash".to_string(),
                        provider_id: None,
                        reasoning_options: vec![],
                    },
                ],
                default_model: Some("deepseek-v4-pro".to_string()),
                permissions: vec![
                    PermissionPolicy::BypassPermissions,
                    PermissionPolicy::Supervised,
                    PermissionPolicy::Plan,
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        Ok(Box::pin(futures::stream::once(async move {
            patch::executor_discovered_options(options)
        })))
    }
}
