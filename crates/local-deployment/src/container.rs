use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::anyhow;
use async_trait::async_trait;
use command_group::AsyncGroupChild;
use db::{
    DBService,
    models::{
        coding_agent_turn::CodingAgentTurn,
        execution_process::{
            ExecutionContext, ExecutionProcess, ExecutionProcessRunReason, ExecutionProcessStatus,
        },
        execution_process_repo_state::ExecutionProcessRepoState,
        repo::Repo,
        session::Session,
        session_command::SessionCommand,
        workspace::Workspace,
        workspace_repo::WorkspaceRepo,
    },
};
use deployment::DeploymentError;
use executors::{
    actions::{Executable, ExecutorAction, ExecutorActionType},
    approvals::{ExecutorApprovalService, NoopExecutorApprovalService},
    env::{ExecutionEnv, RepoContext},
    executors::{
        CancellationToken, CodingAgent, ExecutorExitResult, ExecutorExitSignal,
        StandardCodingAgentExecutor,
    },
    logs::{NormalizedEntryType, utils::patch::extract_normalized_entry_from_patch},
    outcome::NormalizedExecutionOutcome,
};
use futures::{FutureExt, TryStreamExt, stream::select};
use git::GitService;
use serde_json::json;
use services::services::{
    analytics::AnalyticsContext,
    approvals::{Approvals, executor_approvals::ExecutorApprovalBridge},
    config::{Config, DEFAULT_COMMIT_REMINDER_PROMPT, MIN_AUTO_ARCHIVE_IDLE_DAYS},
    container::{ContainerError, ContainerRef, ContainerService},
    diff_stream::{self, DiffStreamHandle},
    file::FileService,
    notification::NotificationService,
    remote_client::RemoteClient,
    remote_sync,
};
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinHandle,
};
use tokio_util::io::ReaderStream;
use utils::{log_msg::LogMsg, msg_store::MsgStore, text::truncate_to_char_boundary};
use uuid::Uuid;
use workspace_manager::{RepoWorkspaceInput, WorkspaceError, WorkspaceManager};

use crate::{command, copy};

const WORKSPACE_TOUCH_DEBOUNCE: Duration = Duration::from_mins(2);

fn execution_current_dir(
    worktree_root: Option<&Path>,
    primary_repo_name: &str,
    primary_repo_path: &Path,
    executor_action: &ExecutorAction,
) -> PathBuf {
    match (worktree_root, executor_action.typ()) {
        // Script actions retain a repository-relative working_dir so a chain
        // can address each workspace repo. Resolve that relative path from
        // the worktree root, not from the primary repository itself.
        (Some(root), ExecutorActionType::ScriptRequest(_)) => root.to_path_buf(),
        (Some(root), _) => root.join(primary_repo_name),
        (None, _) => primary_repo_path.to_path_buf(),
    }
}

#[derive(Clone)]
pub struct LocalContainerService {
    db: DBService,
    workspace_manager: WorkspaceManager,
    child_store: Arc<RwLock<HashMap<Uuid, Arc<RwLock<AsyncGroupChild>>>>>,
    cancellation_tokens: Arc<RwLock<HashMap<Uuid, CancellationToken>>>,
    msg_stores: Arc<RwLock<HashMap<Uuid, Arc<MsgStore>>>>,
    /// Tracks background tasks that stream logs to the database.
    /// When stopping execution, we await these to ensure logs are fully persisted.
    db_stream_handles: Arc<RwLock<HashMap<Uuid, JoinHandle<()>>>>,
    exit_monitor_handles: Arc<RwLock<HashMap<Uuid, JoinHandle<()>>>>,
    workspace_touch_times: Arc<RwLock<HashMap<Uuid, Instant>>>,
    scheduler_lock: Arc<Mutex<()>>,
    config: Arc<RwLock<Config>>,
    git: GitService,
    file_service: FileService,
    analytics: Option<AnalyticsContext>,
    approvals: Approvals,
    notification_service: NotificationService,
    remote_client: Option<RemoteClient>,
}

