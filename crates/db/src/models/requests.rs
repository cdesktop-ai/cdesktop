use executors::profile::ExecutorConfig;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::{execution_process::ExecutionProcess, workspace::Workspace};

#[derive(Debug, Deserialize, Serialize)]
pub struct ContainerQuery {
    #[serde(rename = "ref")]
    pub container_ref: String,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct WorkspaceRepoInput {
    pub repo_id: Uuid,
    pub target_branch: String,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct CreateWorkspaceApiRequest {
    pub name: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub use_worktree: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct LinkedIssueInfo {
    pub remote_project_id: Uuid,
    pub issue_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct CreateAndStartWorkspaceRequest {
    pub name: Option<String>,
    pub repos: Vec<WorkspaceRepoInput>,
    pub linked_issue: Option<LinkedIssueInfo>,
    pub executor_config: ExecutorConfig,
    pub prompt: String,
    pub attachment_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    #[ts(optional)]
    pub use_worktree: Option<bool>,
    /// Provider to route the first message through. None = ambient auth.
    #[serde(default)]
    #[ts(optional)]
    pub selected_provider_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct CreateAndStartWorkspaceResponse {
    pub workspace: Workspace,
    pub execution_process: ExecutionProcess,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct UpdateWorkspace {
    pub archived: Option<bool>,
    pub pinned: Option<bool>,
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
pub struct UpdateSession {
    pub name: Option<String>,
    pub parent_session_id: Option<Uuid>,
}

/// Atomic reorder of the pinned-workspace list. The provided `ordered_ids`
/// becomes the complete pinned set in the given order; workspaces not in the
/// list are unpinned.
#[derive(Debug, Serialize, Deserialize, TS)]
pub struct ReorderPinsRequest {
    pub ordered_ids: Vec<Uuid>,
}
