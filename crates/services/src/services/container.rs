use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Error as AnyhowError, anyhow};
use async_trait::async_trait;
use db::{
    DBService,
    models::{
        coding_agent_turn::{CodingAgentTurn, CreateCodingAgentTurn},
        execution_process::{
            CreateExecutionProcess, ExecutionContext, ExecutionProcess, ExecutionProcessError,
            ExecutionProcessRunReason, ExecutionProcessStatus,
        },
        execution_process_repo_state::{
            CreateExecutionProcessRepoState, ExecutionProcessRepoState,
        },
        metered_approval::{MeteredApproval, MeteredApprovalPolicy, MeteredGateDecision},
        repo::Repo,
        routine_run::RoutineRun,
        session::{CreateSession, Session, SessionError},
        session_command::{SessionCommand, SessionCommandConfig},
        workspace::{Workspace, WorkspaceError, WorkspaceSource},
        workspace_repo::WorkspaceRepo,
    },
};
#[cfg(feature = "qa-mode")]
use executors::executors::qa_mock::QaMockExecutor;
#[cfg(not(feature = "qa-mode"))]
use executors::profile::ExecutorConfigs;
use executors::{
    actions::{
        ExecutorAction, ExecutorActionType,
        coding_agent_follow_up::CodingAgentFollowUpRequest,
        coding_agent_initial::{CodingAgentInitialRequest, PromptKind},
        script::{ScriptContext, ScriptRequest, ScriptRequestLanguage},
    },
    executors::{ExecutorError, StandardCodingAgentExecutor},
    logs::{
        NormalizedEntry, NormalizedEntryError, NormalizedEntryType,
        utils::{
            ConversationPatch,
            patch::{fix_patch_ops, is_add_or_replace, patch_entry_path},
        },
    },
    profile::{ExecutorConfig, ExecutorProfileId},
    provider::ProviderInjection,
};
use futures::{StreamExt, future, stream::BoxStream};
use git::{GitService, GitServiceError};
use json_patch::Patch;
use sqlx::Error as SqlxError;
use thiserror::Error;
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinHandle,
};
use utils::{
    log_msg::LogMsg,
    msg_store::MsgStore,
    text::{git_branch_id, short_uuid},
};
use uuid::Uuid;
use worktree_manager::WorktreeError;

use crate::services::{auth_binding, execution_process, notification::NotificationService};
pub type ContainerRef = String;

