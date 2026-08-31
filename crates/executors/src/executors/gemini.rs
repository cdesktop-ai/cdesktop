use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use derivative::Derivative;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use workspace_utils::msg_store::MsgStore;

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

const SUPPRESSED_STDERR_PATTERNS: &[&str] = &[
    "was started but never ended. Skipping metrics.",
    "YOLO mode is enabled. All tool calls will be automatically approved.",
    "Ripgrep is not available. Falling back to GrepTool.",
    "is overriding the built-in skill.",
];

/// gemini-cli refuses to load `~/.gemini/.env` (and the API key it carries)
/// when the spawn cwd isn't on the user's `trustedFolders.json` allowlist.
/// Session worktrees are fresh paths and never on that list, so the auth
/// env never reaches `process.env` and requests go out keyless → 400.
/// `GEMINI_CLI_TRUST_WORKSPACE=true` is gemini-cli's documented override
/// (`packages/core/src/utils/trust.ts:47-49`), checked before the trust
/// file. cdesktop has already authorized the user to act on the worktree
/// via session creation, so opting in matches the security model. Set on
/// `env.vars` (lowest precedence) so a future profile/provider env entry
/// can opt out by overwriting it.
fn with_workspace_trust(env: &ExecutionEnv) -> ExecutionEnv {
    let mut env = env.clone();
    env.vars
        .insert("GEMINI_CLI_TRUST_WORKSPACE".to_string(), "true".to_string());
    env
}

#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct Gemini {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yolo: Option<bool>,
    #[serde(flatten)]
    pub cmd: CmdOverrides,
    #[serde(skip)]
    #[ts(skip)]
    #[derivative(Debug = "ignore", PartialEq = "ignore")]
    pub approvals: Option<Arc<dyn ExecutorApprovalService>>,
}

impl Gemini {
    fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
        let mut builder = CommandBuilder::new("gemini");

        if let Some(model) = &self.model {
            builder = builder.extend_params(["--model", model.as_str()]);
        }

        if self.yolo.unwrap_or(false) {
            builder = builder.extend_params(["--yolo"]);
        }

        builder = builder.extend_params(["--acp"]);

        apply_overrides(builder, &self.cmd)
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for Gemini {
    fn apply_overrides(&mut self, executor_config: &ExecutorConfig) {
        if let Some(model_id) = &executor_config.model_id
            && !model_id.is_empty()
        {
            self.model = Some(model_id.clone());
        }
        if let Some(permission_policy) = executor_config.permission_policy.clone() {
            self.yolo = Some(matches!(
                permission_policy,
                crate::model_selector::PermissionPolicy::BypassPermissions
            ));
        }
    }

    fn use_approvals(&mut self, approvals: Arc<dyn ExecutorApprovalService>) {
        self.approvals = Some(approvals);
    }

    fn brokers_approvals(&self) -> bool {
        true
    }

    fn provider_slot(&self) -> &'static str {
        "gemini"
    }

