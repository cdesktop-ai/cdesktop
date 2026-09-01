use std::{cmp::Ordering, path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use command_group::AsyncGroupChild;
use convert_case::{Case, Casing};
use derivative::Derivative;
use futures::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::{io::AsyncBufReadExt, process::Command};
use ts_rs::TS;
use workspace_utils::{command_ext::GroupSpawnNoWindowExt, msg_store::MsgStore};

use crate::{
    approvals::ExecutorApprovalService,
    command::{CmdOverrides, CommandBuildError, CommandBuilder, apply_overrides},
    env::{ExecutionEnv, RepoContext},
    executors::{
        AppendPrompt, AvailabilityInfo, BaseCodingAgent, ExecutorError, ExecutorExitResult,
        SlashCommandDescription, SpawnedChild, StandardCodingAgentExecutor,
        opencode::types::OpencodeExecutorEvent, utils::reorder_slash_commands,
    },
    logs::utils::patch,
    model_selector::{AgentInfo, ModelInfo, ModelProvider, PermissionPolicy, ReasoningOption},
    profile::ExecutorConfig,
    provider::{ProviderContext, ProviderInjection, ProviderInjectionError},
    stdout_dup::create_stdout_pipe_writer,
};

mod models;
mod normalize_logs;
mod outcome;
pub(crate) mod sdk;
mod slash_commands;
pub(crate) mod types;

use outcome::OutcomeSink;
use sdk::{
    AgentInfo as SDKAgentInfo, LogWriter, RunConfig, build_authenticated_client,
    generate_server_password, list_agents, list_commands, list_providers, run_session,
    run_slash_command,
};
use slash_commands::{OpencodeSlashCommand, hardcoded_slash_commands};
use types::{Config, ProviderModelInfo};

#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct Opencode {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "mode")]
    pub agent: Option<String>,
    /// Auto-approve agent actions
    #[serde(default = "default_to_true")]
    pub auto_approve: bool,
    /// Enable auto-compaction when the context length approaches the model's context window limit
    #[serde(default = "default_to_true")]
    pub auto_compact: bool,
    #[serde(flatten)]
    pub cmd: CmdOverrides,
    #[serde(skip)]
    #[ts(skip)]
    #[derivative(Debug = "ignore", PartialEq = "ignore")]
    pub approvals: Option<Arc<dyn ExecutorApprovalService>>,
}

/// Represents a spawned OpenCode server with its base URL
struct OpencodeServer {
    #[allow(unused)]
    child: Option<AsyncGroupChild>,
    base_url: String,
    server_password: ServerPassword,
}

impl Drop for OpencodeServer {
    fn drop(&mut self) {
        // kill the process properly using the kill helper as the native kill_on_drop doesn't work reliably causing orphaned processes and memory leaks
        if let Some(mut child) = self.child.take() {
            tokio::spawn(async move {
                let _ = workspace_utils::process::kill_process_group(&mut child).await;
            });
        }
    }
}

type ServerPassword = String;

