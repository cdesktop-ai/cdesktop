use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use axum::{Json, extract::State, response::Json as ResponseJson};
use db::models::{
    coding_agent_turn::CodingAgentTurn,
    execution_process::{ExecutionProcess, ExecutionProcessStatus},
    merge::MergeStatus,
    pull_request::PullRequest,
    workspace::Workspace,
    workspace_repo::{PrimaryRepoInfo, WorkspaceRepo},
};
use deployment::Deployment;
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

static GIT_REFRESH_SEMAPHORE: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(1)));
static GIT_REFRESHES: LazyLock<Mutex<HashMap<Uuid, (Uuid, CancellationToken)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Only queued refreshes are superseded. Once Git is running it is deliberately
/// allowed to finish under the single global permit rather than being killed.
struct RefreshClaim {
    workspace_id: Uuid,
    generation: Uuid,
    cancellation: CancellationToken,
}
impl RefreshClaim {
    fn replace(workspace_id: Uuid) -> Self {
        let cancellation = CancellationToken::new();
        let generation = Uuid::new_v4();
        let mut refreshes = GIT_REFRESHES.lock().expect("git refresh lock poisoned");
        if let Some((_, previous)) =
            refreshes.insert(workspace_id, (generation, cancellation.clone()))
        {
            previous.cancel();
        }
        Self {
            workspace_id,
            generation,
            cancellation,
        }
    }
    async fn acquire(&self) -> Option<OwnedSemaphorePermit> {
        if self.cancellation.is_cancelled() {
            return None;
        }
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => None,
            permit = GIT_REFRESH_SEMAPHORE.clone().acquire_owned() => permit.ok(),
        }
    }
}
impl Drop for RefreshClaim {
    fn drop(&mut self) {
        let mut refreshes = GIT_REFRESHES.lock().expect("git refresh lock poisoned");
        if refreshes
            .get(&self.workspace_id)
            .is_some_and(|(generation, _)| generation == &self.generation)
        {
            refreshes.remove(&self.workspace_id);
        }
    }
}

/// Request for fetching workspace summaries
#[derive(Debug, Deserialize, Serialize, TS)]
pub struct WorkspaceSummaryRequest {
    pub archived: bool,
    /// Git truth is opt-in and strictly task-local; fleet lists are metadata-only.
    #[serde(default)]
    pub include_git: bool,
    #[serde(default)]
    pub refresh_workspace_id: Option<Uuid>,
}

/// Summary info for a single workspace
#[derive(Debug, Serialize, TS)]
pub struct WorkspaceSummary {
    pub workspace_id: Uuid,
    /// Session ID of the latest execution process
    pub latest_session_id: Option<Uuid>,
    /// Is a tool approval currently pending?
    pub has_pending_approval: bool,
    /// Number of files with changes
    pub files_changed: Option<usize>,
    /// Total lines added across all files
    pub lines_added: Option<usize>,
    /// Total lines removed across all files
    pub lines_removed: Option<usize>,
    /// When the latest execution process completed
    #[ts(optional)]
    pub latest_process_completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Status of the latest execution process
    pub latest_process_status: Option<ExecutionProcessStatus>,
    /// Is a dev server currently running?
    pub has_running_dev_server: bool,
    /// Does this workspace have unseen coding agent turns?
    pub has_unseen_turns: bool,
    /// PR status for this workspace (if any PR exists)
    pub pr_status: Option<MergeStatus>,
    /// PR number for this workspace (if any PR exists)
    pub pr_number: Option<i64>,
    /// PR URL for this workspace (if any PR exists)
    pub pr_url: Option<String>,
    /// Primary (alphabetically first) attached repo — drives sidebar folder grouping.
    pub primary_repo: Option<PrimaryRepoInfo>,
}

/// Response containing summaries for requested workspaces
#[derive(Debug, Serialize, TS)]
pub struct WorkspaceSummaryResponse {
    pub summaries: Vec<WorkspaceSummary>,
}

#[derive(Debug, Clone, Default, Serialize, TS)]
pub struct DiffStats {
    pub files_changed: usize,
    pub lines_added: usize,
    pub lines_removed: usize,
}