    /// gemini-cli reads provider config from env only. `GOOGLE_GEMINI_BASE_URL`
    /// and `GEMINI_API_KEY` are set after the slot's own `env` so a
    /// vendor-quirk entry cannot shadow the endpoint or the credential.
    ///
    /// `GOOGLE_API_KEY` (gemini-cli's alternate credential var,
    /// `contentGenerator.ts:156`) is neither set nor cleared:
    /// `getAuthTypeFromEnv` (`:76-93`) prefers `GEMINI_API_KEY`, so ours wins
    /// and an ambient `GOOGLE_API_KEY` passes through untouched.
    ///
    /// The catalog ships no Gemini presets — ambient auth covers official
    /// Google routing — so this applier exists for Custom records pointing at
    /// user-supplied Google-API-compatible endpoints.
    fn build_provider_injection(
        &self,
        ctx: &ProviderContext,
    ) -> Result<ProviderInjection, ProviderInjectionError> {
        let api_key = ctx.require_api_key(BaseCodingAgent::Gemini)?;
        let base_url = ctx.require_base_url(BaseCodingAgent::Gemini)?;

        let mut env = ctx.payload.env.clone();
        env.insert("GOOGLE_GEMINI_BASE_URL".to_string(), base_url.to_string());
        env.insert("GEMINI_API_KEY".to_string(), api_key.to_string());
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
        let gemini_command = self.build_command_builder()?.build_initial()?;
        let env = with_workspace_trust(env);
        // debug! (not info!) — env.provider_vars contains API keys.
        tracing::debug!(
            command = ?gemini_command,
            cwd = %current_dir.display(),
            env = ?env.vars,
            provider_env = ?env.provider_vars,
            "Spawning Gemini"
        );
        let approvals = if self.yolo.unwrap_or(false) {
            None
        } else {
            self.approvals.clone()
        };
        harness
            .spawn_with_command(
                current_dir,
                combined_prompt,
                gemini_command,
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
        let gemini_command = self.build_command_builder()?.build_follow_up(&[])?;
        let env = with_workspace_trust(env);
        // debug! (not info!) — env.provider_vars contains API keys.
        tracing::debug!(
            command = ?gemini_command,
            cwd = %current_dir.display(),
            session_id = %session_id,
            env = ?env.vars,
            provider_env = ?env.provider_vars,
            "Spawning Gemini follow-up"
        );
        let approvals = if self.yolo.unwrap_or(false) {
            None
        } else {
            self.approvals.clone()
        };
        harness
            .spawn_follow_up_with_command(
                current_dir,
                combined_prompt,
                session_id,
                gemini_command,
                &env,
                &self.cmd,
                approvals,
            )
            .await
    }

    fn normalize_logs(
        &self,
        msg_store: Arc<MsgStore>,
        worktree_path: &Path,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        super::acp::normalize_logs_with_suppressed_stderr_patterns(
            msg_store,
            worktree_path,
            SUPPRESSED_STDERR_PATTERNS,
        )
    }

    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        dirs::home_dir().map(|home| home.join(".gemini").join("settings.json"))
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        if let Some(timestamp) = dirs::home_dir()
            .and_then(|home| std::fs::metadata(home.join(".gemini").join("oauth_creds.json")).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
        {
            return AvailabilityInfo::LoginDetected {
                last_auth_timestamp: timestamp,
            };
        }

        let mcp_config_found = self
            .default_mcp_config_path()
            .map(|p| p.exists())
            .unwrap_or(false);

        let installation_indicator_found = dirs::home_dir()
            .map(|home| home.join(".gemini").join("installation_id").exists())
            .unwrap_or(false);

        if mcp_config_found || installation_indicator_found {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }

    fn get_preset_options(&self) -> ExecutorConfig {
        use crate::model_selector::*;
        ExecutorConfig {
            executor: BaseCodingAgent::Gemini,
            variant: None,
            model_id: self.model.clone(),
            agent_id: None,
            reasoning_id: None,
            permission_policy: Some(if self.yolo.unwrap_or(false) {
                PermissionPolicy::BypassPermissions
            } else {
                PermissionPolicy::Supervised
            }),
        }
    }

    async fn discover_options(
        &self,
        _workdir: Option<&std::path::Path>,
        _repo_path: Option<&std::path::Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        let options = ExecutorDiscoveredOptions {
            model_selector: ModelSelectorConfig {
                models: vec![
                    ModelInfo {
                        id: "gemini-3.1-pro-preview".to_string(),
                        name: "Gemini 3.1 Pro Preview".to_string(),
                        provider_id: None,
                        reasoning_options: vec![],
                    },
                    ModelInfo {
                        id: "gemini-3-flash-preview".to_string(),
                        name: "Gemini 3 Flash Preview".to_string(),
                        provider_id: None,
                        reasoning_options: vec![],
                    },
                    ModelInfo {
                        id: "gemini-3.1-flash-lite-preview".to_string(),
                        name: "Gemini 3.1 Flash Lite Preview".to_string(),
                        provider_id: None,
                        reasoning_options: vec![],
                    },
                ],
                default_model: Some("gemini-3.1-pro-preview".to_string()),
                permissions: vec![
                    PermissionPolicy::BypassPermissions,
                    PermissionPolicy::Supervised,
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