impl Opencode {
    fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
        let builder = CommandBuilder::new("opencode")
            // Pass hostname/port as separate args so OpenCode treats them as explicitly set
            // (it checks `process.argv.includes(\"--port\")` / `\"--hostname\"`).
            .extend_params(["serve", "--hostname", "127.0.0.1", "--port", "0"]);
        apply_overrides(builder, &self.cmd)
    }

    /// Compute a cache key for model context windows based on configuration that can affect the list of available models.
    fn compute_models_cache_key(&self) -> String {
        serde_json::to_string(&self.cmd).unwrap_or_default()
    }

    /// Common boilerplate for spawning an OpenCode server process.
    async fn spawn_server_process(
        &self,
        current_dir: &Path,
        env: &ExecutionEnv,
    ) -> Result<(AsyncGroupChild, ServerPassword), ExecutorError> {
        let command_parts = self.build_command_builder()?.build_initial()?;
        let (program_path, args) = command_parts.into_resolved().await?;

        let server_password = generate_server_password();

        // debug! (not info!) — env.provider_vars contains API keys.
        tracing::debug!(
            program = %program_path.display(),
            args = ?args,
            cwd = %current_dir.display(),
            env = ?env.vars,
            provider_env = ?env.provider_vars,
            "Spawning OpenCode server"
        );

        let mut command = Command::new(program_path);
        command
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(current_dir)
            .env("NPM_CONFIG_LOGLEVEL", "error")
            .env("NODE_NO_WARNINGS", "1")
            .env("NO_COLOR", "1")
            .env("OPENCODE_SERVER_USERNAME", "opencode")
            .env("OPENCODE_SERVER_PASSWORD", &server_password)
            .args(&args);

        env.clone()
            .with_profile(&self.cmd)
            .apply_to_command(&mut command);

        let child = command.group_spawn_no_window()?;

        Ok((child, server_password))
    }

    /// Handles process spawning, waiting for the server URL
    async fn spawn_server(
        &self,
        current_dir: &Path,
        env: &ExecutionEnv,
    ) -> Result<OpencodeServer, ExecutorError> {
        let (mut child, server_password) = self.spawn_server_process(current_dir, env).await?;
        let server_stdout = child.inner().stdout.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::other("OpenCode server missing stdout"))
        })?;

        let base_url = wait_for_server_url(server_stdout, None).await?;

        Ok(OpencodeServer {
            child: Some(child),
            base_url,
            server_password,
        })
    }

    async fn spawn_inner(
        &self,
        current_dir: &Path,
        prompt: &str,
        resume_session: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let slash_command = OpencodeSlashCommand::parse(prompt);
        let combined_prompt = if slash_command.is_some() {
            prompt.to_string()
        } else {
            self.append_prompt.combine_prompt(prompt)
        };

        let (mut child, server_password) = self.spawn_server_process(current_dir, env).await?;
        let server_stdout = child.inner().stdout.take().ok_or_else(|| {
            ExecutorError::Io(std::io::Error::other("OpenCode server missing stdout"))
        })?;

        let stdout = create_stdout_pipe_writer(&mut child)?;
        let log_writer = LogWriter::new(stdout);

        let (exit_signal_tx, exit_signal_rx) = tokio::sync::oneshot::channel();
        let cancel = tokio_util::sync::CancellationToken::new();

        // Prepare config values that will be moved into the spawned task
        let directory = current_dir.to_string_lossy().to_string();
        let approvals = self.approvals.clone();
        let model = self.model.clone();
        let model_variant = self.variant.clone();
        let agent = self.agent.clone();
        let auto_approve = self.auto_approve;
        let resume_session_id = resume_session.map(|s| s.to_string());
        let models_cache_key = self.compute_models_cache_key();
        let cancel_for_task = cancel.clone();
        let commit_reminder = env.commit_reminder;
        let commit_reminder_prompt = env.commit_reminder_prompt.clone();
        let repo_context = env.repo_context.clone();
        let outcome_for_task = OutcomeSink::default();

        tokio::spawn(async move {
            // Wait for server to print listening URL
            let base_url = match wait_for_server_url(server_stdout, Some(log_writer.clone())).await
            {
                Ok(url) => url,
                Err(err) => {
                    let _ = log_writer
                        .log_error(format!("OpenCode startup error: {err}"))
                        .await;
                    outcome_for_task.record(outcome::startup_failure());
                    let _ =
                        exit_signal_tx.send(ExecutorExitResult::Failure(outcome_for_task.take()));
                    return;
                }
            };

            let config = RunConfig {
                base_url,
                directory,
                prompt: combined_prompt,
                resume_session_id,
                model,
                model_variant,
                agent,
                approvals,
                auto_approve,
                server_password,
                models_cache_key,
                commit_reminder,
                commit_reminder_prompt,
                repo_context,
                outcome: outcome_for_task.clone(),
            };

            let result = match slash_command {
                Some(command) => {
                    run_slash_command(config, log_writer.clone(), command, cancel_for_task).await
                }
                None => run_session(config, log_writer.clone(), cancel_for_task).await,
            };
            let exit_result = match result {
                Ok(()) => ExecutorExitResult::Success,
                Err(err) => {
                    let _ = log_writer
                        .log_error(format!("OpenCode executor error: {err}"))
                        .await;
                    ExecutorExitResult::Failure(outcome_for_task.take())
                }
            };
            let _ = exit_signal_tx.send(exit_result);
        });

        Ok(SpawnedChild {
            child,
            exit_signal: Some(exit_signal_rx),
            cancel: Some(cancel),
        })
    }

    /// Transform raw model data into ModelInfo structs.
    fn transform_models(
        &self,
        models: &std::collections::HashMap<String, ProviderModelInfo>,
        provider_id: &str,
    ) -> Vec<ModelInfo> {
        let mut ordered = models.values().collect::<Vec<_>>();
        ordered.sort_by(|a, b| match (&a.release_date, &b.release_date) {
            (Some(a_date), Some(b_date)) => b_date.cmp(a_date),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.name.cmp(&b.name),
        });

        ordered
            .into_iter()
            .map(|m| {
                let reasoning_options = m
                    .variants
                    .as_ref()
                    .map(|variants| ReasoningOption::from_names(variants.keys().cloned()))
                    .unwrap_or_default();

                ModelInfo {
                    id: format!("{}/{}", provider_id, m.id),
                    name: m.name.clone(),
                    provider_id: Some(provider_id.to_string()),
                    reasoning_options,
                }
            })
            .collect()
    }
}