/// Fetch summary information for workspaces filtered by archived status.
/// This endpoint returns data that cannot be efficiently included in the streaming endpoint.
#[axum::debug_handler]
pub async fn get_workspace_summaries(
    State(deployment): State<DeploymentImpl>,
    Json(request): Json<WorkspaceSummaryRequest>,
) -> Result<ResponseJson<ApiResponse<WorkspaceSummaryResponse>>, ApiError> {
    let pool = &deployment.db().pool;
    let archived = request.archived;

    // 1. Fetch all workspaces with the given archived status
    let workspaces: Vec<Workspace> = Workspace::find_all_with_status(pool, Some(archived), None)
        .await?
        .into_iter()
        .map(|ws| ws.workspace)
        .collect();

    if workspaces.is_empty() {
        return Ok(ResponseJson(ApiResponse::success(
            WorkspaceSummaryResponse { summaries: vec![] },
        )));
    }

    // 2. Fetch latest process info for workspaces with this archived status
    let latest_processes = ExecutionProcess::find_latest_for_workspaces(pool, archived).await?;

    // 3. Check which workspaces have running dev servers
    let dev_server_workspaces =
        ExecutionProcess::find_workspaces_with_running_dev_servers(pool, archived).await?;

    // 4. Check pending approvals for running processes
    let running_ep_ids: Vec<_> = latest_processes
        .values()
        .filter(|info| info.status == ExecutionProcessStatus::Running)
        .map(|info| info.execution_process_id)
        .collect();
    let pending_approval_eps = deployment
        .approvals()
        .get_pending_execution_process_ids(&running_ep_ids);

    // 5. Check which workspaces have unseen coding agent turns
    let unseen_workspaces = CodingAgentTurn::find_workspaces_with_unseen(pool, archived).await?;

    // 6. Get PR status for each workspace
    let pr_statuses = PullRequest::get_latest_for_workspaces(pool, archived).await?;

    // 6b. Primary repo per workspace (drives sidebar folder grouping)
    let primary_repos = WorkspaceRepo::find_primary_repos_for_archived(pool, archived).await?;

    // 7. The default list never observes Git. A caller may explicitly refresh
    // one non-archived, running workspace; terminal and archived rows are
    // filtered before any refresh claim is created.
    let diff_stats = if request.include_git && !archived {
        match request
            .refresh_workspace_id
            .and_then(|id| workspaces.iter().find(|workspace| workspace.id == id))
        {
            Some(workspace)
                if workspace.container_ref.is_some()
                    && latest_processes.get(&workspace.id).is_some_and(|process| {
                        process.status == ExecutionProcessStatus::Running
                    }) =>
            {
                compute_workspace_diff_stats(&deployment, workspace)
                    .await
                    .map(|stats| HashMap::from([(workspace.id, stats)]))
                    .unwrap_or_default()
            }
            _ => HashMap::new(),
        }
    } else {
        HashMap::new()
    };

    // 8. Assemble response
    let summaries: Vec<WorkspaceSummary> = workspaces
        .iter()
        .map(|ws| {
            let id = ws.id;
            let latest = latest_processes.get(&id);
            let has_pending = latest
                .map(|p| pending_approval_eps.contains(&p.execution_process_id))
                .unwrap_or(false);
            let stats = diff_stats.get(&id);

            WorkspaceSummary {
                workspace_id: id,
                latest_session_id: latest.map(|p| p.session_id),
                has_pending_approval: has_pending,
                files_changed: stats.map(|s| s.files_changed),
                lines_added: stats.map(|s| s.lines_added),
                lines_removed: stats.map(|s| s.lines_removed),
                latest_process_completed_at: latest.and_then(|p| p.completed_at),
                latest_process_status: latest.map(|p| p.status.clone()),
                has_running_dev_server: dev_server_workspaces.contains(&id),
                has_unseen_turns: unseen_workspaces.contains(&id),
                pr_status: pr_statuses.get(&id).map(|pr| pr.pr_status.clone()),
                pr_number: pr_statuses.get(&id).map(|pr| pr.pr_number),
                pr_url: pr_statuses.get(&id).map(|pr| pr.pr_url.clone()),
                primary_repo: primary_repos.get(&id).cloned(),
            }
        })
        .collect();

    Ok(ResponseJson(ApiResponse::success(
        WorkspaceSummaryResponse { summaries },
    )))
}

/// Compute diff stats for a workspace.
pub async fn compute_workspace_diff_stats(
    deployment: &DeploymentImpl,
    workspace: &Workspace,
) -> Option<DiffStats> {
    let claim = RefreshClaim::replace(workspace.id);
    let _permit = claim.acquire().await?;
    if claim.cancellation.is_cancelled() {
        return None;
    }
    let stats = services::services::diff_stream::compute_diff_stats(
        &deployment.db().pool,
        deployment.git(),
        workspace,
    )
    .await?;

    Some(DiffStats {
        files_changed: stats.files_changed,
        lines_added: stats.lines_added,
        lines_removed: stats.lines_removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_summary_is_metadata_only_even_for_a_large_live_fleet() {
        let request = WorkspaceSummaryRequest {
            archived: false,
            include_git: false,
            refresh_workspace_id: None,
        };
        assert!(!request.include_git);
        // The selection predicate is false before a workspace refresh future can exist.
        assert!(!(request.include_git && !request.archived));
    }

    #[tokio::test]
    async fn queued_refresh_is_cancelled_when_superseded() {
        let id = Uuid::new_v4();
        let first = RefreshClaim::replace(id);
        let _second = RefreshClaim::replace(id);
        assert!(first.cancellation.is_cancelled());
        assert!(first.acquire().await.is_none());
    }
}
