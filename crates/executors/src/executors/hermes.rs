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
    model_selector::{ModelSelectorConfig, PermissionPolicy},
    profile::ExecutorConfig,
    provider::{ProviderContext, ProviderInjection, ProviderInjectionError},
};

/// Stderr substrings the Hermes ACP adapter emits at INFO level that aren't
/// actionable for the cdesktop UI. The adapter hardcodes
/// `root.setLevel(logging.INFO)` (`acp_adapter/entry.py:87`) with no env
/// override; every entry is timestamped `YYYY-MM-DD HH:MM:SS [INFO] name: msg`
/// and may include `\n`-separated continuation lines (e.g. when the prompt
/// body is itself logged). Matching ` [INFO] ` on the first line plus the
/// state-machine continuation rule below suppresses the whole entry while
/// letting `[WARNING]` and `[ERROR]` entries through.
const SUPPRESSED_STDERR_PATTERNS: &[&str] = &[" [INFO] "];

/// True when a stderr line begins a fresh log entry (vs. a continuation of
/// the previous entry). Used by the state-machine suppressor to bound
/// multi-line INFO suppression: a continuation inherits the previous
/// entry's suppress decision, but a fresh entry gets re-evaluated.
///
/// Recognised entry kinds:
/// - **Timestamp prefix** `YYYY-MM-DD HH:` — every Python-logger row (the
///   primary case)
/// - **Bare error markers** — Python tracebacks and uncaught exceptions can
///   reach stderr without going through the logger (so no timestamp). If a
///   traceback follows a suppressed `[INFO]` row, leaving it classified as
///   a continuation would drop the whole stack. Treating these as fresh
///   entries lets them bypass INFO-state suppression and surface in the UI.
fn is_hermes_log_entry_start(line: &str) -> bool {
    has_timestamp_prefix(line)
        || line.starts_with("Traceback ")
        || line.starts_with("ERROR:")
        || line.starts_with("CRITICAL:")
        || line.starts_with("FATAL:")
}

fn has_timestamp_prefix(line: &str) -> bool {
    // Cheap check: `YYYY-MM-DD HH:` — 4 digits, dash, 2 digits, dash, 2
    // digits, space, 2 digits, colon. Anything tighter is overkill.
    let bytes = line.as_bytes();
    bytes.len() >= 14
        && bytes[0..4].iter().all(|b| b.is_ascii_digit())
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(|b| b.is_ascii_digit())
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(|b| b.is_ascii_digit())
        && bytes[10] == b' '
        && bytes[11..13].iter().all(|b| b.is_ascii_digit())
        && bytes[13] == b':'
}

#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct Hermes {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(flatten)]
    pub cmd: CmdOverrides,
    /// Per-message picker selection, preserved so `get_preset_options` can
    /// surface it back through round-trips. Runtime-only: v1 declares only
    /// `Supervised` in `discover_options`, but keeping the field avoids
    /// silently dropping an upstream selection if a future profile flips
    /// the policy.
    #[serde(skip)]
    #[ts(skip)]
    #[schemars(skip)]
    #[derivative(PartialEq = "ignore")]
    pub permission_policy: Option<PermissionPolicy>,
    #[serde(skip)]
    #[ts(skip)]
    #[schemars(skip)]
    #[derivative(Debug = "ignore", PartialEq = "ignore")]
    pub approvals: Option<Arc<dyn ExecutorApprovalService>>,
}