fn map_opencode_agents(agents: &[SDKAgentInfo]) -> Vec<AgentInfo> {
    let default_agent_name = if agents
        .iter()
        .any(|a| a.name.eq_ignore_ascii_case("sisyphus"))
    {
        "sisyphus"
    } else {
        "build"
    };

    agents
        .iter()
        .map(|agent| AgentInfo {
            id: agent.name.clone(),
            label: agent.name.to_case(Case::Title),
            description: agent.description.clone(),
            is_default: agent.name.eq_ignore_ascii_case(default_agent_name),
        })
        .collect()
}

fn format_tail(captured: Vec<String>) -> String {
    captured
        .into_iter()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

async fn wait_for_server_url(
    stdout: tokio::process::ChildStdout,
    log_writer: Option<LogWriter>,
) -> Result<String, ExecutorError> {
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let mut captured: Vec<String> = Vec::new();

    loop {
        if tokio::time::Instant::now() > deadline {
            return Err(ExecutorError::Io(std::io::Error::other(format!(
                "Timed out waiting for OpenCode server to print listening URL.\nServer output tail:\n{}",
                format_tail(captured)
            ))));
        }

        let line = match tokio::time::timeout_at(deadline, lines.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => {
                return Err(ExecutorError::Io(std::io::Error::other(format!(
                    "OpenCode server exited before printing listening URL.\nServer output tail:\n{}",
                    format_tail(captured)
                ))));
            }
            Ok(Err(err)) => return Err(ExecutorError::Io(err)),
            Err(_) => continue,
        };

        if let Some(log_writer) = &log_writer {
            log_writer
                .log_event(&OpencodeExecutorEvent::StartupLog {
                    message: line.clone(),
                })
                .await?;
        }
        if captured.len() < 64 {
            captured.push(line.clone());
        }

        if let Some(url) = line.trim().strip_prefix("opencode server listening on ") {
            // Keep draining stdout to avoid backpressure on the server, but don't block startup.
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(lines.into_inner()).lines();
                while let Ok(Some(_)) = lines.next_line().await {}
            });
            return Ok(url.trim().to_string());
        }
    }
}