impl LocalContainerService {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        db: DBService,
        workspace_manager: WorkspaceManager,
        msg_stores: Arc<RwLock<HashMap<Uuid, Arc<MsgStore>>>>,
        config: Arc<RwLock<Config>>,
        git: GitService,
        file_service: FileService,
        analytics: Option<AnalyticsContext>,
        approvals: Approvals,
        remote_client: Option<RemoteClient>,
    ) -> Self {
        let child_store = Arc::new(RwLock::new(HashMap::new()));
        let cancellation_tokens = Arc::new(RwLock::new(HashMap::new()));
        let db_stream_handles = Arc::new(RwLock::new(HashMap::new()));
        let exit_monitor_handles = Arc::new(RwLock::new(HashMap::new()));
        let workspace_touch_times = Arc::new(RwLock::new(HashMap::new()));
        let notification_service = NotificationService::new(config.clone());

        let container = LocalContainerService {
            db,
            workspace_manager,
            child_store,
            cancellation_tokens,
            msg_stores,
            db_stream_handles,
            exit_monitor_handles,
            workspace_touch_times,
            scheduler_lock: Arc::new(Mutex::new(())),
            config,
            git,
            file_service,
            analytics,
            approvals,
            notification_service,
            remote_client,
        };

        container.spawn_workspace_cleanup();

        container
    }

    fn map_workspace_manager_error(err: WorkspaceError) -> ContainerError {
        match err {
            WorkspaceError::Database(err) => ContainerError::Sqlx(err),
            WorkspaceError::Worktree(err) => ContainerError::Worktree(err),
            WorkspaceError::GitService(err) => ContainerError::GitServiceError(err),
            WorkspaceError::Io(err) => ContainerError::Io(err),
            WorkspaceError::NoRepositories => {
                ContainerError::Other(anyhow!("No repositories provided"))
            }
            WorkspaceError::Repo(err) => ContainerError::Other(anyhow!(err)),
            WorkspaceError::WorkspaceNotFound => {
                ContainerError::Other(anyhow!("Workspace not found"))
            }
            WorkspaceError::RepoAlreadyAttached => {
                ContainerError::Other(anyhow!("Repository already attached to workspace"))
            }
            WorkspaceError::BranchNotFound { repo_name, branch } => ContainerError::Other(anyhow!(
                "Branch '{}' does not exist in repository '{}'",
                branch,
                repo_name
            )),
            WorkspaceError::NonGitRepoRequiresDirectMode => ContainerError::Other(anyhow!(
                "Non-Git repo cannot be attached to a worktree-enabled workspace; set use_worktree = false"
            )),
            WorkspaceError::PartialCreation(msg) => ContainerError::Other(anyhow!(msg)),
        }
    }

    async fn workspace_repo_inputs(
        &self,
        workspace_id: Uuid,
    ) -> Result<(Vec<Repo>, Vec<RepoWorkspaceInput>), ContainerError> {
        let workspace_repos =
            WorkspaceRepo::find_by_workspace_id(&self.db.pool, workspace_id).await?;
        if workspace_repos.is_empty() {
            return Err(ContainerError::Other(anyhow!(
                "Workspace has no repositories configured"
            )));
        }

        let repositories =
            WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace_id).await?;
        let target_branches: HashMap<_, _> = workspace_repos
            .iter()
            .map(|wr| (wr.repo_id, wr.target_branch.clone()))
            .collect();

        let workspace_inputs: Vec<RepoWorkspaceInput> = repositories
            .iter()
            .map(|repo| {
                let target_branch = target_branches.get(&repo.id).cloned().ok_or_else(|| {
                    ContainerError::Other(anyhow!(
                        "Missing target branch mapping for repo {} in workspace {}",
                        repo.id,
                        workspace_id
                    ))
                })?;
                Ok(RepoWorkspaceInput::new(repo.clone(), target_branch))
            })
            .collect::<Result<_, ContainerError>>()?;

        Ok((repositories, workspace_inputs))
    }

    async fn get_child_from_store(&self, id: &Uuid) -> Option<Arc<RwLock<AsyncGroupChild>>> {
        let map = self.child_store.read().await;
        map.get(id).cloned()
    }

    async fn add_child_to_store(&self, id: Uuid, exec: AsyncGroupChild) {
        let mut map = self.child_store.write().await;
        map.insert(id, Arc::new(RwLock::new(exec)));
    }

    async fn remove_child_from_store(&self, id: &Uuid) {
        let mut map = self.child_store.write().await;
        map.remove(id);
    }

    async fn add_cancellation_token(&self, id: Uuid, token: CancellationToken) {
        let mut map = self.cancellation_tokens.write().await;
        map.insert(id, token);
    }

    async fn take_cancellation_token(&self, id: &Uuid) -> Option<CancellationToken> {
        let mut map = self.cancellation_tokens.write().await;
        map.remove(id)
    }

    async fn add_db_stream_handle(&self, id: Uuid, handle: JoinHandle<()>) {
        let mut map = self.db_stream_handles.write().await;
        map.insert(id, handle);
    }

    async fn take_db_stream_handle(&self, id: &Uuid) -> Option<JoinHandle<()>> {
        let mut map = self.db_stream_handles.write().await;
        map.remove(id)
    }

    async fn add_exit_monitor_handle(&self, id: Uuid, handle: JoinHandle<()>) {
        let mut map = self.exit_monitor_handles.write().await;
        map.insert(id, handle);
    }

    async fn take_exit_monitor_handle(&self, id: &Uuid) -> Option<JoinHandle<()>> {
        let mut map = self.exit_monitor_handles.write().await;
        map.remove(id)
    }

    async fn cleanup_workspace(&self, workspace: &Workspace) {
        // Worktree-disabled workspaces own no managed filesystem resources;
        // nothing to clean up, and we must not touch the user's real repo.
        if !workspace.use_worktree {
            return;
        }
        let Some(workspace_dir) = WorkspaceManager::workspace_dir_for(workspace) else {
            return;
        };

        let repositories = WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace.id)
            .await
            .unwrap_or_default();

        if repositories.is_empty() {
            tracing::warn!(
                "No repositories found for workspace {}, cleaning up workspace directory only",
                workspace.id
            );
            if workspace_dir.exists()
                && let Err(e) = tokio::fs::remove_dir_all(&workspace_dir).await
            {
                tracing::warn!("Failed to remove workspace directory: {}", e);
            }
        } else {
            WorkspaceManager::cleanup_workspace(&workspace_dir, &repositories)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        "Failed to clean up workspace for workspace {}: {}",
                        workspace.id,
                        e
                    );
                });
        }

        let _ = Workspace::mark_worktree_deleted(&self.db.pool, workspace.id).await;
    }

    async fn cleanup_expired_workspaces(&self) -> Result<(), DeploymentError> {
        if std::env::var("DISABLE_WORKTREE_CLEANUP").is_ok() {
            tracing::info!(
                "Expired workspace cleanup is disabled via DISABLE_WORKTREE_CLEANUP environment variable"
            );
            return Ok(());
        }

        let expired_workspaces = Workspace::find_expired_for_cleanup(&self.db.pool).await?;
        if expired_workspaces.is_empty() {
            tracing::debug!("No expired workspaces found");
            return Ok(());
        }
        tracing::info!(
            "Found {} expired workspaces to clean up",
            expired_workspaces.len()
        );
        for workspace in &expired_workspaces {
            self.cleanup_workspace(workspace).await;
        }
        Ok(())
    }

    /// Archive workspaces that have gone idle, so they stop accumulating.
    ///
    /// The idle threshold is floored at [`MIN_AUTO_ARCHIVE_IDLE_DAYS`] so that
    /// auto-archive can never fire before the 72-hour worktree retention window
    /// has already elapsed. Archiving therefore never shortens how long an
    /// idle worktree survives on disk, whatever the operator configures.
    async fn auto_archive_idle_workspaces(&self) -> Result<(), DeploymentError> {
        // Archiving moves a workspace from the 72-hour retention window into
        // the one-hour one, so it is upstream of worktree deletion and honours
        // the same kill switch. `pnpm run dev` sets this, which keeps a
        // developer's live workspaces untouched.
        if std::env::var("DISABLE_WORKTREE_CLEANUP").is_ok() {
            tracing::info!(
                "Auto-archive is disabled via DISABLE_WORKTREE_CLEANUP environment variable"
            );
            return Ok(());
        }
        let (enabled, idle_days) = {
            let config = self.config.read().await;
            (config.auto_archive_enabled, config.auto_archive_idle_days)
        };
        if !enabled {
            return Ok(());
        }
        let idle_days = idle_days.max(MIN_AUTO_ARCHIVE_IDLE_DAYS);

        let idle = Workspace::find_idle_for_auto_archive(&self.db.pool, idle_days).await?;
        if idle.is_empty() {
            tracing::debug!("No idle workspaces to auto-archive");
            return Ok(());
        }
        tracing::info!(
            "Auto-archiving {} workspaces idle for more than {} days",
            idle.len(),
            idle_days
        );
        for workspace_id in idle {
            if let Err(e) = self.archive_workspace(workspace_id).await {
                tracing::error!("Failed to auto-archive workspace {}: {}", workspace_id, e);
            }
        }
        Ok(())
    }

    fn spawn_workspace_cleanup(&self) {
        let container = self.clone();
        tokio::spawn(async move {
            container
                .workspace_manager
                .cleanup_orphan_workspaces()
                .await;

            let mut cleanup_interval =
                tokio::time::interval(tokio::time::Duration::from_secs(1800)); // 30 minutes
            loop {
                cleanup_interval.tick().await;
                tracing::info!("Starting periodic workspace cleanup...");
                // Archive first: a freshly archived workspace becomes eligible
                // for worktree cleanup on the next tick, not this one.
                container
                    .auto_archive_idle_workspaces()
                    .await
                    .unwrap_or_else(|e| {
                        tracing::error!("Failed to auto-archive idle workspaces: {}", e)
                    });
                container
                    .cleanup_expired_workspaces()
                    .await
                    .unwrap_or_else(|e| {
                        tracing::error!("Failed to clean up expired workspaces: {}", e)
                    });
            }
        });
    }

    /// Record the current HEAD commit for each repository as the "after" state.
    /// Errors are silently ignored since this runs after the main execution completes
    /// and failure should not block process finalization.
    async fn update_after_head_commits(&self, exec_id: Uuid) {
        if let Ok(ctx) = ExecutionProcess::load_context(&self.db.pool, exec_id).await {
            for repo in &ctx.repos {
                let Some(repo_path) = ctx.workspace.execution_dir(repo) else {
                    continue;
                };
                if let Ok(head) = self.git().get_head_info(&repo_path) {
                    let _ = ExecutionProcessRepoState::update_after_head_commit(
                        &self.db.pool,
                        exec_id,
                        repo.id,
                        &head.oid,
                    )
                    .await;
                }
            }
        }
    }

    /// Get the commit message based on the execution run reason.
    async fn get_commit_message(&self, ctx: &ExecutionContext) -> String {
        match ctx.execution_process.run_reason {
            ExecutionProcessRunReason::CodingAgent => {
                // Try to retrieve the task summary from the coding agent turn
                // otherwise fallback to default message
                match CodingAgentTurn::find_by_execution_process_id(
                    &self.db().pool,
                    ctx.execution_process.id,
                )
                .await
                {
                    Ok(Some(turn)) if turn.summary.is_some() => turn.summary.unwrap(),
                    Ok(_) => {
                        tracing::debug!(
                            "No summary found for execution process {}, using default message",
                            ctx.execution_process.id
                        );
                        format!(
                            "Commit changes from coding agent for workspace {}",
                            ctx.workspace.id
                        )
                    }
                    Err(e) => {
                        tracing::debug!(
                            "Failed to retrieve summary for execution process {}: {}",
                            ctx.execution_process.id,
                            e
                        );
                        format!(
                            "Commit changes from coding agent for workspace {}",
                            ctx.workspace.id
                        )
                    }
                }
            }
            ExecutionProcessRunReason::CleanupScript => {
                format!("Cleanup script changes for workspace {}", ctx.workspace.id)
            }
            _ => format!(
                "Changes from execution process {}",
                ctx.execution_process.id
            ),
        }
    }

    /// Check which repos have uncommitted changes. Fails if any repo is inaccessible.
    fn check_repos_for_changes(
        &self,
        workspace_root: &Path,
        repos: &[Repo],
    ) -> Result<Vec<(Repo, PathBuf)>, ContainerError> {
        let git = GitService::new();
        let mut repos_with_changes = Vec::new();

        for repo in repos {
            let worktree_path = workspace_root.join(&repo.name);

            match git.get_worktree_status(&worktree_path) {
                Ok(ws) if !ws.entries.is_empty() => {
                    repos_with_changes.push((repo.clone(), worktree_path));
                }
                Ok(_) => {
                    tracing::debug!("No changes in repo '{}'", repo.name);
                }
                Err(e) => {
                    return Err(ContainerError::Other(anyhow!(
                        "Pre-flight check failed for repo '{}': {}",
                        repo.name,
                        e
                    )));
                }
            }
        }

        Ok(repos_with_changes)
    }

    // Kept for possible post-v1 revival; no current call sites now that the
    // auto-commit chain has been removed.
    #[allow(dead_code)]
    async fn has_commits_from_execution(
        &self,
        ctx: &ExecutionContext,
    ) -> Result<bool, ContainerError> {
        let repo_states = ExecutionProcessRepoState::find_by_execution_process_id(
            &self.db.pool,
            ctx.execution_process.id,
        )
        .await?;

        for repo in &ctx.repos {
            let Some(repo_path) = ctx.workspace.execution_dir(repo) else {
                continue;
            };
            let current_head = self.git().get_head_info(&repo_path).ok().map(|h| h.oid);

            let before_head = repo_states
                .iter()
                .find(|s| s.repo_id == repo.id)
                .and_then(|s| s.before_head_commit.clone());

            if current_head != before_head {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Commit changes to each repo. Logs failures but continues with other repos.
    fn commit_repos(&self, repos_with_changes: Vec<(Repo, PathBuf)>, message: &str) -> bool {
        let mut any_committed = false;

        for (repo, worktree_path) in repos_with_changes {
            tracing::debug!(
                "Committing changes for repo '{}' at {:?}",
                repo.name,
                &worktree_path
            );

            match self.git().commit(&worktree_path, message) {
                Ok(true) => {
                    any_committed = true;
                    tracing::info!("Committed changes in repo '{}'", repo.name);
                }
                Ok(false) => {
                    tracing::warn!("No changes committed in repo '{}' (unexpected)", repo.name);
                }
                Err(e) => {
                    tracing::warn!("Failed to commit in repo '{}': {}", repo.name, e);
                }
            }
        }

        any_committed
    }

    /// Spawn a background task that polls the child process for completion and
    /// cleans up the execution entry when it exits.
    fn spawn_exit_monitor(
        &self,
        exec_id: &Uuid,
        exit_signal: Option<ExecutorExitSignal>,
    ) -> JoinHandle<()> {
        let exec_id = *exec_id;
        let child_store = self.child_store.clone();
        let msg_stores = self.msg_stores.clone();
        let db = self.db.clone();
        let config = self.config.clone();
        let container = self.clone();
        let analytics = self.analytics.clone();

        let mut process_exit_rx = self.spawn_os_exit_watcher(exec_id);

        tokio::spawn(async move {
            let mut exit_signal_future = exit_signal
                .map(|rx| rx.boxed()) // wait for result
                .unwrap_or_else(|| std::future::pending().boxed()); // no signal, stall forever

            let outcome = tokio::select! {
                // Exit signal with result.
                // Some coding agent processes do not automatically exit after processing the user request; instead the executor
                // signals when processing has finished to gracefully kill the process.
                exit_result = &mut exit_signal_future => {
                    // Executor signaled completion: kill group and use the provided result
                    if let Some(child_lock) = child_store.read().await.get(&exec_id).cloned() {
                        let mut child = child_lock.write().await ;
                        if let Err(err) = command::kill_process_group(&mut child).await {
                            tracing::error!("Failed to kill process group after exit signal: {} {}", exec_id, err);
                        }
                    }

                    NormalizedProcessOutcome::from_executor_signal(exit_result)
                }
                // Process exit
                exit_status_result = &mut process_exit_rx => {
                    NormalizedProcessOutcome::from_exit_status_result(
                        exit_status_result.unwrap_or_else(|e| Err(std::io::Error::other(e))),
                    )
                }
            };
            let (status, exit_code) = outcome.status_and_exit_code();

            let completed_attempt = match ExecutionProcess::complete_running_attempt(
                &db.pool,
                exec_id,
                status,
                exit_code,
                outcome.normalized_outcome(),
            )
            .await
            {
                Ok(completed) => completed,
                Err(e) => {
                    tracing::error!("Failed to update execution process completion: {}", e);
                    false
                }
            };

            if completed_attempt
                && let Ok(ctx) = ExecutionProcess::load_context(&db.pool, exec_id).await
            {
                // Update executor session summary if available
                if let Err(e) = container.update_executor_session_summary(&exec_id).await {
                    tracing::warn!("Failed to update executor session summary: {}", e);
                }

                let success = matches!(
                    ctx.execution_process.status,
                    ExecutionProcessStatus::Completed
                ) && exit_code == Some(0);

                let cleanup_done = matches!(
                    ctx.execution_process.run_reason,
                    ExecutionProcessRunReason::CleanupScript
                ) && !matches!(
                    ctx.execution_process.status,
                    ExecutionProcessStatus::Running
                );

                if success || cleanup_done {
                    // No app-level auto-commit and no auto-chained cleanup: just
                    // start whatever next_action the chain already has, if any.
                    if let Err(e) = container.try_start_next_action(&ctx).await {
                        tracing::error!("Failed to start next action after completion: {}", e);
                    }
                }

                let has_chained_follow_up = ctx
                    .execution_process
                    .executor_action()
                    .ok()
                    .and_then(|action| action.next_action())
                    .is_some();

                if matches!(
                    ctx.execution_process.run_reason,
                    ExecutionProcessRunReason::CodingAgent
                ) && let Err(error) =
                    SessionCommand::finish_execution(&db.pool, ctx.execution_process.id, success)
                        .await
                {
                    tracing::error!(
                        "Failed to finish commands for execution {}: {}",
                        ctx.execution_process.id,
                        error
                    );
                }

                if let Err(error) = container.dispatch_all_pending_commands().await {
                    tracing::error!("Failed to dispatch pending session commands: {}", error);
                }

                let has_running_agent = ExecutionProcess::has_running_coding_agent_for_session(
                    &db.pool,
                    ctx.session.id,
                )
                .await
                .unwrap_or(true);

                if container.should_finalize(&ctx) && !has_running_agent {
                    container.finalize_task(&ctx).await;
                }

                let should_mark_turn_unseen = matches!(
                    ctx.execution_process.run_reason,
                    ExecutionProcessRunReason::CodingAgent
                ) && !has_chained_follow_up
                    && !has_running_agent;

                if should_mark_turn_unseen
                    && let Err(error) = CodingAgentTurn::mark_unseen_by_execution_process_id(
                        &db.pool,
                        ctx.execution_process.id,
                    )
                    .await
                {
                    tracing::warn!(
                        "Failed to mark coding agent turn unseen for execution {}: {}",
                        ctx.execution_process.id,
                        error
                    );
                }
                // Fire analytics event when CodingAgent execution has finished
                if config.read().await.analytics_enabled
                    && matches!(
                        &ctx.execution_process.run_reason,
                        ExecutionProcessRunReason::CodingAgent
                    )
                    && let Some(analytics) = &analytics
                {
                    analytics.analytics_service.track_event(&analytics.user_id, "task_attempt_finished", Some(json!({
                        "workspace_id": ctx.workspace.id.to_string(),
                        "session_id": ctx.session.id.to_string(),
                        "execution_success": matches!(ctx.execution_process.status, ExecutionProcessStatus::Completed),
                        "exit_code": ctx.execution_process.exit_code,
                    })));
                }

                // Sync workspace to remote after CodingAgent execution
                if matches!(
                    &ctx.execution_process.run_reason,
                    ExecutionProcessRunReason::CodingAgent
                ) && let Some(client) = &container.remote_client
                {
                    let stats = diff_stream::compute_diff_stats(
                        &container.db.pool,
                        &container.git,
                        &ctx.workspace,
                    )
                    .await;
                    let workspace_name =
                        Workspace::find_by_id_with_status(&container.db.pool, ctx.workspace.id)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|ws| ws.workspace.name);
                    let client = client.clone();
                    let workspace_id = ctx.workspace.id;
                    let archived = ctx.workspace.archived;
                    tokio::spawn(async move {
                        remote_sync::sync_workspace_to_remote(
                            &client,
                            workspace_id,
                            workspace_name.map(Some),
                            Some(archived),
                            stats.as_ref(),
                        )
                        .await;
                    });
                }
            }

            if completed_attempt {
                // Now that commit/next-action/finalization steps for this process are complete,
                // capture the HEAD OID as the definitive "after" state (best-effort).
                container.update_after_head_commits(exec_id).await;
            }

            // Wait for DB persistence to complete before cleaning up MsgStore
            let db_stream_handle = container.take_db_stream_handle(&exec_id).await;
            if let Some(msg_arc) = msg_stores.write().await.remove(&exec_id) {
                msg_arc.push_finished();
            }
            if let Some(handle) = db_stream_handle {
                let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
            }

            // SIGKILL any orphaned children (e.g. MCP servers) still in the
            // process group. The executor itself is already done — either it
            // exited naturally or was killed in the exit-signal branch above.
            if let Some(child_lock) = child_store.read().await.get(&exec_id).cloned() {
                let mut child = child_lock.write().await;
                let _ = child.start_kill();
            }
            child_store.write().await.remove(&exec_id);
        })
    }

    fn spawn_os_exit_watcher(
        &self,
        exec_id: Uuid,
    ) -> tokio::sync::oneshot::Receiver<std::io::Result<std::process::ExitStatus>> {
        let (tx, rx) = tokio::sync::oneshot::channel::<std::io::Result<std::process::ExitStatus>>();
        let child_store = self.child_store.clone();
        tokio::spawn(async move {
            loop {
                let child_lock = {
                    let map = child_store.read().await;
                    map.get(&exec_id).cloned()
                };
                if let Some(child_lock) = child_lock {
                    let mut child_handler = child_lock.write().await;
                    match child_handler.try_wait() {
                        Ok(Some(status)) => {
                            let _ = tx.send(Ok(status));
                            break;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            break;
                        }
                    }
                } else {
                    let _ = tx.send(Err(io::Error::other(format!(
                        "Child handle missing for {exec_id}"
                    ))));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        });
        rx
    }

    async fn track_child_msgs_in_store(
        &self,
        id: Uuid,
        child: &mut AsyncGroupChild,
    ) -> Result<(), ContainerError> {
        let store = self
            .get_msg_store_by_id(&id)
            .await
            .ok_or_else(|| ContainerError::Other(anyhow!("MsgStore not found for execution")))?;
        let out = child.inner().stdout.take().expect("no stdout");
        let err = child.inner().stderr.take().expect("no stderr");

        // Map stdout bytes -> LogMsg::Stdout
        let out = ReaderStream::new(out)
            .map_ok(|chunk| LogMsg::Stdout(String::from_utf8_lossy(&chunk).into_owned()));

        // Map stderr bytes -> LogMsg::Stderr
        let err = ReaderStream::new(err)
            .map_ok(|chunk| LogMsg::Stderr(String::from_utf8_lossy(&chunk).into_owned()));

        // If you have a JSON Patch source, map it to LogMsg::JsonPatch too, then select all three.

        // Merge and forward into the store
        let merged = select(out, err); // Stream<Item = Result<LogMsg, io::Error>>
        store.clone().spawn_forwarder(merged);
        Ok(())
    }

    /// Create a live diff log stream for ongoing attempts for WebSocket
    /// Returns a stream that owns the filesystem watcher - when dropped, watcher is cleaned up
    async fn create_live_diff_stream(
        &self,
        args: diff_stream::DiffStreamArgs,
    ) -> Result<DiffStreamHandle, ContainerError> {
        diff_stream::create(args)
            .await
            .map_err(|e| ContainerError::Other(anyhow!("{e}")))
    }

    /// Extract the last assistant message from the MsgStore history
    fn extract_last_assistant_message(&self, exec_id: &Uuid) -> Option<String> {
        // Get the MsgStore for this execution
        let msg_stores = self.msg_stores.try_read().ok()?;
        let msg_store = msg_stores.get(exec_id)?;

        // Get the history and scan in reverse for the last assistant message
        let history = msg_store.get_history();

        for msg in history.iter().rev() {
            if let LogMsg::JsonPatch(patch) = msg {
                // Try to extract a NormalizedEntry from the patch
                if let Some((_, entry)) = extract_normalized_entry_from_patch(patch)
                    && matches!(entry.entry_type, NormalizedEntryType::AssistantMessage)
                {
                    let content = entry.content.trim();
                    if !content.is_empty() {
                        const MAX_SUMMARY_LENGTH: usize = 4096;
                        if content.len() > MAX_SUMMARY_LENGTH {
                            let truncated = truncate_to_char_boundary(content, MAX_SUMMARY_LENGTH);
                            return Some(format!("{truncated}..."));
                        }
                        return Some(content.to_string());
                    }
                }
            }
        }

        None
    }

    /// Update the coding agent turn summary with the final assistant message
    async fn update_executor_session_summary(&self, exec_id: &Uuid) -> Result<(), anyhow::Error> {
        // Check if there's a coding agent turn for this execution process
        let turn = CodingAgentTurn::find_by_execution_process_id(&self.db.pool, *exec_id).await?;

        if let Some(turn) = turn {
            // Only update if summary is not already set
            if turn.summary.is_none() {
                if let Some(summary) = self.extract_last_assistant_message(exec_id) {
                    CodingAgentTurn::update_summary(&self.db.pool, *exec_id, &summary).await?;
                } else {
                    tracing::debug!("No assistant message found for execution {}", exec_id);
                }
            }
        }

        Ok(())
    }

    /// Copy project files and workspace attachments to the workspace.
    /// Skips files that already exist (fast no-op if all exist).
    async fn copy_files_and_images(
        &self,
        workspace_dir: &Path,
        workspace: &Workspace,
    ) -> Result<(), ContainerError> {
        let repos = WorkspaceRepo::find_repos_with_copy_files(&self.db.pool, workspace.id).await?;

        for repo in &repos {
            if let Some(copy_files) = &repo.copy_files
                && !copy_files.trim().is_empty()
            {
                let worktree_path = workspace_dir.join(&repo.name);
                self.copy_project_files(&repo.path, &worktree_path, copy_files)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!(
                            "Failed to copy project files for repo '{}': {}",
                            repo.name,
                            e
                        );
                    });
            }
        }

        let agent_working_dir = Session::find_latest_by_workspace_id(&self.db.pool, workspace.id)
            .await?
            .and_then(|session| session.agent_working_dir);

        if let Err(e) = self
            .file_service
            .copy_files_by_workspace_to_worktree(
                workspace_dir,
                workspace.id,
                agent_working_dir.as_deref(),
            )
            .await
        {
            tracing::warn!("Failed to copy workspace files to workspace: {}", e);
        }

        Ok(())
    }

    /// Create workspace-level CLAUDE.md and AGENTS.md files that import from each repo.
    /// Uses the @import syntax to reference each repo's config files.
    /// Skips creating files if they already exist or if no repos have the source file.
    async fn create_workspace_config_files(
        workspace_dir: &Path,
        repos: &[Repo],
    ) -> Result<(), ContainerError> {
        const CONFIG_FILES: [&str; 2] = ["CLAUDE.md", "AGENTS.md"];

        for config_file in CONFIG_FILES {
            let workspace_config_path = workspace_dir.join(config_file);

            if workspace_config_path.exists() {
                tracing::trace!(
                    "Workspace config file {} already exists, skipping",
                    config_file
                );
                continue;
            }

            let mut import_lines = Vec::new();
            for repo in repos {
                let repo_config_path = workspace_dir.join(&repo.name).join(config_file);
                if repo_config_path.exists() {
                    import_lines.push(format!("@{}/{}", repo.name, config_file));
                }
            }

            if import_lines.is_empty() {
                tracing::trace!(
                    "No repos have {}, skipping workspace config creation",
                    config_file
                );
                continue;
            }

            let content = import_lines.join("\n") + "\n";
            if let Err(e) = tokio::fs::write(&workspace_config_path, &content).await {
                tracing::warn!(
                    "Failed to create workspace config file {}: {}",
                    config_file,
                    e
                );
                continue;
            }

            tracing::info!(
                "Created workspace {} with {} import(s)",
                config_file,
                import_lines.len()
            );
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
enum NormalizedProcessOutcome {
    Success {
        exit_code: i64,
    },
    Failure {
        exit_code: Option<i64>,
        /// Normalized classification when the executor observed a stable
        /// provider signal; `None` falls back to `Unknown` at read time.
        outcome: Option<NormalizedExecutionOutcome>,
    },
}

impl NormalizedProcessOutcome {
    fn from_executor_signal(
        result: Result<ExecutorExitResult, tokio::sync::oneshot::error::RecvError>,
    ) -> Self {
        match result {
            Ok(ExecutorExitResult::Success) => Self::Success { exit_code: 0 },
            Ok(ExecutorExitResult::Failure(outcome)) => Self::Failure {
                exit_code: Some(1),
                outcome,
            },
            Err(_) => Self::Failure {
                exit_code: Some(1),
                outcome: None,
            },
        }
    }

    fn from_exit_status_result(result: std::io::Result<std::process::ExitStatus>) -> Self {
        match result {
            Ok(status) => {
                let exit_code = status.code().unwrap_or(-1) as i64;
                if status.success() {
                    Self::Success { exit_code }
                } else {
                    Self::Failure {
                        exit_code: Some(exit_code),
                        outcome: None,
                    }
                }
            }
            Err(_) => Self::Failure {
                exit_code: None,
                outcome: None,
            },
        }
    }

    fn status_and_exit_code(&self) -> (ExecutionProcessStatus, Option<i64>) {
        match self {
            Self::Success { exit_code } => (ExecutionProcessStatus::Completed, Some(*exit_code)),
            Self::Failure { exit_code, .. } => (ExecutionProcessStatus::Failed, *exit_code),
        }
    }

    fn normalized_outcome(&self) -> Option<&NormalizedExecutionOutcome> {
        match self {
            Self::Success { .. } => None,
            Self::Failure { outcome, .. } => outcome.as_ref(),
        }
    }
}

#[async_trait]
impl ContainerService for LocalContainerService {
    fn msg_stores(&self) -> &Arc<RwLock<HashMap<Uuid, Arc<MsgStore>>>> {
        &self.msg_stores
    }

    fn db(&self) -> &DBService {
        &self.db
    }

    fn git(&self) -> &GitService {
        &self.git
    }

    fn notification_service(&self) -> &NotificationService {
        &self.notification_service
    }

    fn scheduler_lock(&self) -> &Mutex<()> {
        &self.scheduler_lock
    }

    async fn touch(&self, workspace: &Workspace) -> Result<(), ContainerError> {
        let now = Instant::now();

        // We debounce touches to avoid excessive database writes, which in SQLites causes DB locks
        let should_debounce = |last_touch: &Instant| -> bool {
            now.duration_since(*last_touch) < WORKSPACE_TOUCH_DEBOUNCE
        };

        // Quick check with read lock
        if self
            .workspace_touch_times
            .read()
            .await
            .get(&workspace.id)
            .is_some_and(should_debounce)
        {
            return Ok(());
        }

        let mut map = self.workspace_touch_times.write().await;
        // Clean up stale entries older than the debounce window, reduce memory usage over time
        map.retain(|_, time| should_debounce(time));
        // check in case another thread has touched already
        if map.get(&workspace.id).is_some_and(should_debounce) {
            return Ok(());
        }
        map.insert(workspace.id, now);
        drop(map);

        Workspace::touch(&self.db.pool, workspace.id).await?;
        Ok(())
    }

    async fn store_db_stream_handle(&self, id: Uuid, handle: JoinHandle<()>) {
        self.add_db_stream_handle(id, handle).await;
    }

    async fn take_db_stream_handle(&self, id: &Uuid) -> Option<JoinHandle<()>> {
        LocalContainerService::take_db_stream_handle(self, id).await
    }

    async fn git_branch_prefix(&self) -> String {
        self.config.read().await.git_branch_prefix.clone()
    }

    async fn workspace_to_current_dir(&self, workspace: &Workspace) -> PathBuf {
        if workspace.use_worktree {
            return PathBuf::from(workspace.container_ref.clone().unwrap_or_default());
        }
        // Worktree-disabled: use the first attached repo's real path (v1 is
        // single-repo-per-session; multi-repo OFF is a known limitation).
        WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace.id)
            .await
            .ok()
            .and_then(|repos| repos.into_iter().next())
            .map(|repo| repo.path)
            .unwrap_or_default()
    }

    async fn create(&self, workspace: &Workspace) -> Result<ContainerRef, ContainerError> {
        // Worktree-disabled workspaces don't materialize a container; leave
        // `container_ref = NULL` in DB and return an empty ref.
        if !workspace.use_worktree {
            return Ok(String::new());
        }

        let label = workspace.name.as_deref().unwrap_or("workspace");
        let workspace_dir_name = WorkspaceManager::dir_name_from_workspace(&workspace.id, label);
        let workspace_dir = WorkspaceManager::get_workspace_base_dir().join(&workspace_dir_name);

        let (repositories, workspace_inputs) = self.workspace_repo_inputs(workspace.id).await?;

        let created_workspace = WorkspaceManager::create_workspace(
            &workspace_dir,
            &workspace_inputs,
            &workspace.branch,
        )
        .await
        .map_err(Self::map_workspace_manager_error)?;

        // Copy project files and images to workspace
        self.copy_files_and_images(&created_workspace.workspace_dir, workspace)
            .await?;

        Self::create_workspace_config_files(&created_workspace.workspace_dir, &repositories)
            .await?;

        Workspace::update_container_ref(
            &self.db.pool,
            workspace.id,
            &created_workspace.workspace_dir.to_string_lossy(),
        )
        .await?;

        Ok(created_workspace
            .workspace_dir
            .to_string_lossy()
            .to_string())
    }

    async fn delete(&self, workspace: &Workspace) -> Result<(), ContainerError> {
        self.try_stop(workspace, true).await;
        self.cleanup_workspace(workspace).await;
        Ok(())
    }

    async fn ensure_container_exists(
        &self,
        workspace: &Workspace,
    ) -> Result<ContainerRef, ContainerError> {
        self.touch(workspace).await?;

        // Worktree-disabled workspaces have no container to materialize; the
        // execution location is each attached `Repo.path` directly.
        if !workspace.use_worktree {
            return Ok(String::new());
        }

        let (repositories, workspace_inputs) = self.workspace_repo_inputs(workspace.id).await?;

        let workspace_dir = if let Some(container_ref) = &workspace.container_ref {
            PathBuf::from(container_ref)
        } else {
            let label = workspace.name.as_deref().unwrap_or("workspace");
            let workspace_dir_name =
                WorkspaceManager::dir_name_from_workspace(&workspace.id, label);
            WorkspaceManager::get_workspace_base_dir().join(&workspace_dir_name)
        };

        WorkspaceManager::ensure_workspace_exists(
            &workspace_dir,
            &workspace_inputs,
            &workspace.branch,
        )
        .await
        .map_err(Self::map_workspace_manager_error)?;

        if workspace.container_ref.is_none() && workspace.use_worktree {
            Workspace::update_container_ref(
                &self.db.pool,
                workspace.id,
                &workspace_dir.to_string_lossy(),
            )
            .await?;
        }

        if workspace.worktree_deleted {
            Workspace::clear_worktree_deleted(&self.db.pool, workspace.id).await?;
        }

        // Copy project files and images (fast no-op if already exist)
        self.copy_files_and_images(&workspace_dir, workspace)
            .await?;

        Self::create_workspace_config_files(&workspace_dir, &repositories).await?;

        Ok(workspace_dir.to_string_lossy().to_string())
    }

    async fn is_container_clean(&self, workspace: &Workspace) -> Result<bool, ContainerError> {
        let Some(container_ref) = &workspace.container_ref else {
            return Ok(true);
        };

        let workspace_dir = PathBuf::from(container_ref);
        if !workspace_dir.exists() {
            return Ok(true);
        }

        let repositories =
            WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace.id).await?;

        for repo in &repositories {
            let worktree_path = workspace_dir.join(&repo.name);
            if worktree_path.exists() {
                let (uncommitted, untracked) =
                    self.git().get_worktree_change_counts(&worktree_path)?;
                if uncommitted > 0 || untracked > 0 {
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    async fn start_execution_inner(
        &self,
        workspace: &Workspace,
        execution_process: &ExecutionProcess,
        executor_action: &ExecutorAction,
    ) -> Result<(), ContainerError> {
        let repos = WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace.id).await?;

        // Resolve the executor's working directory:
        // - Worktree mode: the primary repo's worktree subdir under the
        //   managed container. Claude's CLAUDE.md discovery needs to land
        //   inside the repo, not in the synthetic parent.
        // - Direct mode: the primary repo's real on-disk path.
        let primary_repo = repos.first().ok_or(ContainerError::Other(anyhow!(
            "Workspace has no attached repo"
        )))?;
        let worktree_root: Option<PathBuf> = if workspace.use_worktree {
            Some(PathBuf::from(workspace.container_ref.as_ref().ok_or(
                ContainerError::Other(anyhow!("Container ref not found for workspace")),
            )?))
        } else {
            None
        };
        let current_dir = execution_current_dir(
            worktree_root.as_deref(),
            &primary_repo.name,
            &primary_repo.path,
            executor_action,
        );

        // The adapter decides whether it brokers approvals; an executor this
        // file has never heard of cannot end up silently unable to ask.
        let brokers_approvals = executor_action
            .base_executor()
            .and_then(CodingAgent::registered)
            .is_some_and(|agent| agent.brokers_approvals());
        let approvals_service: Arc<dyn ExecutorApprovalService> = if brokers_approvals {
            ExecutorApprovalBridge::new(
                self.approvals.clone(),
                self.db.clone(),
                self.notification_service.clone(),
                execution_process.id,
            )
        } else {
            Arc::new(NoopExecutorApprovalService {})
        };

        let repo_names: Vec<String> = repos.iter().map(|r| r.name.clone()).collect();
        // Absolute on-disk paths for every repo in order. Direct-mode repos
        // have arbitrary real locations; worktree-mode repos sit as siblings
        // under worktree_root. The list is ordered so `repo_paths[0]` is the
        // primary and `repo_paths[1..]` are the secondaries — the Claude
        // executor consumes the tail as `--add-dir` arguments.
        let repo_paths: Vec<PathBuf> = match &worktree_root {
            Some(root) => repos.iter().map(|r| root.join(&r.name)).collect(),
            None => repos.iter().map(|r| r.path.clone()).collect(),
        };
        let repo_context = RepoContext::new(current_dir.clone(), repo_names, repo_paths);

        let config = self.config.read().await;
        let commit_reminder_enabled = config.commit_reminder_enabled;
        let commit_reminder_prompt = config
            .commit_reminder_prompt
            .clone()
            .unwrap_or_else(|| DEFAULT_COMMIT_REMINDER_PROMPT.to_string());
        drop(config);
        let mut env = ExecutionEnv::new(
            repo_context,
            commit_reminder_enabled,
            commit_reminder_prompt,
        );

        // Always inject workspace/session context
        env.insert("VK_WORKSPACE_ID", workspace.id.to_string());
        env.insert("VK_WORKSPACE_BRANCH", &workspace.branch);
        // CDESKTOP_SESSION_ID lets the `cdesktop team` CLI identify the caller
        // session — used for lead-only spawn enforcement and to attribute peer
        // sends. Read by `npx-cli/src/cli.ts` team subcommand.
        env.insert(
            "CDESKTOP_SESSION_ID",
            execution_process.session_id.to_string(),
        );

        // Provider env goes into provider_vars so it overrides profile/cmd env
        // (applied last in ExecutionEnv::apply_to_command — highest precedence).
        if let Some(provider_env) = &executor_action.provider_env {
            tracing::debug!(keys = ?provider_env.keys().collect::<Vec<_>>(), "injecting provider env");
            env.provider_vars = provider_env.clone();
        }
        // Structured injection travels opaquely; only the harness that
        // emitted it can read it back out of the spawn env.
        if let Some(structured) = &executor_action.provider_structured {
            tracing::debug!(?structured, "injecting structured provider payload");
            env.provider_structured = Some(structured.clone());
        }

        // Create the child and stream, add to execution tracker with timeout
        let mut spawned = tokio::time::timeout(
            Duration::from_secs(30),
            executor_action.spawn(&current_dir, approvals_service, &env),
        )
        .await
        .map_err(|_| {
            ContainerError::Other(anyhow!(
                "Timeout: process took more than 30 seconds to start"
            ))
        })??;

        if let Err(e) = self
            .track_child_msgs_in_store(execution_process.id, &mut spawned.child)
            .await
        {
            let _ = command::kill_process_group(&mut spawned.child).await;
            return Err(e);
        }

        self.add_child_to_store(execution_process.id, spawned.child)
            .await;

        // Store cancellation token for graceful shutdown
        if let Some(cancel) = spawned.cancel {
            self.add_cancellation_token(execution_process.id, cancel)
                .await;
        }

        // Spawn unified exit monitor: watches OS exit and optional executor signal
        let hn = self.spawn_exit_monitor(&execution_process.id, spawned.exit_signal);
        self.add_exit_monitor_handle(execution_process.id, hn).await;

        Ok(())
    }

    async fn stop_execution(
        &self,
        execution_process: &ExecutionProcess,
        status: ExecutionProcessStatus,
    ) -> Result<(), ContainerError> {
        let Some(child) = self.get_child_from_store(&execution_process.id).await else {
            // No child in this server's store means the row is an orphan
            // (previous process, or the child was already reaped). The stop
            // must still reach a terminal state instead of erroring and
            // leaving the row running forever.
            tracing::warn!(
                execution_process_id = %execution_process.id,
                "stopping execution with no live child; terminalizing the orphan row"
            );
            let requeue = status == ExecutionProcessStatus::Killed;
            ExecutionProcess::update_completion(
                &self.db().pool,
                execution_process.id,
                status,
                None,
            )
            .await?;
            if requeue {
                SessionCommand::requeue_killed_execution(&self.db().pool, execution_process.id)
                    .await?;
            }
            return Ok(());
        };
        let exit_code = if status == ExecutionProcessStatus::Completed {
            Some(0)
        } else {
            None
        };

        // Try graceful cancellation first, then force kill
        if let Some(cancel) = self.take_cancellation_token(&execution_process.id).await {
            cancel.cancel();

            // Wait for exit monitor to finish gracefully
            if let Some(monitor_handle) = self.take_exit_monitor_handle(&execution_process.id).await
            {
                match tokio::time::timeout(Duration::from_secs(5), monitor_handle).await {
                    Ok(_) => {
                        tracing::debug!("Process {} exited gracefully", execution_process.id);
                    }
                    Err(_) => {
                        tracing::debug!(
                            "Graceful shutdown timed out for process {}, force killing",
                            execution_process.id
                        );
                    }
                }
            }
        }

        {
            let mut child_guard = child.write().await;
            if let Err(e) = command::kill_process_group(&mut child_guard).await {
                tracing::error!(
                    "Failed to stop execution process {}: {}",
                    execution_process.id,
                    e
                );
                return Err(e);
            }
        }

        // Terminal state is the durable record that the stop side effect has
        // completed. Never publish it before cancellation/kill succeeds: a
        // keyed-stop replay must not mistake an interrupted intent for a
        // stopped process after restart.
        ExecutionProcess::update_completion(&self.db.pool, execution_process.id, status, exit_code)
            .await?;
        self.remove_child_from_store(&execution_process.id).await;

        // Mark the process finished in the MsgStore and wait for DB persistence
        let db_stream_handle = self.take_db_stream_handle(&execution_process.id).await;
        if let Some(msg) = self.msg_stores.write().await.remove(&execution_process.id) {
            msg.push_finished();
        }
        if let Some(handle) = db_stream_handle {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }

        tracing::debug!(
            "Execution process {} stopped successfully",
            execution_process.id
        );

        // Record after-head commit OID (best-effort)
        self.update_after_head_commits(execution_process.id).await;

        Ok(())
    }

    async fn stream_diff(
        &self,
        workspace: &Workspace,
        stats_only: bool,
    ) -> Result<futures::stream::BoxStream<'static, Result<LogMsg, std::io::Error>>, ContainerError>
    {
        let workspace_repos =
            WorkspaceRepo::find_by_workspace_id(&self.db.pool, workspace.id).await?;
        let target_branches: HashMap<_, _> = workspace_repos
            .iter()
            .map(|wr| (wr.repo_id, wr.target_branch.clone()))
            .collect();

        let repositories: Vec<_> =
            WorkspaceRepo::find_repos_for_workspace(&self.db.pool, workspace.id)
                .await?
                .into_iter()
                .filter(|r| r.is_git)
                .collect();

        let mut streams = Vec::new();

        // In worktree mode, materialize the container first so worktree paths
        // exist; in direct mode, this is a no-op.
        self.ensure_container_exists(workspace).await?;

        for repo in repositories {
            let Some(worktree_path) = workspace.execution_dir(&repo) else {
                tracing::warn!(
                    "Skipping diff stream for repo {}: execution dir unresolved",
                    repo.name
                );
                continue;
            };
            let branch = &workspace.branch;

            let Some(target_branch) = target_branches.get(&repo.id) else {
                tracing::warn!(
                    "Skipping diff stream for repo {}: no target branch configured",
                    repo.name
                );
                continue;
            };

            let base_commit = match self
                .git()
                .get_base_commit(&repo.path, branch, target_branch)
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "Skipping diff stream for repo {}: failed to get base commit: {}",
                        repo.name,
                        e
                    );
                    continue;
                }
            };

            let stream = self
                .create_live_diff_stream(diff_stream::DiffStreamArgs {
                    git_service: self.git().clone(),
                    db: self.db().clone(),
                    workspace_id: workspace.id,
                    repo_id: repo.id,
                    repo_path: repo.path.clone(),
                    worktree_path: worktree_path.clone(),
                    branch: branch.to_string(),
                    target_branch: target_branch.clone(),
                    base_commit: base_commit.clone(),
                    stats_only,
                    path_prefix: Some(repo.name.clone()),
                })
                .await?;

            streams.push(Box::pin(stream));
        }

        if streams.is_empty() {
            return Ok(Box::pin(futures::stream::empty()));
        }

        // Merge all streams into one
        Ok(Box::pin(futures::stream::select_all(streams)))
    }

    async fn try_commit_changes(&self, ctx: &ExecutionContext) -> Result<bool, ContainerError> {
        if !matches!(
            ctx.execution_process.run_reason,
            ExecutionProcessRunReason::CodingAgent | ExecutionProcessRunReason::CleanupScript,
        ) {
            return Ok(false);
        }

        let message = self.get_commit_message(ctx).await;

        let container_ref = ctx
            .workspace
            .container_ref
            .as_ref()
            .ok_or_else(|| ContainerError::Other(anyhow!("Container reference not found")))?;
        let workspace_root = PathBuf::from(container_ref);

        let repos_with_changes = self.check_repos_for_changes(&workspace_root, &ctx.repos)?;
        if repos_with_changes.is_empty() {
            tracing::debug!("No changes to commit in any repository");
            return Ok(false);
        }

        Ok(self.commit_repos(repos_with_changes, &message))
    }

    /// Copy files from the original project directory to the worktree.
    /// Skips files that already exist at target with same size.
    async fn copy_project_files(
        &self,
        source_dir: &Path,
        target_dir: &Path,
        copy_files: &str,
    ) -> Result<(), ContainerError> {
        let source_dir = source_dir.to_path_buf();
        let target_dir = target_dir.to_path_buf();
        let copy_files = copy_files.to_string();

        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tokio::task::spawn_blocking(move || {
                copy::copy_project_files_impl(&source_dir, &target_dir, &copy_files)
            }),
        )
        .await
        .map_err(|_| ContainerError::Other(anyhow!("Copy project files timed out after 30s")))?
        .map_err(|e| ContainerError::Other(anyhow!("Copy files task failed: {e}")))?
    }

    async fn kill_all_running_processes(&self) -> Result<(), ContainerError> {
        tracing::info!("Killing all running processes");
        let running_processes = ExecutionProcess::find_running(&self.db.pool).await?;

        tracing::info!(
            "Found {} running processes to kill",
            running_processes.len()
        );

        for process in running_processes {
            tracing::info!(
                "Killing process: id={}, run_reason={:?}",
                process.id,
                process.run_reason
            );
            if let Err(error) = self
                .stop_execution(&process, ExecutionProcessStatus::Killed)
                .await
            {
                tracing::error!(
                    "Failed to cleanly kill running execution process {:?}: {:?}",
                    process,
                    error
                );
            } else {
                tracing::info!("Successfully killed process: id={}", process.id);
            }
        }

        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use executors::actions::script::{ScriptContext, ScriptRequest, ScriptRequestLanguage};
    use tokio::sync::oneshot;

    use super::*;

    fn exit_status(code: i32) -> std::process::ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            ExitStatusExt::from_raw(code << 8)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            ExitStatusExt::from_raw(code as u32)
        }
    }

    #[test]
    fn normalizes_exit_status_to_process_outcome() {
        assert_eq!(
            NormalizedProcessOutcome::from_exit_status_result(Ok(exit_status(0))),
            NormalizedProcessOutcome::Success { exit_code: 0 }
        );
        assert_eq!(
            NormalizedProcessOutcome::from_exit_status_result(Ok(exit_status(2))),
            NormalizedProcessOutcome::Failure {
                exit_code: Some(2),
                outcome: None
            }
        );
        assert_eq!(
            NormalizedProcessOutcome::from_exit_status_result(Err(std::io::Error::other(
                "missing child"
            ))),
            NormalizedProcessOutcome::Failure {
                exit_code: None,
                outcome: None
            }
        );
    }

    #[test]
    fn unknown_executor_signal_fails_closed() {
        let (tx, rx) = oneshot::channel::<ExecutorExitResult>();
        drop(tx);

        assert_eq!(
            NormalizedProcessOutcome::from_executor_signal(rx.blocking_recv()),
            NormalizedProcessOutcome::Failure {
                exit_code: Some(1),
                outcome: None
            }
        );
        assert_eq!(
            NormalizedProcessOutcome::from_executor_signal(Ok(ExecutorExitResult::Success)),
            NormalizedProcessOutcome::Success { exit_code: 0 }
        );
    }

    #[test]
    fn executor_signal_failure_preserves_normalized_outcome() {
        use executors::outcome::{ExecutionOutcomeClass, NormalizedExecutionOutcome};

        let outcome = NormalizedExecutionOutcome::new(ExecutionOutcomeClass::QuotaExhausted)
            .with_provider_code("usage_limit_exceeded");
        let normalized = NormalizedProcessOutcome::from_executor_signal(Ok(
            ExecutorExitResult::Failure(Some(outcome.clone())),
        ));

        assert_eq!(normalized.normalized_outcome(), Some(&outcome));
        assert_eq!(
            normalized.status_and_exit_code(),
            (ExecutionProcessStatus::Failed, Some(1))
        );
    }

    #[test]
    fn worktree_setup_script_resolves_repo_dir_from_workspace_root() {
        let root = Path::new("/tmp/workspace");
        let action = ExecutorAction::new(
            ExecutorActionType::ScriptRequest(ScriptRequest {
                script: "true".to_string(),
                language: ScriptRequestLanguage::Bash,
                context: ScriptContext::SetupScript,
                working_dir: Some("catapult-games".to_string()),
            }),
            None,
        );

        assert_eq!(
            execution_current_dir(
                Some(root),
                "catapult-games",
                Path::new("/source/catapult-games"),
                &action,
            ),
            root,
        );
    }
}