impl Hermes {
    /// Spawn `hermes acp` (ACP stdio JSON-RPC). The runtime takes no flags;
    /// provider selection arrives via env (see `build_hermes_injection` in
    /// `crates/db/src/models/provider.rs`).
    fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
        let builder = CommandBuilder::new("hermes").extend_params(["acp"]);
        apply_overrides(builder, &self.cmd)
    }

    /// Mirror the profile-level `model` (if set) onto `HERMES_INFERENCE_MODEL`
    /// when nothing else has already supplied it. The provider applier sets
    /// the same key from the picker; this fallback is only for cases where
    /// the picker is bypassed (e.g. agent invoked from a profile preset).
    fn with_model_env(&self, env: &ExecutionEnv) -> ExecutionEnv {
        let mut env = env.clone();
        if let Some(model) = &self.model
            && !env.vars.contains_key("HERMES_INFERENCE_MODEL")
            && !env.provider_vars.contains_key("HERMES_INFERENCE_MODEL")
        {
            env.vars
                .insert("HERMES_INFERENCE_MODEL".to_string(), model.clone());
        }
        env
    }

    /// Translate the picker's permission policy into Hermes's `HERMES_YOLO_MODE`
    /// env var. Read per-command by `check_dangerous_command`
    /// (`related/hermes-agent/tools/approval.py:851`), but only consulted via
    /// `os.getenv` inside the spawned process — so it must be set at spawn
    /// time. Each follow-up re-spawns hermes (`acp/harness.rs:147`), so a
    /// per-message picker toggle takes effect on the very next turn.
    ///
    /// Hardline-floor commands (rm -rf /, mkfs, raw-device dd, shutdown,
    /// fork bombs, kill -1) are still blocked unconditionally by Hermes,
    /// before the YOLO check (`approval.py:844`). Bypass relaxes the
    /// dangerous-pattern prompt, not the hardline floor.
    fn with_yolo_env(&self, env: ExecutionEnv) -> ExecutionEnv {
        let mut env = env;
        if matches!(
            self.permission_policy,
            Some(PermissionPolicy::BypassPermissions)
        ) && !env.vars.contains_key("HERMES_YOLO_MODE")
            && !env.provider_vars.contains_key("HERMES_YOLO_MODE")
        {
            env.vars
                .insert("HERMES_YOLO_MODE".to_string(), "1".to_string());
        }
        env
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for Hermes {
    fn apply_overrides(&mut self, executor_config: &ExecutorConfig) {
        if let Some(model_id) = &executor_config.model_id
            && !model_id.is_empty()
        {
            self.model = Some(model_id.clone());
        }
        if let Some(policy) = executor_config.permission_policy.clone() {
            // Only `BypassPermissions` is currently exposed via
            // `discover_options`: `with_yolo_env` injects
            // `HERMES_YOLO_MODE=1`, short-circuiting Hermes's
            // dangerous-command classifier.
            //
            // Supervised is wired through the harness's
            // `permission/request` round-trip in principle, but upstream
            // Hermes's ACP callback is broken (see `discover_options`),
            // so we don't advertise it yet.
            //
            // Other variants (AcceptEdits, Plan, Auto) have no Hermes
            // equivalent: `set_session_mode` is a stub that records
            // `state.mode` but no code reads it back
            // (`acp_adapter/server.py:1685`). We still record whatever
            // policy upstream hands us so round-trips through
            // `get_preset_options` are lossless.
            self.permission_policy = Some(policy);
        }
    }

    fn use_approvals(&mut self, approvals: Arc<dyn ExecutorApprovalService>) {
        self.approvals = Some(approvals);
    }

    fn brokers_approvals(&self) -> bool {
        true
    }

    fn provider_slot(&self) -> &'static str {
        "hermes"
    }

    /// `hermes acp` takes no CLI flags; provider selection happens through env
    /// read at startup by `hermes_cli/runtime_provider.py`. Force the carrier
    /// with `HERMES_INFERENCE_PROVIDER=<slug>` (resolution at `:311`), then
    /// override that carrier's hardcoded base URL with `<SLUG>_BASE_URL` (e.g.
    /// `OPENROUTER_BASE_URL` at `:577`, whose env fallback short-circuits the
    /// constant). The auth var follows the carrier's own convention.
    ///
    /// `apiMode` picks the carrier: `anthropic_messages` -> anthropic,
    /// `codex_responses` -> xai, and `chat_completions` (the default, and any
    /// unrecognised mode) -> openrouter, the most common chat-completions
    /// carrier.
    ///
    /// `HERMES_INFERENCE_MODEL` is read by the oneshot/TUI paths
    /// (`hermes_cli/oneshot.py:228`) but not by `acp_adapter/`, which applies
    /// model selection via `set_session_model` (`acp_adapter/server.py:1651`).
    /// It is set anyway so the same injection stays correct if a future
    /// profile invokes Hermes off the ACP path.
    fn build_provider_injection(
        &self,
        ctx: &ProviderContext,
    ) -> Result<ProviderInjection, ProviderInjectionError> {
        let api_key = ctx.require_api_key(BaseCodingAgent::Hermes)?;
        let base_url = ctx.require_base_url(BaseCodingAgent::Hermes)?;

        let (slug, url_env, key_env) = match ctx.payload.extra_str("apiMode") {
            Some("anthropic_messages") => ("anthropic", "ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY"),
            Some("codex_responses") => ("xai", "XAI_BASE_URL", "XAI_API_KEY"),
            _ => ("openrouter", "OPENROUTER_BASE_URL", "OPENROUTER_API_KEY"),
        };

        let mut env = ctx.payload.env.clone();
        env.insert("HERMES_INFERENCE_PROVIDER".to_string(), slug.to_string());
        env.insert(url_env.to_string(), base_url.to_string());
        env.insert(key_env.to_string(), api_key.to_string());
        if !ctx.model_id.is_empty() {
            env.insert("HERMES_INFERENCE_MODEL".to_string(), ctx.model_id.clone());
        }
        Ok(ProviderInjection::from_env(env))
    }

    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        // Hermes's ACP server reads the model from config.yaml only, not
        // env vars (`cli.py:2390-2398`). The CLI doc is explicit:
        // "LLM_MODEL/OPENAI_MODEL env vars are NOT checked — config.yaml
        // is authoritative." `HERMES_INFERENCE_MODEL` is only honored in
        // `hermes -z` / `--tui`, not `hermes acp`. Push the picker's
        // model through the ACP `session/set_model` round-trip instead,
        // which Hermes does honor (`acp_adapter/server.py:1651`).
        let harness = match &self.model {
            Some(model) => AcpAgentHarness::new().with_model(model.clone()),
            None => AcpAgentHarness::new(),
        };
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let hermes_command = self.build_command_builder()?.build_initial()?;
        let env = self.with_yolo_env(self.with_model_env(env));
        // debug! (not info!) — env.provider_vars contains API keys.
        tracing::debug!(
            command = ?hermes_command,
            cwd = %current_dir.display(),
            env = ?env.vars,
            provider_env = ?env.provider_vars,
            "Spawning Hermes"
        );
        harness
            .spawn_with_command(
                current_dir,
                combined_prompt,
                hermes_command,
                &env,
                &self.cmd,
                self.approvals.clone(),
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
        // See `spawn` for why we push the model via `with_model` rather
        // than rely on `HERMES_INFERENCE_MODEL`.
        let harness = match &self.model {
            Some(model) => AcpAgentHarness::new().with_model(model.clone()),
            None => AcpAgentHarness::new(),
        };
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let hermes_command = self.build_command_builder()?.build_follow_up(&[])?;
        let env = self.with_yolo_env(self.with_model_env(env));
        // debug! (not info!) — env.provider_vars contains API keys.
        tracing::debug!(
            command = ?hermes_command,
            cwd = %current_dir.display(),
            session_id = %session_id,
            env = ?env.vars,
            provider_env = ?env.provider_vars,
            "Spawning Hermes follow-up"
        );
        harness
            .spawn_follow_up_with_command(
                current_dir,
                combined_prompt,
                session_id,
                hermes_command,
                &env,
                &self.cmd,
                self.approvals.clone(),
            )
            .await
    }

    fn normalize_logs(
        &self,
        msg_store: Arc<workspace_utils::msg_store::MsgStore>,
        worktree_path: &Path,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        super::acp::normalize_logs_with_stderr_filter(
            msg_store,
            worktree_path,
            SUPPRESSED_STDERR_PATTERNS,
            Some(is_hermes_log_entry_start),
        )
    }

    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        // Hermes does not have a standalone MCP config file. Servers live
        // under the `mcp_servers` key inside `~/.hermes/config.yaml`
        // (`related/hermes-agent/hermes_cli/mcp_config.py:8,82,89,410`).
        // Returning a path here would make cdesktop's Passthrough adapter
        // write JSON to a file Hermes never reads, breaking MCP merge in
        // both directions. Users configure MCP servers via `hermes mcp add`
        // until cdesktop grows a YAML-sub-key adapter for Hermes.
        None
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        // Hermes installs to ~/.hermes/ via the NousResearch install.sh
        // one-liner. Presence of that directory (with config.yaml or .env)
        // is the strongest local signal of "Hermes is installed."
        let hermes_dir_found = dirs::home_dir()
            .map(|home| {
                let dir = home.join(".hermes");
                dir.join("config.yaml").exists() || dir.join(".env").exists()
            })
            .unwrap_or(false);

        if hermes_dir_found {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }

    fn get_preset_options(&self) -> ExecutorConfig {
        ExecutorConfig {
            executor: BaseCodingAgent::Hermes,
            variant: None,
            model_id: self.model.clone(),
            agent_id: None,
            reasoning_id: None,
            permission_policy: self
                .permission_policy
                .clone()
                .or(Some(PermissionPolicy::BypassPermissions)),
        }
    }

    async fn discover_options(
        &self,
        _workdir: Option<&std::path::Path>,
        _repo_path: Option<&std::path::Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        // No hardcoded model list under Default. Hermes ships no
        // scriptable model-listing surface — `hermes model` requires a
        // TTY (`hermes_cli/main.py:1671`, `_require_tty("model")`) and
        // no CLI subcommand emits JSON (verified across all 39
        // entrypoints). Any hardcoded list we shipped would drift the
        // moment NousResearch updates their catalog, and the picker
        // already exposes per-provider model lists for OpenRouter,
        // Anthropic, etc. via each provider record's `enabledModels`.
        // The Default provider therefore surfaces only the virtual
        // "Default Model" sentinel — meaning "let Hermes resolve the
        // model from ~/.hermes/config.yaml".
        let options = ExecutorDiscoveredOptions {
            model_selector: ModelSelectorConfig {
                models: vec![],
                default_model: None,
                // Supervised is intentionally omitted: Hermes's ACP
                // approval callback (`acp_adapter/permissions.py:43`)
                // hasn't been updated to accept `allow_permanent`, but
                // its CLI-side caller
                // (`tools/approval.py:694`) passes it as a kwarg. Every
                // dangerous-command prompt raises TypeError, the
                // `except` block returns "deny", and the tool call is
                // silently blocked. Re-enable once upstream widens the
                // callback signature.
                permissions: vec![PermissionPolicy::BypassPermissions],
                ..Default::default()
            },
            ..Default::default()
        };
        Ok(Box::pin(futures::stream::once(async move {
            patch::executor_discovered_options(options)
        })))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::env::RepoContext;

    fn empty_env() -> ExecutionEnv {
        ExecutionEnv::new(
            RepoContext::new(PathBuf::from("/tmp"), vec![], vec![]),
            false,
            String::new(),
        )
    }

    fn hermes_with_policy(policy: Option<PermissionPolicy>) -> Hermes {
        Hermes {
            append_prompt: AppendPrompt::default(),
            model: None,
            cmd: CmdOverrides::default(),
            permission_policy: policy,
            approvals: None,
        }
    }

    #[test]
    fn bypass_policy_injects_yolo_env() {
        let agent = hermes_with_policy(Some(PermissionPolicy::BypassPermissions));
        let env = agent.with_yolo_env(empty_env());
        assert_eq!(
            env.vars.get("HERMES_YOLO_MODE").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn supervised_policy_omits_yolo_env() {
        let agent = hermes_with_policy(Some(PermissionPolicy::Supervised));
        let env = agent.with_yolo_env(empty_env());
        assert!(!env.vars.contains_key("HERMES_YOLO_MODE"));
    }

    #[test]
    fn no_policy_omits_yolo_env() {
        let agent = hermes_with_policy(None);
        let env = agent.with_yolo_env(empty_env());
        assert!(!env.vars.contains_key("HERMES_YOLO_MODE"));
    }

    #[test]
    fn existing_yolo_env_preserved() {
        let agent = hermes_with_policy(Some(PermissionPolicy::BypassPermissions));
        let mut env = empty_env();
        env.vars
            .insert("HERMES_YOLO_MODE".to_string(), "preset".to_string());
        let env = agent.with_yolo_env(env);
        assert_eq!(
            env.vars.get("HERMES_YOLO_MODE").map(String::as_str),
            Some("preset"),
        );
    }

    #[test]
    fn provider_yolo_env_blocks_default() {
        let agent = hermes_with_policy(Some(PermissionPolicy::BypassPermissions));
        let mut env = empty_env();
        env.provider_vars
            .insert("HERMES_YOLO_MODE".to_string(), "0".to_string());
        let env = agent.with_yolo_env(env);
        assert!(!env.vars.contains_key("HERMES_YOLO_MODE"));
    }
}