fn default_discovered_options() -> crate::executor_discovery::ExecutorDiscoveredOptions {
    use crate::{
        executor_discovery::ExecutorDiscoveredOptions, model_selector::ModelSelectorConfig,
    };
    ExecutorDiscoveredOptions {
        model_selector: ModelSelectorConfig {
            providers: vec![],
            models: vec![],
            default_model: None,
            agents: vec![],
            permissions: vec![
                PermissionPolicy::BypassPermissions,
                PermissionPolicy::Supervised,
            ],
        },
        slash_commands: hardcoded_slash_commands(),
        loading_models: false,
        loading_agents: false,
        loading_slash_commands: false,
        error: None,
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for Opencode {
    fn apply_overrides(&mut self, executor_config: &ExecutorConfig) {
        if let Some(model_id) = &executor_config.model_id
            && !model_id.is_empty()
        {
            self.model = Some(model_id.clone());
        }

        if let Some(agent_id) = &executor_config.agent_id {
            self.agent = Some(agent_id.clone());
        }

        if let Some(permission_policy) = executor_config.permission_policy.clone() {
            self.auto_approve = matches!(permission_policy, PermissionPolicy::BypassPermissions);
        }

        if let Some(reasoning_id) = &executor_config.reasoning_id {
            self.variant = Some(reasoning_id.clone());
        }
    }

    fn use_approvals(&mut self, approvals: Arc<dyn ExecutorApprovalService>) {
        self.approvals = Some(approvals);
    }

    fn brokers_approvals(&self) -> bool {
        true
    }

    fn provider_slot(&self) -> &'static str {
        "opencode"
    }

    /// OpenCode's session API takes `(provider_id, model_id)` and its only
    /// signal for the split is the first `/`. A record's enabled models are
    /// stored as raw vendor ids (`openai/gpt-5.4-mini` for OpenRouter), which
    /// would split to the wrong provider, so prefix the record slug — the same
    /// key [`Self::build_provider_injection`] registers the provider under.
    ///
    /// No-op for ambient records, whose ids already come prefixed out of
    /// executor discovery, and for ids that already carry the slug.
    fn provider_model_id(&self, ctx: &ProviderContext) -> String {
        let prefix = format!("{}/", ctx.slug);
        if ctx.ambient || ctx.model_id.starts_with(&prefix) {
            return ctx.model_id.clone();
        }
        format!("{prefix}{}", ctx.model_id)
    }

    /// Ship the provider/model config through `OPENCODE_CONFIG_CONTENT`, which
    /// OpenCode loads after the user's global and project configs — so our
    /// keys win without touching `~/.config/opencode/`.
    ///
    /// `enabled_models` must be non-empty: OpenCode's runtime deletes any
    /// provider whose `models` map is empty (`provider.ts:1393`), silently
    /// breaking the agent.
    ///
    /// `baseURL` and `apiKey` are inserted after the slot's own `options`, and
    /// `OPENCODE_CONFIG_CONTENT` after its own `env`, so a vendor-quirk entry
    /// can never shadow the endpoint or the credential.
    fn build_provider_injection(
        &self,
        ctx: &ProviderContext,
    ) -> Result<ProviderInjection, ProviderInjectionError> {
        let api_key = ctx.require_api_key(BaseCodingAgent::Opencode)?;
        let base_url = ctx.require_base_url(BaseCodingAgent::Opencode)?;
        if ctx.enabled_models.is_empty() {
            return Err(ProviderInjectionError::EmptyEnabledModels);
        }

        let mut options = ctx.payload.extra_object("options");
        options.insert("baseURL".to_string(), Value::String(base_url.to_string()));
        options.insert("apiKey".to_string(), Value::String(api_key.to_string()));

        let models: Map<String, Value> = ctx
            .enabled_models
            .iter()
            .map(|id| (id.clone(), Value::Object(Map::new())))
            .collect();

        let mut provider = Map::new();
        if let Some(npm) = ctx.payload.extra_str("npm") {
            provider.insert("npm".to_string(), Value::String(npm.to_string()));
        }
        provider.insert("name".to_string(), Value::String(ctx.record_name.clone()));
        provider.insert("options".to_string(), Value::Object(options));
        provider.insert("models".to_string(), Value::Object(models));

        let providers = Map::from_iter([(ctx.slug.clone(), Value::Object(provider))]);
        let config = Map::from_iter([("provider".to_string(), Value::Object(providers))]);

        let mut env = ctx.payload.env.clone();
        env.insert(
            "OPENCODE_CONFIG_CONTENT".to_string(),
            serde_json::to_string(&Value::Object(config))?,
        );
        Ok(ProviderInjection::from_env(env))
    }

    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let env = setup_database_env(env);
        let env = setup_permissions_env(self.auto_approve, &env);
        let env = setup_compaction_env(self.auto_compact, &env);
        self.spawn_inner(current_dir, prompt, None, &env).await
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        _reset_to_message_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let env = setup_database_env(env);
        let env = setup_permissions_env(self.auto_approve, &env);
        let env = setup_compaction_env(self.auto_compact, &env);
        self.spawn_inner(current_dir, prompt, Some(session_id), &env)
            .await
    }

    fn normalize_logs(
        &self,
        msg_store: Arc<MsgStore>,
        worktree_path: &Path,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        normalize_logs::normalize_logs(msg_store, worktree_path)
    }

    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        #[cfg(not(windows))]
        {
            let base_dirs = xdg::BaseDirectories::with_prefix("opencode");
            // First try opencode.json, then opencode.jsonc
            base_dirs
                .get_config_file("opencode.json")
                .filter(|p| p.exists())
                .or_else(|| base_dirs.get_config_file("opencode.jsonc"))
        }
        #[cfg(windows)]
        {
            let config_dir = std::env::var("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .ok()
                .or_else(|| dirs::home_dir().map(|p| p.join(".config")))
                .map(|p| p.join("opencode"))?;

            let path = Some(config_dir.join("opencode.json"))
                .filter(|p| p.exists())
                .unwrap_or_else(|| config_dir.join("opencode.jsonc"));
            Some(path)
        }
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        let mcp_config_found = self
            .default_mcp_config_path()
            .map(|p| p.exists())
            .unwrap_or(false);

        // Check multiple installation indicator paths:
        // 1. XDG config dir: $XDG_CONFIG_HOME/opencode
        // 2. XDG data dir: $XDG_DATA_HOME/opencode
        // 3. XDG state dir: $XDG_STATE_HOME/opencode
        // 4. OpenCode CLI home: ~/.opencode
        #[cfg(not(windows))]
        let installation_indicator_found = {
            let base_dirs = xdg::BaseDirectories::with_prefix("opencode");

            let config_dir_exists = base_dirs
                .get_config_home()
                .map(|config| config.exists())
                .unwrap_or(false);

            let data_dir_exists = base_dirs
                .get_data_home()
                .map(|data| data.exists())
                .unwrap_or(false);

            let state_dir_exists = base_dirs
                .get_state_home()
                .map(|state| state.exists())
                .unwrap_or(false);

            config_dir_exists || data_dir_exists || state_dir_exists
        };

        #[cfg(windows)]
        let installation_indicator_found = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(std::path::PathBuf::from)
            .and_then(|p| p.join("opencode").exists().then_some(()))
            .or_else(|| {
                dirs::home_dir()
                    .and_then(|p| p.join(".config").join("opencode").exists().then_some(()))
            })
            .is_some();

        let home_opencode_exists = dirs::home_dir()
            .map(|home| home.join(".opencode").exists())
            .unwrap_or(false);

        if mcp_config_found || installation_indicator_found || home_opencode_exists {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }

    async fn discover_options(
        &self,
        workdir: Option<&Path>,
        repo_path: Option<&Path>,
    ) -> Result<futures::stream::BoxStream<'static, json_patch::Patch>, ExecutorError> {
        use crate::{
            executor_discovery::ExecutorConfigCacheKey, executors::utils::executor_options_cache,
        };

        let cache = executor_options_cache();
        let cmd_key = self.compute_models_cache_key();
        let base_executor = BaseCodingAgent::Opencode;

        let (target_path, initial_options) = if let Some(wd) = workdir {
            let wd_buf = wd.to_path_buf();
            let target_key =
                ExecutorConfigCacheKey::new(Some(&wd_buf), cmd_key.clone(), base_executor);
            if let Some(cached) = cache.get(&target_key) {
                return Ok(Box::pin(futures::stream::once(async move {
                    patch::executor_discovered_options(cached.as_ref().clone().with_loading(false))
                })));
            }
            let provisional = repo_path
                .and_then(|rp| {
                    let rp_buf = rp.to_path_buf();
                    let repo_key =
                        ExecutorConfigCacheKey::new(Some(&rp_buf), cmd_key.clone(), base_executor);
                    cache.get(&repo_key)
                })
                .or_else(|| {
                    let global_key =
                        ExecutorConfigCacheKey::new(None, cmd_key.clone(), base_executor);
                    cache.get(&global_key)
                });
            (
                Some(wd.to_path_buf()),
                provisional
                    .map(|p| p.as_ref().clone().with_loading(true))
                    .unwrap_or_else(|| default_discovered_options().with_loading(true)),
            )
        } else if let Some(rp) = repo_path {
            let rp_buf = rp.to_path_buf();
            let target_key =
                ExecutorConfigCacheKey::new(Some(&rp_buf), cmd_key.clone(), base_executor);
            if let Some(cached) = cache.get(&target_key) {
                return Ok(Box::pin(futures::stream::once(async move {
                    patch::executor_discovered_options(cached.as_ref().clone().with_loading(false))
                })));
            }
            let global_key = ExecutorConfigCacheKey::new(None, cmd_key.clone(), base_executor);
            let provisional = cache.get(&global_key);
            (
                Some(rp.to_path_buf()),
                provisional
                    .map(|p| p.as_ref().clone().with_loading(true))
                    .unwrap_or_else(|| default_discovered_options().with_loading(true)),
            )
        } else {
            let global_key = ExecutorConfigCacheKey::new(None, cmd_key.clone(), base_executor);
            if let Some(cached) = cache.get(&global_key) {
                return Ok(Box::pin(futures::stream::once(async move {
                    patch::executor_discovered_options(cached.as_ref().clone().with_loading(false))
                })));
            }
            (None, default_discovered_options().with_loading(true))
        };

        let initial_patch = patch::executor_discovered_options(initial_options);

        let this = self.clone();
        let cmd_key_for_discovery = cmd_key.clone();

        let discovery_stream = async_stream::stream! {
            let discovery_path = target_path.as_deref().unwrap_or(Path::new(".")).to_path_buf();
            let mut final_options = default_discovered_options();

            let env = ExecutionEnv::new(RepoContext::default(), false, String::new());
            let env = setup_permissions_env(this.auto_approve, &env);

            let server = match this.spawn_server(&discovery_path, &env).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Failed to spawn OpenCode server: {}", e);
                    yield patch::discovery_error(e.to_string());
                    return;
                }
            };

            let directory = discovery_path.to_string_lossy();
            let client = match build_authenticated_client(&directory, &server.server_password) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("Failed to build authenticated client: {}", e);
                    yield patch::discovery_error(e.to_string());
                    return;
                }
            };

            let base_url = server.base_url.clone();
            let directory_str = directory.to_string();

            let providers_future = list_providers(&client, &base_url, &directory_str);
            let agents_future = list_agents(&client, &base_url, &directory_str);
            let commands_future = list_commands(&client, &base_url, &directory_str);

            let config_future = async {
                let resp = client
                    .get(format!("{}/config", base_url))
                    .query(&[("directory", &directory_str)])
                    .send()
                    .await
                    .map_err(|e| ExecutorError::Io(std::io::Error::other(format!("HTTP request failed: {e}"))))?;

                if resp.status().is_success() {
                    resp.json::<Config>().await.map_err(|e| {
                        ExecutorError::Io(std::io::Error::other(format!(
                            "Failed to parse config response: {e}"
                        )))
                    })
                } else {
                    Ok(Config { model: None })
                }
            };

            let (providers_result, agents_result, commands_result, config_result) =
                tokio::join!(providers_future, agents_future, commands_future, config_future);

            match providers_result {
                Ok(data) => {
                    models::seed_context_windows_cache(
                        &cmd_key_for_discovery,
                        models::extract_context_windows(&data),
                    );

                    final_options.model_selector.providers = data
                        .all
                        .iter()
                        .filter(|p| data.connected.contains(&p.id))
                        .map(|p| ModelProvider {
                            id: p.id.clone(),
                            name: p.name.clone(),
                        })
                        .collect();

                    final_options.model_selector.models = data
                        .all
                        .iter()
                        .filter(|p| data.connected.contains(&p.id))
                        .flat_map(|p| this.transform_models(&p.models, &p.id))
                        .collect();

                    yield patch::update_providers(final_options.model_selector.providers.clone());
                    yield patch::update_models(final_options.model_selector.models.clone());
                    yield patch::models_loaded();
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch OpenCode providers: {}", e);
                }
            }

            match config_result {
                Ok(config) => {
                    final_options.model_selector.default_model = config.model;
                    yield patch::update_default_model(final_options.model_selector.default_model.clone());
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch OpenCode config: {}", e);
                }
            }

            match agents_result {
                Ok(agents) => {
                    final_options.model_selector.agents = map_opencode_agents(&agents);
                    yield patch::update_agents(final_options.model_selector.agents.clone());
                    yield patch::agents_loaded();
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch OpenCode agents: {}", e);
                }
            }

            match commands_result {
                Ok(commands) => {
                    let defaults = hardcoded_slash_commands();
                    let mut seen: std::collections::HashSet<String> =
                        defaults.iter().map(|cmd| cmd.name.clone()).collect();
                    let discovered: Vec<SlashCommandDescription> = commands
                        .into_iter()
                        .map(|cmd| SlashCommandDescription {
                            name: cmd.name.trim_start_matches('/').to_string(),
                            description: cmd.description,
                        })
                        .filter(|cmd| seen.insert(cmd.name.clone()))
                        .chain(defaults)
                        .collect();
                    final_options.slash_commands = reorder_slash_commands(discovered);
                    yield patch::update_slash_commands(final_options.slash_commands.clone());
                    yield patch::slash_commands_loaded();
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch OpenCode commands: {}", e);
                    final_options.slash_commands = hardcoded_slash_commands();
                    yield patch::update_slash_commands(final_options.slash_commands.clone());
                    yield patch::slash_commands_loaded();
                }
            }

            let cache = executor_options_cache();
            if let Some(path) = &target_path {
                let target_cache_key = ExecutorConfigCacheKey::new(
                    Some(path),
                    cmd_key_for_discovery.clone(),
                    BaseCodingAgent::Opencode,
                );
                cache.put(target_cache_key, final_options.clone());
            }
            let global_cache_key = ExecutorConfigCacheKey::new(
                None,
                cmd_key_for_discovery,
                BaseCodingAgent::Opencode,
            );
            cache.put(global_cache_key, final_options);
        };

        Ok(Box::pin(
            futures::stream::once(async move { initial_patch }).chain(discovery_stream),
        ))
    }

    fn get_preset_options(&self) -> ExecutorConfig {
        ExecutorConfig {
            executor: BaseCodingAgent::Opencode,
            variant: None,
            model_id: self.model.clone(),
            agent_id: self.agent.clone(),
            reasoning_id: self.variant.clone(),
            permission_policy: Some(if self.auto_approve {
                PermissionPolicy::BypassPermissions
            } else {
                PermissionPolicy::Supervised
            }),
        }
    }
}