fn max_running_agents() -> i64 {
    std::env::var("CDESKTOP_MAX_RUNNING_AGENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value: &i64| *value > 0)
        .unwrap_or(4)
}

#[derive(Debug, Error)]
pub enum ContainerError {
    #[error(transparent)]
    GitServiceError(#[from] GitServiceError),
    #[error(transparent)]
    Sqlx(#[from] SqlxError),
    #[error(transparent)]
    ExecutorError(#[from] ExecutorError),
    #[error(transparent)]
    Worktree(#[from] WorktreeError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    ExecutionProcess(#[from] ExecutionProcessError),
    #[error("Io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to kill process: {0}")]
    KillFailed(std::io::Error),
    #[error(transparent)]
    Other(#[from] AnyhowError), // Catches any unclassified errors
}

#[async_trait]
pub trait ContainerService {
    fn msg_stores(&self) -> &Arc<RwLock<HashMap<Uuid, Arc<MsgStore>>>>;

    fn db(&self) -> &DBService;

    fn git(&self) -> &GitService;

    fn notification_service(&self) -> &NotificationService;

    fn scheduler_lock(&self) -> &Mutex<()>;

    async fn touch(&self, workspace: &Workspace) -> Result<(), ContainerError>;

    async fn workspace_to_current_dir(&self, workspace: &Workspace) -> PathBuf;

    async fn discover_executor_options(
        &self,
        executor_profile_id: ExecutorProfileId,
        session_id: Option<Uuid>,
        workspace_id: Option<Uuid>,
        repo_id: Option<Uuid>,
    ) -> Result<Option<BoxStream<'static, Patch>>, ContainerError> {
        let (workdir, repo_path) = if let Some(session_id) = session_id {
            let session = Session::find_by_id(&self.db().pool, session_id)
                .await?
                .ok_or(SqlxError::RowNotFound)?;

            if let Some(workspace_id) = workspace_id
                && session.workspace_id != workspace_id
            {
                return Err(ContainerError::Other(anyhow!(
                    "Session does not belong to workspace"
                )));
            }

            let workspace = Workspace::find_by_id(&self.db().pool, session.workspace_id)
                .await?
                .ok_or(SqlxError::RowNotFound)?;

            let repos =
                WorkspaceRepo::find_repos_for_workspace(&self.db().pool, session.workspace_id)
                    .await
                    .unwrap_or_default();

            let workspace_path = if workspace.use_worktree {
                let container_ref = match workspace.container_ref.as_deref() {
                    Some(container_ref) if !container_ref.is_empty() => container_ref.to_string(),
                    _ => self.ensure_container_exists(&workspace).await?,
                };

                if container_ref.is_empty() {
                    return Err(ContainerError::Other(anyhow!("Workspace path is empty")));
                }

                PathBuf::from(container_ref)
            } else {
                let repo = repos.first().ok_or_else(|| {
                    ContainerError::Other(anyhow!(
                        "Worktree-disabled workspace has no attached repo"
                    ))
                })?;
                repo.path.clone()
            };

            let workdir = match session.agent_working_dir.as_deref() {
                Some(dir) if !dir.is_empty() => Some(workspace_path.join(dir)),
                _ => Some(workspace_path),
            };

            let repo_path = if repos.len() == 1 {
                Some(repos[0].path.clone())
            } else {
                None
            };

            (workdir, repo_path)
        } else if workspace_id.is_some() {
            return Err(ContainerError::Other(anyhow!(
                "session_id is required when workspace_id is provided"
            )));
        } else if let Some(repo_id) = repo_id {
            let repo = Repo::find_by_id(&self.db().pool, repo_id)
                .await
                .ok()
                .flatten()
                .map(|repo| repo.path);
            (None, repo)
        } else {
            (None, None)
        };

        #[cfg(feature = "qa-mode")]
        {
            let _ = executor_profile_id;
            let _ = workdir;
            let _ = repo_path;
            return Ok(None);
        }
        #[cfg(not(feature = "qa-mode"))]
        {
            let executor =
                ExecutorConfigs::get_cached().get_coding_agent_or_default(&executor_profile_id);

            // Spawn background task to refresh global cache for this executor
            let base_agent = executors::executors::BaseCodingAgent::from(&executor);
            executors::executors::utils::spawn_global_cache_refresh_for_agent(base_agent);

            let stream = executor
                .discover_options(workdir.as_deref(), repo_path.as_deref())
                .await?;
            Ok(Some(stream))
        }
    }

    async fn store_db_stream_handle(&self, id: Uuid, handle: JoinHandle<()>);

    async fn take_db_stream_handle(&self, id: &Uuid) -> Option<JoinHandle<()>>;

    async fn create(&self, workspace: &Workspace) -> Result<ContainerRef, ContainerError>;

    async fn kill_all_running_processes(&self) -> Result<(), ContainerError>;

    async fn delete(&self, workspace: &Workspace) -> Result<(), ContainerError>;

    /// A context is finalized when
    /// - Always when the execution process has failed or been killed
    /// - Never when the run reason is DevServer
    /// - Never when a setup script has no next_action (parallel mode)
    /// - The next action is None (no follow-up actions)
    fn should_finalize(&self, ctx: &ExecutionContext) -> bool {
        // Never finalize DevServer processes
        if matches!(
            ctx.execution_process.run_reason,
            ExecutionProcessRunReason::DevServer
        ) {
            return false;
        }

        // Never finalize setup scripts without a next_action (parallel mode).
        // In sequential mode, setup scripts have next_action pointing to coding agent,
        // so they won't finalize anyway (handled by next_action.is_none() check below).
        let action = ctx.execution_process.executor_action().unwrap();
        if matches!(
            ctx.execution_process.run_reason,
            ExecutionProcessRunReason::SetupScript
        ) && action.next_action.is_none()
        {
            return false;
        }

        // Always finalize failed or killed executions, regardless of next action
        if matches!(
            ctx.execution_process.status,
            ExecutionProcessStatus::Failed | ExecutionProcessStatus::Killed
        ) {
            return true;
        }

        // Otherwise, finalize only if no next action
        action.next_action.is_none()
    }

    /// Finalize workspace execution by sending notifications
    async fn finalize_task(&self, ctx: &ExecutionContext) {
        // Routine completion side-effect: when a coding-agent process for a
        // routine-owned workspace terminates, flip the matching routine_run
        // to Done/Failed so the detail page stops showing "running".
        if matches!(
            ctx.execution_process.run_reason,
            ExecutionProcessRunReason::CodingAgent
        ) && ctx.workspace.source == WorkspaceSource::Routine
        {
            let pool = &self.db().pool;
            match RoutineRun::find_active_by_workspace(pool, ctx.workspace.id).await {
                Ok(Some(run)) => {
                    let failed =
                        matches!(ctx.execution_process.status, ExecutionProcessStatus::Failed);
                    if let Err(e) =
                        RoutineRun::mark_done(pool, run.id, chrono::Utc::now(), failed).await
                    {
                        tracing::error!(
                            "Failed to mark routine_run {} done after workspace {} finished: {:?}",
                            run.id,
                            ctx.workspace.id,
                            e
                        );
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::error!(
                    "Failed to look up routine_run for workspace {}: {:?}",
                    ctx.workspace.id,
                    e
                ),
            }
        }

        // Skip notification if process was intentionally killed by user
        if matches!(ctx.execution_process.status, ExecutionProcessStatus::Killed) {
            return;
        }

        let workspace_name = ctx
            .workspace
            .name
            .as_deref()
            .unwrap_or(&ctx.workspace.branch);
        let title = format!("Workspace Complete: {}", workspace_name);
        let message = match ctx.execution_process.status {
            ExecutionProcessStatus::Completed => format!(
                "✅ '{}' completed successfully\nBranch: {:?}\nExecutor: {:?}",
                workspace_name, ctx.workspace.branch, ctx.session.executor
            ),
            ExecutionProcessStatus::Failed => format!(
                "❌ '{}' execution failed\nBranch: {:?}\nExecutor: {:?}",
                workspace_name, ctx.workspace.branch, ctx.session.executor
            ),
            _ => {
                tracing::warn!(
                    "Tried to notify workspace completion for {} but process is still running!",
                    ctx.workspace.id
                );
                return;
            }
        };
        self.notification_service()
            .notify(&title, &message, Some(ctx.workspace.id))
            .await;
    }

    /// Cleanup executions marked as running in the db, call at startup
    async fn cleanup_orphan_executions(&self) -> Result<(), ContainerError> {
        let running_processes = ExecutionProcess::find_running(&self.db().pool).await?;
        for process in running_processes {
            tracing::info!(
                "Found orphaned execution process {} for session {}",
                process.id,
                process.session_id
            );
            // Update the execution process status first
            if let Err(e) = ExecutionProcess::update_completion(
                &self.db().pool,
                process.id,
                ExecutionProcessStatus::Failed,
                None, // No exit code for orphaned processes
            )
            .await
            {
                tracing::error!(
                    "Failed to update orphaned execution process {} status: {}",
                    process.id,
                    e
                );
                continue;
            }
            SessionCommand::release_execution(&self.db().pool, process.id).await?;
            // Capture after-head commit OID per repository
            if let Ok(ctx) = ExecutionProcess::load_context(&self.db().pool, process.id).await {
                for repo in &ctx.repos {
                    let Some(repo_path) = ctx.workspace.execution_dir(repo) else {
                        continue;
                    };
                    if let Ok(head) = self.git().get_head_info(&repo_path)
                        && let Err(err) = ExecutionProcessRepoState::update_after_head_commit(
                            &self.db().pool,
                            process.id,
                            repo.id,
                            &head.oid,
                        )
                        .await
                    {
                        tracing::warn!(
                            "Failed to update after_head_commit for repo {} on process {}: {}",
                            repo.id,
                            process.id,
                            err
                        );
                    }
                }
            }
            // Process marked as failed
            tracing::info!("Marked orphaned execution process {} as failed", process.id);
        }
        Ok(())
    }

    async fn dispatch_pending_commands(
        &self,
        session_id: Uuid,
    ) -> Result<Option<ExecutionProcess>, ContainerError> {
        let _scheduler = self.scheduler_lock().lock().await;
        let pool = &self.db().pool;
        if ExecutionProcess::has_running_coding_agent_for_session(pool, session_id).await? {
            return Ok(None);
        }
        let max_running = max_running_agents();
        if ExecutionProcess::count_running_coding_agents(pool).await? >= max_running {
            return Ok(None);
        }

        // Metered approval gate (plan §12): consult the durable policy for
        // the head of the queue before claiming. `ask` creates/awaits its
        // durable approval, `never` blocks with a routes_exhausted record —
        // in both held cases nothing is claimed and no attempt is spent.
        let pending = SessionCommand::pending(pool, session_id).await?;
        if let Some(head) = pending.first() {
            match MeteredApproval::gate(pool, head).await? {
                MeteredGateDecision::Proceed => {}
                MeteredGateDecision::AwaitApproval | MeteredGateDecision::Blocked => {
                    return Ok(None);
                }
            }
        }

        // A crash between claim and bind leaves a claimed batch with no
        // execution row. Claims and binds only happen under this scheduler
        // lock, so any unbound claim seen here is a leftover - return it to
        // the queue before claiming again.
        SessionCommand::release_unbound(pool, session_id).await?;
        let execution_id = Uuid::new_v4();
        let commands = SessionCommand::claim_pending(pool, session_id).await?;
        let Some(first) = commands.first() else {
            return Ok(None);
        };
        // The queue can change between the gate peek and the claim (e.g. a
        // replace command). Re-verify the claimed head and release the claim
        // if its gate is not open, so a metered command can never launch
        // without its durable authorization.
        match MeteredApproval::gate(pool, first).await? {
            MeteredGateDecision::Proceed => {}
            MeteredGateDecision::AwaitApproval | MeteredGateDecision::Blocked => {
                SessionCommand::release_unbound(pool, session_id).await?;
                return Ok(None);
            }
        }
        // Metered bookkeeping for the winning claim: consume the approval
        // (allow-once) or durably record the auto-start notification.
        if let Some(metered) = &first.config.0.metered {
            match metered.policy {
                MeteredApprovalPolicy::Ask => {
                    MeteredApproval::consume_approval(pool, first.id, execution_id).await?;
                }
                MeteredApprovalPolicy::Auto => {
                    MeteredApproval::record_auto_start(
                        pool,
                        first.id,
                        execution_id,
                        metered.account_alias.as_deref(),
                    )
                    .await?;
                }
                MeteredApprovalPolicy::Never => {
                    SessionCommand::release_unbound(pool, session_id).await?;
                    return Ok(None);
                }
            }
        }
        let first_config = first.config.clone();
        let commands = commands
            .into_iter()
            .take_while(|command| command.config == first_config)
            .collect::<Vec<_>>();
        let first = &commands[0];
        let command_config = first.config.0.clone();
        let mut executor_config = command_config.executor_config.clone();
        let selected_provider_id = command_config.selected_provider_id;
        let prompt = commands
            .iter()
            .map(|command| command.body.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let result = async {
            let session = Session::find_by_id(pool, session_id)
                .await?
                .ok_or(SessionError::NotFound)?;
            let workspace = Workspace::find_by_id(pool, session.workspace_id)
                .await?
                .ok_or(WorkspaceError::WorkspaceNotFound)?;
            self.ensure_container_exists(&workspace).await?;

            let profile = executor_config.profile_id();
            let expected = ExecutionProcess::latest_executor_profile_for_session(pool, session.id)
                .await?
                .map(|value| value.executor.to_string())
                .or_else(|| session.executor.clone());
            if let Some(expected) = expected {
                let actual = profile.executor.to_string();
                if expected != actual {
                    return Err(SessionError::ExecutorMismatch { expected, actual }.into());
                }
            }
            if session.executor.is_none() {
                Session::update_executor(pool, session.id, &profile.executor.to_string()).await?;
            }

            // Resolve the auth binding to credential material immediately
            // before launch. The resolved injection stays in memory only:
            // the persisted action is stripped via `without_provider_bindings`
            // and the runtime fields are non-serializable by construction.
            let resolved =
                auth_binding::resolve_for_launch(pool, &command_config, &mut executor_config)
                    .await
                    .map_err(|error| ContainerError::Other(error.into()))?;

            let working_dir = session
                .agent_working_dir
                .as_ref()
                .filter(|dir| !dir.is_empty())
                .cloned();
            let action_type = match CodingAgentTurn::find_latest_session_info(pool, session.id)
                .await?
            {
                Some(info) => {
                    ExecutorActionType::CodingAgentFollowUpRequest(CodingAgentFollowUpRequest {
                        prompt,
                        session_id: info.session_id,
                        reset_to_message_id: None,
                        executor_config: executor_config.clone(),
                        working_dir,
                    })
                }
                None => ExecutorActionType::CodingAgentInitialRequest(CodingAgentInitialRequest {
                    prompt,
                    prompt_kind: PromptKind::User,
                    executor_config: executor_config.clone(),
                    working_dir,
                }),
            };
            let mut action = ExecutorAction::new(action_type, None);
            action = action.with_provider_injection(resolved.injection);
            action = action.with_provider_selection(
                selected_provider_id.map(|id| id.to_string()),
                executor_config.model_id.clone(),
            );
            self.start_execution_with_id(
                &workspace,
                &session,
                &action,
                &ExecutionProcessRunReason::CodingAgent,
                execution_id,
                true,
            )
            .await
        }
        .await;

        if result.is_err() {
            SessionCommand::finish_execution(pool, execution_id, false).await?;
        }
        result.map(Some)
    }

    /// A freshly started server owns no child processes, so every execution
    /// still marked running at boot is an orphan from a previous process.
    /// Terminalize them and return their claimed commands to the queue so the
    /// four-slot coding-agent cap can never be filled by ghosts (the durable
    /// queue froze fleet-wide this way once).
    async fn terminalize_inherited_executions(&self) -> Result<(), ContainerError> {
        let pool = &self.db().pool;
        for process in ExecutionProcess::find_running(pool).await? {
            if process.run_reason != ExecutionProcessRunReason::CodingAgent {
                continue;
            }
            tracing::warn!(
                execution_process_id = %process.id,
                session_id = %process.session_id,
                "terminalizing execution inherited as running from a previous server process"
            );
            ExecutionProcess::update_completion(
                pool,
                process.id,
                ExecutionProcessStatus::Killed,
                None,
            )
            .await?;
            SessionCommand::requeue_killed_execution(pool, process.id).await?;
        }
        Ok(())
    }

    async fn dispatch_all_pending_commands(&self) -> Result<(), ContainerError> {
        for session_id in SessionCommand::pending_session_ids(&self.db().pool).await? {
            match self.dispatch_pending_commands(session_id).await {
                Ok(None)
                    if ExecutionProcess::count_running_coding_agents(&self.db().pool).await?
                        >= max_running_agents() =>
                {
                    break;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %error,
                        "failed session command did not block the fleet"
                    );
                }
            }
        }
        Ok(())
    }

    /// Backfill before_head_commit for legacy execution processes.
    /// Rules:
    /// - If a process has after_head_commit and missing before_head_commit,
    ///   then set before_head_commit to the previous process's after_head_commit.
    /// - If there is no previous process, set before_head_commit to the base branch commit.
    async fn backfill_before_head_commits(&self) -> Result<(), ContainerError> {
        let pool = &self.db().pool;
        let rows = ExecutionProcess::list_missing_before_context(pool).await?;
        for row in rows {
            // Skip if no after commit at all (shouldn't happen due to WHERE)
            // Prefer previous process after-commit if present
            let mut before = row.prev_after_head_commit.clone();

            // Fallback to base branch commit OID
            if before.is_none() {
                let repo_path = std::path::Path::new(row.repo_path.as_deref().unwrap_or_default());
                match self
                    .git()
                    .get_branch_oid(repo_path, row.target_branch.as_str())
                {
                    Ok(oid) => before = Some(oid),
                    Err(e) => {
                        tracing::warn!(
                            "Backfill: Failed to resolve base branch OID for workspace {} (branch {}): {}",
                            row.workspace_id,
                            row.target_branch,
                            e
                        );
                    }
                }
            }

            if let Some(before_oid) = before
                && let Err(e) = ExecutionProcessRepoState::update_before_head_commit(
                    pool,
                    row.id,
                    row.repo_id,
                    &before_oid,
                )
                .await
            {
                tracing::warn!(
                    "Backfill: Failed to update before_head_commit for process {}: {}",
                    row.id,
                    e
                );
            }
        }

        Ok(())
    }

    /// Backfill repo names that were migrated with a sentinel placeholder.
    /// Also backfills dev_script_working_dir and agent_working_dir for single-repo projects.
    async fn backfill_repo_names(&self) -> Result<(), ContainerError> {
        let pool = &self.db().pool;
        let repos = Repo::list_needing_name_fix(pool).await?;

        if repos.is_empty() {
            return Ok(());
        }

        tracing::info!("Backfilling {} repo names", repos.len());

        for repo in repos {
            let name = repo
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&repo.id.to_string())
                .to_string();

            Repo::update_name(pool, repo.id, &name, &name).await?;
        }

        Ok(())
    }

    fn cleanup_actions_for_repos(&self, repos: &[Repo]) -> Option<ExecutorAction> {
        let repos_with_cleanup: Vec<_> = repos
            .iter()
            .filter(|r| r.cleanup_script.is_some())
            .collect();

        if repos_with_cleanup.is_empty() {
            return None;
        }

        let mut iter = repos_with_cleanup.iter();
        let first = iter.next()?;
        let mut root_action = ExecutorAction::new(
            ExecutorActionType::ScriptRequest(ScriptRequest {
                script: first.cleanup_script.clone().unwrap(),
                language: ScriptRequestLanguage::Bash,
                context: ScriptContext::CleanupScript,
                working_dir: Some(first.name.clone()),
            }),
            None,
        );

        for repo in iter {
            root_action = root_action.append_action(ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: repo.cleanup_script.clone().unwrap(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::CleanupScript,
                    working_dir: Some(repo.name.clone()),
                }),
                None,
            ));
        }

        Some(root_action)
    }

    fn archive_actions_for_repos(&self, repos: &[Repo]) -> Option<ExecutorAction> {
        let repos_with_archive: Vec<_> = repos
            .iter()
            .filter(|r| r.archive_script.is_some())
            .collect();

        if repos_with_archive.is_empty() {
            return None;
        }

        let mut iter = repos_with_archive.iter();
        let first = iter.next()?;
        let mut root_action = ExecutorAction::new(
            ExecutorActionType::ScriptRequest(ScriptRequest {
                script: first.archive_script.clone().unwrap(),
                language: ScriptRequestLanguage::Bash,
                context: ScriptContext::ArchiveScript,
                working_dir: Some(first.name.clone()),
            }),
            None,
        );

        for repo in iter {
            root_action = root_action.append_action(ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: repo.archive_script.clone().unwrap(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::ArchiveScript,
                    working_dir: Some(repo.name.clone()),
                }),
                None,
            ));
        }

        Some(root_action)
    }

    /// Attempts to run the archive script for a workspace if configured.
    /// Silently returns Ok if no archive script is configured or if conditions aren't met.
    async fn try_run_archive_script(&self, workspace_id: Uuid) -> Result<(), ContainerError> {
        let pool = &self.db().pool;
        let workspace = Workspace::find_by_id(pool, workspace_id)
            .await?
            .ok_or(ContainerError::Other(anyhow!("Workspace not found")))?;
        // Worktree-disabled workspaces leave lifecycle to the user — no
        // archive-script execution against the real repo.
        if !workspace.use_worktree {
            return Ok(());
        }
        if ExecutionProcess::has_running_non_dev_server_processes_for_workspace(pool, workspace.id)
            .await
            .unwrap_or(true)
        {
            return Ok(());
        }
        if self.ensure_container_exists(&workspace).await.is_err() {
            return Ok(());
        }
        let repos = WorkspaceRepo::find_repos_for_workspace(pool, workspace.id).await?;
        let Some(action) = self.archive_actions_for_repos(&repos) else {
            return Ok(());
        };
        let session = match Session::find_latest_by_workspace_id(pool, workspace.id).await? {
            Some(s) => s,
            None => {
                Session::create(
                    pool,
                    &CreateSession {
                        executor: None,
                        name: None,
                        parent_session_id: None,
                    },
                    Uuid::new_v4(),
                    workspace.id,
                )
                .await?
            }
        };
        self.start_execution(
            &workspace,
            &session,
            &action,
            &ExecutionProcessRunReason::ArchiveScript,
        )
        .await?;

        Ok(())
    }

    /// Archive a workspace: set archived flag, stop running dev servers, and run archive script.
    async fn archive_workspace(&self, workspace_id: Uuid) -> Result<(), ContainerError> {
        let pool = &self.db().pool;

        Workspace::set_archived(pool, workspace_id, true).await?;

        // Stop running dev servers
        if let Ok(dev_servers) =
            ExecutionProcess::find_running_dev_servers_by_workspace(pool, workspace_id).await
        {
            for dev_server in dev_servers {
                if let Err(e) = self
                    .stop_execution(&dev_server, ExecutionProcessStatus::Killed)
                    .await
                {
                    tracing::error!(
                        "Failed to stop dev server {} for workspace {}: {}",
                        dev_server.id,
                        workspace_id,
                        e
                    );
                }
            }
        }

        // Run archive script (silently skips if not configured)
        if let Err(e) = self.try_run_archive_script(workspace_id).await {
            tracing::error!(
                "Failed to run archive script for workspace {}: {}",
                workspace_id,
                e
            );
        }

        Ok(())
    }

    fn setup_actions_for_repos(&self, repos: &[Repo]) -> Option<ExecutorAction> {
        let repos_with_setup: Vec<_> = repos.iter().filter(|r| r.setup_script.is_some()).collect();

        if repos_with_setup.is_empty() {
            return None;
        }

        let mut iter = repos_with_setup.iter();
        let first = iter.next()?;
        let mut root_action = ExecutorAction::new(
            ExecutorActionType::ScriptRequest(ScriptRequest {
                script: first.setup_script.clone().unwrap(),
                language: ScriptRequestLanguage::Bash,
                context: ScriptContext::SetupScript,
                working_dir: Some(first.name.clone()),
            }),
            None,
        );

        for repo in iter {
            root_action = root_action.append_action(ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: repo.setup_script.clone().unwrap(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::SetupScript,
                    working_dir: Some(repo.name.clone()),
                }),
                None,
            ));
        }

        Some(root_action)
    }

    fn setup_action_for_repo(repo: &Repo) -> Option<ExecutorAction> {
        repo.setup_script.as_ref().map(|script| {
            ExecutorAction::new(
                ExecutorActionType::ScriptRequest(ScriptRequest {
                    script: script.clone(),
                    language: ScriptRequestLanguage::Bash,
                    context: ScriptContext::SetupScript,
                    working_dir: Some(repo.name.clone()),
                }),
                None,
            )
        })
    }

    fn build_sequential_setup_chain(
        repos: &[&Repo],
        next_action: ExecutorAction,
    ) -> ExecutorAction {
        let mut chained = next_action;
        for repo in repos.iter().rev() {
            if let Some(script) = &repo.setup_script {
                chained = ExecutorAction::new(
                    ExecutorActionType::ScriptRequest(ScriptRequest {
                        script: script.clone(),
                        language: ScriptRequestLanguage::Bash,
                        context: ScriptContext::SetupScript,
                        working_dir: Some(repo.name.clone()),
                    }),
                    Some(Box::new(chained)),
                );
            }
        }
        chained
    }

    /// Reset a session to a specific process: restore worktrees, stop processes, drop later processes.
    async fn reset_session_to_process(
        &self,
        session_id: Uuid,
        target_process_id: Uuid,
        perform_git_reset: bool,
        force_when_dirty: bool,
    ) -> Result<(), ContainerError> {
        let pool = &self.db().pool;

        let process = ExecutionProcess::find_by_id(pool, target_process_id)
            .await?
            .ok_or_else(|| ContainerError::Other(anyhow!("Process not found")))?;
        if process.session_id != session_id {
            return Err(ContainerError::Other(anyhow!(
                "Process does not belong to this session"
            )));
        }

        let session = Session::find_by_id(pool, session_id)
            .await?
            .ok_or_else(|| ContainerError::Other(anyhow!("Session not found")))?;
        let workspace = Workspace::find_by_id(pool, session.workspace_id)
            .await?
            .ok_or_else(|| ContainerError::Other(anyhow!("Workspace not found")))?;

        let repos = WorkspaceRepo::find_repos_for_workspace(pool, workspace.id).await?;
        let repo_states =
            ExecutionProcessRepoState::find_by_execution_process_id(pool, target_process_id)
                .await?;

        // Worktree-disabled workspaces leave git state to the user; never
        // reconcile/reset the real repo on their behalf.
        if workspace.use_worktree {
            let container_ref = self.ensure_container_exists(&workspace).await?;
            let workspace_dir = std::path::PathBuf::from(container_ref);
            let is_dirty = self
                .is_container_clean(&workspace)
                .await
                .map(|is_clean| !is_clean)
                .unwrap_or(false);

            for repo in &repos {
                let repo_state = repo_states.iter().find(|s| s.repo_id == repo.id);
                let target_oid = match repo_state.and_then(|s| s.before_head_commit.clone()) {
                    Some(oid) => Some(oid),
                    None => {
                        ExecutionProcess::find_prev_after_head_commit(
                            pool,
                            session_id,
                            target_process_id,
                            repo.id,
                        )
                        .await?
                    }
                };

                let worktree_path = workspace_dir.join(&repo.name);
                if let Some(oid) = target_oid {
                    self.git().reconcile_worktree_to_commit(
                        &worktree_path,
                        &oid,
                        git::WorktreeResetOptions::new(
                            perform_git_reset,
                            force_when_dirty,
                            is_dirty,
                            perform_git_reset,
                        ),
                    );
                }
            }
        }

        self.try_stop(&workspace, false).await;
        ExecutionProcess::drop_at_and_after(pool, session_id, target_process_id).await?;

        Ok(())
    }

    async fn try_stop(&self, workspace: &Workspace, include_dev_server: bool) {
        // stop execution processes for this workspace's sessions
        let sessions = match Session::find_by_workspace_id(&self.db().pool, workspace.id).await {
            Ok(s) => s,
            Err(_) => return,
        };

        for session in sessions {
            if let Ok(processes) =
                ExecutionProcess::find_by_session_id(&self.db().pool, session.id, false).await
            {
                for process in processes {
                    // Skip dev server processes unless explicitly included
                    if !include_dev_server
                        && process.run_reason == ExecutionProcessRunReason::DevServer
                    {
                        continue;
                    }
                    if process.status == ExecutionProcessStatus::Running {
                        self.stop_execution(&process, ExecutionProcessStatus::Killed)
                            .await
                            .unwrap_or_else(|e| {
                                tracing::debug!(
                                    "Failed to stop execution process {} for workspace {}: {}",
                                    process.id,
                                    workspace.id,
                                    e
                                );
                            });
                    }
                }
            }
        }
    }

    async fn ensure_container_exists(
        &self,
        workspace: &Workspace,
    ) -> Result<ContainerRef, ContainerError>;

    async fn is_container_clean(&self, workspace: &Workspace) -> Result<bool, ContainerError>;

    async fn start_execution_inner(
        &self,
        workspace: &Workspace,
        execution_process: &ExecutionProcess,
        executor_action: &ExecutorAction,
    ) -> Result<(), ContainerError>;

    async fn stop_execution(
        &self,
        execution_process: &ExecutionProcess,
        status: ExecutionProcessStatus,
    ) -> Result<(), ContainerError>;

    async fn try_commit_changes(&self, ctx: &ExecutionContext) -> Result<bool, ContainerError>;

    async fn copy_project_files(
        &self,
        source_dir: &Path,
        target_dir: &Path,
        copy_files: &str,
    ) -> Result<(), ContainerError>;

    /// Stream diff updates as LogMsg for WebSocket endpoints.
    async fn stream_diff(
        &self,
        workspace: &Workspace,
        stats_only: bool,
    ) -> Result<futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>>, ContainerError>;

    /// Fetch the MsgStore for a given execution ID, panicking if missing.
    async fn get_msg_store_by_id(&self, uuid: &Uuid) -> Option<Arc<MsgStore>> {
        let map = self.msg_stores().read().await;
        map.get(uuid).cloned()
    }

    async fn git_branch_prefix(&self) -> String;

    async fn git_branch_from_workspace(&self, workspace_id: &Uuid, task_title: &str) -> String {
        let task_title_id = git_branch_id(task_title);
        let prefix = self.git_branch_prefix().await;

        if prefix.is_empty() {
            format!("{}-{}", short_uuid(workspace_id), task_title_id)
        } else {
            format!("{}/{}-{}", prefix, short_uuid(workspace_id), task_title_id)
        }
    }

    async fn stream_raw_logs(
        &self,
        id: &Uuid,
    ) -> Option<futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>>> {
        if let Some(store) = self.get_msg_store_by_id(id).await {
            // First try in-memory store
            return Some(
                store
                    .history_plus_stream()
                    .filter(|msg| {
                        future::ready(matches!(
                            msg,
                            Ok(LogMsg::Stdout(..) | LogMsg::Stderr(..) | LogMsg::Finished)
                        ))
                    })
                    .boxed(),
            );
        } else {
            let messages = execution_process::load_raw_log_messages(&self.db().pool, *id).await?;

            let stream = futures::stream::iter(
                messages
                    .into_iter()
                    .filter(|m| matches!(m, LogMsg::Stdout(_) | LogMsg::Stderr(_)))
                    .chain(std::iter::once(LogMsg::Finished))
                    .map(Ok::<_, std::io::Error>),
            )
            .boxed();

            Some(stream)
        }
    }

    async fn stream_normalized_logs(
        &self,
        id: &Uuid,
    ) -> Option<futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>>> {
        if let Some(store) = self.get_msg_store_by_id(id).await {
            Some(
                store
                    .history_plus_stream()
                    .take_while(|msg| future::ready(!matches!(msg, Ok(LogMsg::Finished))))
                    .filter(|msg| future::ready(matches!(msg, Ok(LogMsg::JsonPatch(..)))))
                    .chain(futures::stream::once(async {
                        Ok::<_, std::io::Error>(LogMsg::Finished)
                    }))
                    .boxed(),
            )
        } else {
            let raw_messages =
                execution_process::load_raw_log_messages(&self.db().pool, *id).await?;

            // Create temporary store and populate
            // Include JsonPatch messages (already normalized) and Stdout/Stderr (need normalization)
            let temp_store = Arc::new(MsgStore::new());
            for msg in raw_messages {
                if matches!(
                    msg,
                    LogMsg::Stdout(_) | LogMsg::Stderr(_) | LogMsg::JsonPatch(_)
                ) {
                    temp_store.push(msg);
                }
            }
            temp_store.push_finished();

            let process = match ExecutionProcess::find_by_id(&self.db().pool, *id).await {
                Ok(Some(process)) => process,
                Ok(None) => {
                    tracing::error!("No execution process found for ID: {}", id);
                    return None;
                }
                Err(e) => {
                    tracing::error!("Failed to fetch execution process {}: {}", id, e);
                    return None;
                }
            };

            // Get the workspace to determine correct directory
            let (workspace, _session) =
                match process.parent_workspace_and_session(&self.db().pool).await {
                    Ok(Some((workspace, session))) => (workspace, session),
                    Ok(None) => {
                        tracing::error!(
                            "No workspace/session found for session ID: {}",
                            process.session_id
                        );
                        return None;
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to fetch workspace for session {}: {}",
                            process.session_id,
                            e
                        );
                        return None;
                    }
                };

            if let Err(err) = self.ensure_container_exists(&workspace).await {
                tracing::warn!(
                    "Failed to recreate worktree before log normalization for workspace {}: {}",
                    workspace.id,
                    err
                );
            }

            let current_dir = self.workspace_to_current_dir(&workspace).await;

            let executor_action = if let Ok(executor_action) = process.executor_action() {
                executor_action
            } else {
                tracing::error!(
                    "Failed to parse executor action: {:?}",
                    process.executor_action()
                );
                return None;
            };

            // Spawn normalizer on populated store and collect JoinHandles
            let handles = match executor_action.typ() {
                ExecutorActionType::CodingAgentInitialRequest(request) => {
                    #[cfg(feature = "qa-mode")]
                    {
                        let executor = QaMockExecutor;
                        executor.normalize_logs(
                            temp_store.clone(),
                            &request.effective_dir(&current_dir),
                        )
                    }
                    #[cfg(not(feature = "qa-mode"))]
                    {
                        let executor = ExecutorConfigs::get_cached()
                            .get_coding_agent_or_default(&request.executor_config.profile_id());
                        executor.normalize_logs(
                            temp_store.clone(),
                            &request.effective_dir(&current_dir),
                        )
                    }
                }
                ExecutorActionType::CodingAgentFollowUpRequest(request) => {
                    #[cfg(feature = "qa-mode")]
                    {
                        let executor = QaMockExecutor;
                        executor.normalize_logs(
                            temp_store.clone(),
                            &request.effective_dir(&current_dir),
                        )
                    }
                    #[cfg(not(feature = "qa-mode"))]
                    {
                        let executor = ExecutorConfigs::get_cached()
                            .get_coding_agent_or_default(&request.executor_config.profile_id());
                        executor.normalize_logs(
                            temp_store.clone(),
                            &request.effective_dir(&current_dir),
                        )
                    }
                }
                #[cfg(feature = "qa-mode")]
                ExecutorActionType::ReviewRequest(_request) => {
                    let executor = QaMockExecutor;
                    executor.normalize_logs(temp_store.clone(), &current_dir)
                }
                #[cfg(not(feature = "qa-mode"))]
                ExecutorActionType::ReviewRequest(request) => {
                    let executor = ExecutorConfigs::get_cached()
                        .get_coding_agent_or_default(&request.executor_config.profile_id());
                    executor.normalize_logs(temp_store.clone(), &current_dir)
                }
                _ => {
                    tracing::debug!(
                        "Executor action doesn't support log normalization: {:?}",
                        process.executor_action()
                    );
                    return None;
                }
            };

            // Await all normalizer tasks, then push Ready so the dedup
            // stream knows when to flush its buffer and terminate.
            {
                let store = temp_store.clone();
                tokio::spawn(async move {
                    for handle in handles {
                        let _ = handle.await;
                    }
                    store.push(LogMsg::Ready);
                });
            }

            // Stream normalized patches, deduplicating consecutive patches
            // that target the same path (only the final state matters for
            // historical replay). The Ready sentinel flushes the buffer.
            enum PatchOrDone {
                Patch(Patch),
                Done,
            }

            let stream = temp_store
                .history_plus_stream()
                .filter_map(|msg| async move {
                    match msg {
                        Ok(LogMsg::JsonPatch(patch)) => Some(PatchOrDone::Patch(patch)),
                        Ok(LogMsg::Ready) => Some(PatchOrDone::Done),
                        _ => None,
                    }
                });

            let deduped = futures::stream::unfold(
                (stream.boxed(), None::<Patch>, HashSet::<String>::new()),
                |(mut stream, buffered, mut sent_paths)| async move {
                    match stream.next().await {
                        Some(PatchOrDone::Patch(patch)) => {
                            let Some(prev) = buffered else {
                                // First patch — just buffer it
                                return Some((None, (stream, Some(patch), sent_paths)));
                            };
                            if patch_entry_path(&patch) == patch_entry_path(&prev)
                                && is_add_or_replace(&patch)
                                && is_add_or_replace(&prev)
                            {
                                // Same path, both add/replace — replace buffer
                                Some((None, (stream, Some(patch), sent_paths)))
                            } else {
                                // Different — emit prev, buffer new
                                let prev = fix_patch_ops(prev, &mut sent_paths);
                                Some((Some(prev), (stream, Some(patch), sent_paths)))
                            }
                        }
                        Some(PatchOrDone::Done) | None => {
                            // Sentinel or stream end: flush buffer and terminate
                            if let Some(prev) = buffered {
                                let prev = fix_patch_ops(prev, &mut sent_paths);
                                return Some((Some(prev), (stream, None, sent_paths)));
                            }
                            None
                        }
                    }
                },
            )
            .filter_map(|opt| async move { opt })
            .map(|p| Ok::<_, std::io::Error>(LogMsg::JsonPatch(p)))
            .chain(futures::stream::once(async {
                Ok::<_, std::io::Error>(LogMsg::Finished)
            }));

            Some(deduped.boxed())
        }
    }

    async fn start_workspace(
        &self,
        workspace: &Workspace,
        executor_config: ExecutorConfig,
        prompt: String,
        injection: ProviderInjection,
        selected_provider_id: Option<String>,
        selected_model_id: Option<String>,
    ) -> Result<ExecutionProcess, ContainerError> {
        // Create container
        self.create(workspace).await?;

        let repos = WorkspaceRepo::find_repos_for_workspace(&self.db().pool, workspace.id).await?;

        let workspace = Workspace::find_by_id(&self.db().pool, workspace.id)
            .await?
            .ok_or(SqlxError::RowNotFound)?;

        // Create a session for this workspace
        let session = Session::create(
            &self.db().pool,
            &CreateSession {
                executor: Some(executor_config.executor.to_string()),
                name: None,
                parent_session_id: None,
            },
            Uuid::new_v4(),
            workspace.id,
        )
        .await?;

        // Worktree-disabled workspaces skip setup scripts entirely — the
        // user's real repo already has its environment. Build only the coding
        // agent action. Worktree-enabled path keeps the existing setup chain.
        let repos_with_setup: Vec<_> = if workspace.use_worktree {
            repos.iter().filter(|r| r.setup_script.is_some()).collect()
        } else {
            Vec::new()
        };

        let all_parallel = repos_with_setup.iter().all(|r| r.parallel_setup_script);

        let working_dir = session
            .agent_working_dir
            .as_ref()
            .filter(|dir| !dir.is_empty())
            .cloned();

        let coding_action = {
            let mut a = ExecutorAction::new(
                ExecutorActionType::CodingAgentInitialRequest(CodingAgentInitialRequest {
                    prompt,
                    prompt_kind: PromptKind::User,
                    executor_config: executor_config.clone(),
                    working_dir,
                }),
                None,
            );
            a = a.with_provider_injection(injection);
            a.with_provider_selection(selected_provider_id, selected_model_id)
        };

        let execution_process = if repos_with_setup.is_empty() {
            // No setup to run (either no scripts configured, or worktree-disabled).
            self.start_execution(
                &workspace,
                &session,
                &coding_action,
                &ExecutionProcessRunReason::CodingAgent,
            )
            .await?
        } else if all_parallel {
            // All parallel: start each setup independently, then start coding agent
            for repo in &repos_with_setup {
                if let Some(action) = Self::setup_action_for_repo(repo)
                    && let Err(e) = self
                        .start_execution(
                            &workspace,
                            &session,
                            &action,
                            &ExecutionProcessRunReason::SetupScript,
                        )
                        .await
                {
                    tracing::warn!(?e, "Failed to start setup script in parallel mode");
                }
            }
            self.start_execution(
                &workspace,
                &session,
                &coding_action,
                &ExecutionProcessRunReason::CodingAgent,
            )
            .await?
        } else {
            // Any sequential: chain ALL setups → coding agent via next_action
            let main_action = Self::build_sequential_setup_chain(&repos_with_setup, coding_action);
            self.start_execution(
                &workspace,
                &session,
                &main_action,
                &ExecutionProcessRunReason::SetupScript,
            )
            .await?
        };

        Ok(execution_process)
    }

    async fn start_execution(
        &self,
        workspace: &Workspace,
        session: &Session,
        executor_action: &ExecutorAction,
        run_reason: &ExecutionProcessRunReason,
    ) -> Result<ExecutionProcess, ContainerError> {
        self.start_execution_with_id(
            workspace,
            session,
            executor_action,
            run_reason,
            Uuid::new_v4(),
            false,
        )
        .await
    }

    async fn start_execution_with_id(
        &self,
        workspace: &Workspace,
        session: &Session,
        executor_action: &ExecutorAction,
        run_reason: &ExecutionProcessRunReason,
        execution_process_id: Uuid,
        claim_pending_commands: bool,
    ) -> Result<ExecutionProcess, ContainerError> {
        // Create new execution process record
        // Capture current HEAD per repository as the "before" commit for this execution
        let repositories =
            WorkspaceRepo::find_repos_for_workspace(&self.db().pool, workspace.id).await?;
        if repositories.is_empty() {
            return Err(ContainerError::Other(anyhow!(
                "Workspace has no repositories configured"
            )));
        }

        let mut repo_states = Vec::with_capacity(repositories.len());
        for repo in &repositories {
            let before_head_commit = workspace
                .execution_dir(repo)
                .and_then(|repo_path| self.git().get_head_info(&repo_path).ok())
                .map(|h| h.oid);
            repo_states.push(CreateExecutionProcessRepoState {
                repo_id: repo.id,
                before_head_commit,
                after_head_commit: None,
                merge_commit: None,
            });
        }
        let create_execution_process = CreateExecutionProcess {
            session_id: session.id,
            executor_action: executor_action.without_provider_bindings(),
            run_reason: run_reason.clone(),
        };

        let execution_process = ExecutionProcess::create(
            &self.db().pool,
            &create_execution_process,
            execution_process_id,
            &repo_states,
        )
        .await?;
        if *run_reason == ExecutionProcessRunReason::CodingAgent {
            // Stamp the claimed batch now that the execution row exists (the
            // FK forbids stamping at claim time) and before the child spawns,
            // so completion callbacks always find their commands.
            if claim_pending_commands
                && SessionCommand::bind_execution(&self.db().pool, session.id, execution_process.id)
                    .await?
                    == 0
            {
                ExecutionProcess::update_completion(
                    &self.db().pool,
                    execution_process.id,
                    ExecutionProcessStatus::Failed,
                    None,
                )
                .await?;
                return Err(ContainerError::Other(anyhow!(
                    "No claimed session command for execution attempt {}",
                    execution_process.id
                )));
            }
            let command = match executor_action.typ() {
                ExecutorActionType::CodingAgentInitialRequest(request) => {
                    Some((request.prompt.clone(), request.executor_config.clone()))
                }
                ExecutorActionType::CodingAgentFollowUpRequest(request) => {
                    Some((request.prompt.clone(), request.executor_config.clone()))
                }
                ExecutorActionType::ReviewRequest(request) => {
                    Some((request.prompt.clone(), request.executor_config.clone()))
                }
                ExecutorActionType::ScriptRequest(_) => None,
            };
            if !claim_pending_commands
                && let Some((body, executor_config)) = command
                && let Err(error) = SessionCommand::ensure_claimed(
                    &self.db().pool,
                    session.id,
                    execution_process.id,
                    body,
                    SessionCommandConfig {
                        executor_config,
                        selected_provider_id: executor_action
                            .selected_provider_id
                            .as_deref()
                            .and_then(|id| id.parse().ok()),
                        auth_binding_id: executor_action
                            .selected_provider_id
                            .as_deref()
                            .and_then(|id| id.parse().ok()),
                        metered: None,
                    },
                )
                .await
            {
                ExecutionProcess::update_completion(
                    &self.db().pool,
                    execution_process.id,
                    ExecutionProcessStatus::Failed,
                    None,
                )
                .await?;
                return Err(error.into());
            }
        }
        self.msg_stores()
            .write()
            .await
            .insert(execution_process.id, Arc::new(MsgStore::new()));
        if *run_reason != ExecutionProcessRunReason::ArchiveScript
            && let Err(e) = Workspace::set_archived(&self.db().pool, workspace.id, false).await
        {
            self.msg_stores()
                .write()
                .await
                .remove(&execution_process.id);
            return Err(e.into());
        }

        if let Some(prompt) = match executor_action.typ() {
            ExecutorActionType::CodingAgentInitialRequest(coding_agent_request) => {
                Some(coding_agent_request.prompt.clone())
            }
            ExecutorActionType::CodingAgentFollowUpRequest(follow_up_request) => {
                Some(follow_up_request.prompt.clone())
            }
            ExecutorActionType::ReviewRequest(review_request) => {
                Some(review_request.prompt.clone())
            }
            ExecutorActionType::ScriptRequest(_) => None,
        } {
            let create_coding_agent_turn = CreateCodingAgentTurn {
                execution_process_id: execution_process.id,
                prompt: Some(prompt),
            };

            let coding_agent_turn_id = Uuid::new_v4();

            if let Err(e) = CodingAgentTurn::create(
                &self.db().pool,
                &create_coding_agent_turn,
                coding_agent_turn_id,
            )
            .await
            {
                self.msg_stores()
                    .write()
                    .await
                    .remove(&execution_process.id);
                return Err(e.into());
            }

            // Persist the per-message (model, provider) selection so the recents
            // query and transcript markers work (§4/§6/§7).
            if let (Some(model_id), Some(provider_id)) = (
                executor_action.selected_model_id.as_deref(),
                executor_action.selected_provider_id.as_deref(),
            ) && !model_id.is_empty()
                && !provider_id.is_empty()
                && let Err(e) = CodingAgentTurn::update_selected_model_provider(
                    &self.db().pool,
                    execution_process.id,
                    model_id,
                    provider_id,
                )
                .await
            {
                tracing::warn!(
                    execution_process_id = %execution_process.id,
                    error = %e,
                    "failed to persist model/provider selection on turn"
                );
            }
        }

        if let Err(start_error) = self
            .start_execution_inner(workspace, &execution_process, executor_action)
            .await
        {
            self.msg_stores()
                .write()
                .await
                .remove(&execution_process.id);
            // Mark process as failed
            if let Err(update_error) = ExecutionProcess::update_completion(
                &self.db().pool,
                execution_process.id,
                ExecutionProcessStatus::Failed,
                None,
            )
            .await
            {
                tracing::error!(
                    "Failed to mark execution process {} as failed after start error: {}",
                    execution_process.id,
                    update_error
                );
            }
            if let Err(command_error) =
                SessionCommand::finish_execution(&self.db().pool, execution_process.id, false).await
            {
                tracing::error!(
                    "Failed to fail commands for execution {} after start error: {}",
                    execution_process.id,
                    command_error
                );
            }
            // Emit stderr error message
            let log_message = LogMsg::Stderr(format!("Failed to start execution: {start_error}"));
            if let Err(e) = execution_process::append_log_message(
                session.id,
                execution_process.id,
                &log_message,
            )
            .await
            {
                tracing::error!(
                    "Failed to write error log for execution {}: {}",
                    execution_process.id,
                    e
                );
            }

            // Emit NextAction with failure context for coding agent requests
            if let ContainerError::ExecutorError(ExecutorError::ExecutableNotFound { program }) =
                &start_error
            {
                let help_text = format!("The required executable `{program}` is not installed.");
                let error_message = NormalizedEntry {
                    timestamp: None,
                    entry_type: NormalizedEntryType::ErrorMessage {
                        error_type: NormalizedEntryError::SetupRequired,
                    },
                    content: help_text,
                    metadata: None,
                };
                let patch = ConversationPatch::add_normalized_entry(2, error_message);
                if let Err(e) = execution_process::append_log_message(
                    session.id,
                    execution_process.id,
                    &LogMsg::JsonPatch(patch),
                )
                .await
                {
                    tracing::error!(
                        "Failed to write setup-required log for execution {}: {}",
                        execution_process.id,
                        e
                    );
                }
            };
            return Err(start_error);
        }

        // Start processing normalised logs for executor requests and follow ups
        let workspace_root = self.workspace_to_current_dir(workspace).await;
        #[cfg_attr(feature = "qa-mode", allow(unused_variables))]
        if let Some((executor_profile_id, working_dir)) = match executor_action.typ() {
            ExecutorActionType::CodingAgentInitialRequest(request) => Some((
                request.executor_config.profile_id(),
                request.effective_dir(&workspace_root),
            )),
            ExecutorActionType::CodingAgentFollowUpRequest(request) => Some((
                request.executor_config.profile_id(),
                request.effective_dir(&workspace_root),
            )),
            ExecutorActionType::ReviewRequest(request) => Some((
                request.executor_config.profile_id(),
                request.effective_dir(&workspace_root),
            )),
            _ => None,
        } {
            let msg_store = match self.get_msg_store_by_id(&execution_process.id).await {
                Some(store) => store,
                None => {
                    self.msg_stores()
                        .write()
                        .await
                        .remove(&execution_process.id);
                    return Err(ContainerError::Other(anyhow!(
                        "MsgStore missing for execution {} during normalization setup",
                        execution_process.id
                    )));
                }
            };
            #[cfg(feature = "qa-mode")]
            {
                let executor = QaMockExecutor;
                let _ = executor.normalize_logs(msg_store, &working_dir);
            }
            #[cfg(not(feature = "qa-mode"))]
            {
                if let Some(executor) =
                    ExecutorConfigs::get_cached().get_coding_agent(&executor_profile_id)
                {
                    let _ = executor.normalize_logs(msg_store, &working_dir);
                } else {
                    tracing::error!(
                        "Failed to resolve profile '{:?}' for normalization",
                        executor_profile_id
                    );
                }
            }
        }

        execution_process::spawn_stream_raw_logs_to_storage(
            self.msg_stores().clone(),
            self.db().clone(),
            execution_process.id,
            session.id,
        );
        Ok(execution_process)
    }

    async fn try_start_next_action(&self, ctx: &ExecutionContext) -> Result<(), ContainerError> {
        let action = ctx.execution_process.executor_action()?;
        let next_action = if let Some(next_action) = action.next_action() {
            next_action
        } else {
            tracing::debug!("No next action configured");
            return Ok(());
        };

        // Determine the run reason of the next action
        let next_run_reason = match (action.typ(), next_action.typ()) {
            (ExecutorActionType::ScriptRequest(_), ExecutorActionType::ScriptRequest(_)) => {
                ExecutionProcessRunReason::SetupScript
            }
            (
                ExecutorActionType::CodingAgentInitialRequest(_)
                | ExecutorActionType::CodingAgentFollowUpRequest(_)
                | ExecutorActionType::ReviewRequest(_),
                ExecutorActionType::ScriptRequest(_),
            ) => ExecutionProcessRunReason::CleanupScript,
            (
                _,
                ExecutorActionType::CodingAgentFollowUpRequest(_)
                | ExecutorActionType::CodingAgentInitialRequest(_)
                | ExecutorActionType::ReviewRequest(_),
            ) => ExecutionProcessRunReason::CodingAgent,
        };

        self.start_execution(&ctx.workspace, &ctx.session, next_action, &next_run_reason)
            .await?;

        tracing::debug!("Started next action: {:?}", next_action);
        Ok(())
    }
}