fn default_to_true() -> bool {
    true
}

/// Pin OpenCode's session database to a cdesktop-owned path.
///
/// OpenCode resolves its database from the ambient XDG data dir, which cdesktop
/// shares with whatever else launched it - a database another tool has left
/// mid-migration takes down every spawn that touches it. Owning the path makes
/// that unreachable.
///
/// The path is per-installation, not per-spawn: a follow-up starts a *new*
/// server and resumes by session id, so the session must still be there. An
/// explicit `OPENCODE_DB` still wins, which is what makes the behaviour
/// testable and lets an operator relocate it.
fn setup_database_env(env: &ExecutionEnv) -> ExecutionEnv {
    if env.get("OPENCODE_DB").is_some() {
        return env.clone();
    }

    let database_path = workspace_utils::assets::asset_dir()
        .join("opencode")
        .join("opencode.db");

    if let Some(parent) = database_path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        // Leaving OPENCODE_DB unset falls back to the ambient database, which
        // still works whenever it is healthy.
        tracing::warn!(
            path = %parent.display(),
            "Could not create the OpenCode database directory: {err}"
        );
        return env.clone();
    }

    let mut env = env.clone();
    env.insert("OPENCODE_DB", database_path.to_string_lossy().as_ref());
    env
}

fn setup_permissions_env(auto_approve: bool, env: &ExecutionEnv) -> ExecutionEnv {
    let mut env = env.clone();

    let permissions = match env.get("OPENCODE_PERMISSION") {
        Some(existing) => Some(existing.to_string()),
        None => build_default_permissions(auto_approve),
    };

    if let Some(permissions) = permissions {
        env.insert("OPENCODE_PERMISSION", &permissions);
    }
    env
}

fn build_default_permissions(auto_approve: bool) -> Option<String> {
    if auto_approve {
        None
    } else {
        Some(r#"{"edit":"ask","bash":"ask","webfetch":"ask","doom_loop":"ask","external_directory":"ask","question":"allow"}"#.to_string())
    }
}

fn setup_compaction_env(auto_compact: bool, env: &ExecutionEnv) -> ExecutionEnv {
    if !auto_compact {
        return env.clone();
    }

    let mut env = env.clone();
    // Read OPENCODE_CONFIG_CONTENT from whichever slot has it (provider_vars
    // wins, since Phase D writes the provider applier's JSON there), then
    // ALWAYS write the merged result to `provider_vars`. `provider_vars` is
    // applied last in `apply_to_command`, so this lets the compaction config
    // win over profile env even when no provider applier ran (Default
    // provider). One write target keeps the env-write contract simple and
    // prevents future writers from racing.
    let existing = env
        .provider_vars
        .get("OPENCODE_CONFIG_CONTENT")
        .cloned()
        .or_else(|| env.vars.get("OPENCODE_CONFIG_CONTENT").cloned());
    let merged = merge_compaction_config(existing.as_deref());
    env.provider_vars
        .insert("OPENCODE_CONFIG_CONTENT".to_string(), merged);
    env
}

fn merge_compaction_config(existing_json: Option<&str>) -> String {
    let mut config: Map<String, Value> = existing_json
        .and_then(|value| serde_json::from_str(value.trim()).ok())
        .unwrap_or_default();

    let mut compaction = config
        .remove("compaction")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    compaction.insert("auto".to_string(), Value::Bool(true));
    config.insert("compaction".to_string(), Value::Object(compaction));

    serde_json::to_string(&config).unwrap_or_else(|_| r#"{"compaction":{"auto":true}}"#.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{env::RepoContext, provider::ProviderPayload};

    fn env() -> ExecutionEnv {
        ExecutionEnv::new(RepoContext::default(), false, String::new())
    }

    fn context(ambient: bool, slug: &str, model_id: &str) -> ProviderContext {
        ProviderContext {
            ambient,
            record_name: "Test Provider".to_string(),
            slug: slug.to_string(),
            api_key: Some("sk-real".to_string()),
            payload: ProviderPayload::from_slot(Some(&json!({
                "baseUrl": "https://openrouter.ai/api/v1",
            }))),
            enabled_models: vec![model_id.to_string()],
            model_id: model_id.to_string(),
        }
    }

    /// The slug/model split OpenCode's session API needs is this adapter's
    /// business. It has to agree with the key the same adapter registers the
    /// provider under, which is why both live here.
    #[test]
    fn model_ids_carry_the_slug_opencode_splits_on() {
        let agent = serde_json::from_value::<Opencode>(json!({})).unwrap();

        let ctx = context(false, "openrouter", "openai/gpt-5.4-mini");
        assert_eq!(
            agent.provider_model_id(&ctx),
            "openrouter/openai/gpt-5.4-mini"
        );

        // Already prefixed: idempotent, not double-prefixed.
        let ctx = context(false, "openrouter", "openrouter/openai/gpt-5.4-mini");
        assert_eq!(
            agent.provider_model_id(&ctx),
            "openrouter/openai/gpt-5.4-mini"
        );

        // Ambient ids already come prefixed out of executor discovery.
        let ctx = context(true, "openrouter", "anthropic/claude-opus-4-5");
        assert_eq!(agent.provider_model_id(&ctx), "anthropic/claude-opus-4-5");
    }

    /// The registered provider key and the model-id prefix are the same slug —
    /// if they drifted, every session would ask a provider that does not exist.
    #[test]
    fn registered_provider_key_matches_the_model_id_prefix() {
        let agent = serde_json::from_value::<Opencode>(json!({})).unwrap();
        let ctx = context(false, "custom", "gpt-4");

        let env = agent
            .build_provider_injection(&ctx)
            .expect("injection builds")
            .env
            .expect("non-ambient record emits env");
        let config: Value =
            serde_json::from_str(&env["OPENCODE_CONFIG_CONTENT"]).expect("config is JSON");

        let model_id = agent.provider_model_id(&ctx);
        let (slug, _) = model_id.split_once('/').expect("id carries a slug");
        assert!(config["provider"][slug].is_object());
    }

    #[test]
    fn spawns_own_the_opencode_database_by_default() {
        let pinned = setup_database_env(&env());

        let path = std::path::PathBuf::from(
            pinned
                .get("OPENCODE_DB")
                .expect("spawns must not inherit the ambient database"),
        );
        assert!(path.starts_with(workspace_utils::assets::asset_dir()));
        assert!(
            path.parent().is_some_and(std::path::Path::is_dir),
            "the database directory must exist before OpenCode opens it"
        );
    }

    #[test]
    fn a_follow_up_resumes_against_the_same_database() {
        // Resume starts a fresh server and asks it for an existing session, so
        // the two spawns have to agree on where sessions live.
        assert_eq!(
            setup_database_env(&env()).get("OPENCODE_DB"),
            setup_database_env(&env()).get("OPENCODE_DB")
        );
    }

    #[test]
    fn an_explicit_database_is_left_alone() {
        let mut configured = env();
        configured.insert("OPENCODE_DB", "/tmp/operator-chosen/opencode.db");

        assert_eq!(
            setup_database_env(&configured).get("OPENCODE_DB").unwrap(),
            "/tmp/operator-chosen/opencode.db"
        );
    }
}
