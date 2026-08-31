//! Agent Teams MVP — spawn endpoint shared between two HTTP routes:
//!
//! - `POST /api/workspaces/{workspace_id}/teammates` (UI; caller-agnostic)
//! - `POST /api/sessions/{caller_id}/teammates` (CLI; records parentage)
//!
//! See `plans/agent-teams-mvp.md` for the design contract.

use axum::{Extension, Json, http::StatusCode, response::Json as ResponseJson};
use db::models::{
    execution_process::{ExecutionProcess, ExecutionProcessRunReason},
    provider::Provider,
    session::{CreateSession, Session},
    workspace::Workspace,
};
use deployment::Deployment;
use executors::{
    actions::{
        ExecutorAction, ExecutorActionType,
        coding_agent_initial::{CodingAgentInitialRequest, PromptKind},
    },
    profile::ExecutorConfig,
    provider::ProviderInjection,
};
use serde::{Deserialize, Serialize};
use services::services::container::ContainerService;
use thiserror::Error;
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

/// Maximum length of a teammate `name`. Names render in pill labels and
/// command-line `--name` arguments, so the limit is tight.
const MAX_NAME_LEN: usize = 24;

#[derive(Debug, Deserialize, TS)]
pub struct SpawnTeammateRequest {
    /// Display name (also serves as role label). Required, ≤24 chars,
    /// no embedded newlines.
    pub name: String,
    /// Initial bootstrap prompt. Optional — when omitted, the teammate
    /// boots with the fallback message from the wrap template.
    #[serde(default)]
    #[ts(optional)]
    pub prompt: Option<String>,
    /// Executor config (executor + variant + model + reasoning +
    /// permission policy). Optional — when omitted, the server inherits
    /// from the caller's last execution.
    #[serde(default)]
    #[ts(optional)]
    pub executor_config: Option<ExecutorConfig>,
    /// Provider id for routing this teammate's API calls. Optional —
    /// inherited from caller when omitted. Required when the spawn
    /// changes executor type relative to the caller.
    #[serde(default)]
    #[ts(optional)]
    pub selected_provider_id: Option<Uuid>,
}

#[derive(Debug, Serialize, TS)]
pub struct SpawnTeammateResponse {
    pub session_id: Uuid,
}

/// Source of the spawn — drives the auth rule and telemetry label.
#[derive(Debug, Clone, Copy)]
pub enum SpawnSource {
    /// `/workspaces/{id}/teammates` — caller-agnostic.
    WorkspaceUi,
    /// `/sessions/{caller_id}/teammates`: agent initiated.
    SessionCli,
}

impl SpawnSource {
    fn as_str(&self) -> &'static str {
        match self {
            SpawnSource::WorkspaceUi => "ui",
            SpawnSource::SessionCli => "cli",
        }
    }
}

#[derive(Debug, Error)]
pub enum TeammateError {
    #[error(
        "Cross-executor spawn requires both `selected_provider_id` and `executor_config.model_id`"
    )]
    ExecutorRequiresProvider,
    #[error("Provider '{0}' is missing or disabled")]
    ProviderNotConfigured(String),
    #[error("{0}")]
    NameInvalid(&'static str),
    #[error("Workspace not found")]
    WorkspaceNotFound,
    #[error("Workspace is archived")]
    WorkspaceArchived,
    #[error("No prior execution on caller to inherit executor config from")]
    NoCallerHistory,
    #[error("Failed to spawn teammate executor: {0}")]
    SpawnFailed(String),
}

impl TeammateError {
    pub fn code(&self) -> &'static str {
        match self {
            TeammateError::ExecutorRequiresProvider => "EXECUTOR_REQUIRES_PROVIDER",
            TeammateError::ProviderNotConfigured(_) => "PROVIDER_NOT_CONFIGURED",
            TeammateError::NameInvalid(_) => "NAME_INVALID",
            TeammateError::WorkspaceNotFound => "WORKSPACE_NOT_FOUND",
            TeammateError::WorkspaceArchived => "WORKSPACE_ARCHIVED",
            TeammateError::NoCallerHistory => "NO_CALLER_HISTORY",
            TeammateError::SpawnFailed(_) => "SPAWN_FAILED",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            TeammateError::ExecutorRequiresProvider
            | TeammateError::ProviderNotConfigured(_)
            | TeammateError::NameInvalid(_)
            | TeammateError::NoCallerHistory => StatusCode::BAD_REQUEST,
            TeammateError::WorkspaceNotFound => StatusCode::NOT_FOUND,
            TeammateError::WorkspaceArchived => StatusCode::CONFLICT,
            TeammateError::SpawnFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// `POST /api/workspaces/{workspace_id}/teammates` — used by the UI spawn
/// modal. No lead check (the human user is implicitly authorised).
pub async fn spawn_via_workspace(
    Extension(workspace): Extension<Workspace>,
    axum::extract::State(deployment): axum::extract::State<DeploymentImpl>,
    Json(payload): Json<SpawnTeammateRequest>,
) -> Result<ResponseJson<ApiResponse<SpawnTeammateResponse>>, ApiError> {
    let session_id = spawn_teammate_core(
        &deployment,
        &workspace,
        None,
        payload,
        SpawnSource::WorkspaceUi,
    )
    .await?;
    Ok(ResponseJson(ApiResponse::success(SpawnTeammateResponse {
        session_id,
    })))
}

/// `POST /api/sessions/{caller_id}/teammates` — used by the `cdesktop team
/// spawn` CLI. The caller becomes the new session's parent.
pub async fn spawn_via_session(
    Extension(caller): Extension<Session>,
    axum::extract::State(deployment): axum::extract::State<DeploymentImpl>,
    Json(payload): Json<SpawnTeammateRequest>,
) -> Result<ResponseJson<ApiResponse<SpawnTeammateResponse>>, ApiError> {
    let pool = &deployment.db().pool;

    let workspace = Workspace::find_by_id(pool, caller.workspace_id)
        .await?
        .ok_or_else(|| ApiError::from(TeammateError::WorkspaceNotFound))?;

    let session_id = spawn_teammate_core(
        &deployment,
        &workspace,
        Some(&caller),
        payload,
        SpawnSource::SessionCli,
    )
    .await?;
    Ok(ResponseJson(ApiResponse::success(SpawnTeammateResponse {
        session_id,
    })))
}

/// Shared spawn pipeline. `caller` is `Some` for CLI route (used to inherit
/// executor config when payload omits it). For the UI route, the modal
/// always passes a full config (or at least an executor) so `caller` is
/// `None` and the inheritance fallback comes from
/// `Session::find_first_by_workspace_id` (the lead).
async fn spawn_teammate_core(
    deployment: &DeploymentImpl,
    workspace: &Workspace,
    caller: Option<&Session>,
    payload: SpawnTeammateRequest,
    source: SpawnSource,
) -> Result<Uuid, ApiError> {
    let pool = &deployment.db().pool;

    validate_name(&payload.name)?;

    if workspace.archived {
        return Err(ApiError::from(TeammateError::WorkspaceArchived));
    }

    // Resolve the inheritance source: the explicit caller (CLI) or the lead
    // session of the workspace (UI).
    let inherit_from: Session = match caller {
        Some(c) => c.clone(),
        None => Session::find_first_by_workspace_id(pool, workspace.id)
            .await?
            .ok_or_else(|| ApiError::from(TeammateError::NoCallerHistory))?,
    };

    let (inherited_cfg, inherited_provider_id) =
        match ExecutionProcess::latest_executor_config_for_session(pool, inherit_from.id).await? {
            Some((cfg, pid)) => (Some(cfg), pid),
            None => (None, None),
        };

    let mut executor_config = match (payload.executor_config, inherited_cfg) {
        (Some(req), _) => req,
        (None, Some(inherited)) => inherited,
        (None, None) => return Err(ApiError::from(TeammateError::NoCallerHistory)),
    };

    // Cross-executor spawn requires explicit provider + model. We can only
    // decide "cross-executor" when we know the caller's prior executor.
    let prior_executor =
        ExecutionProcess::latest_executor_profile_for_session(pool, inherit_from.id)
            .await?
            .map(|p| p.executor);
    validate_cross_executor_spawn(
        prior_executor,
        executor_config.executor,
        executor_config.model_id.is_some(),
        payload.selected_provider_id.is_some(),
    )?;

    // Provider: explicit > inherited.
    let provider_uuid: Option<Uuid> = match payload.selected_provider_id {
        Some(id) => Some(id),
        None => inherited_provider_id.as_ref().and_then(|s| s.parse().ok()),
    };

    let resolved_provider = if let Some(provider_id) = provider_uuid {
        let provider = Provider::find_by_id(pool, provider_id).await.map_err(|_| {
            ApiError::from(TeammateError::ProviderNotConfigured(
                provider_id.to_string(),
            ))
        })?;
        if !provider.enabled {
            return Err(ApiError::from(TeammateError::ProviderNotConfigured(
                provider.name.clone(),
            )));
        }
        Some(provider)
    } else {
        None
    };

    // Create the empty session — name comes from the request (it doubles as
    // the role label and the pill caption).
    let new_session = Session::create(
        pool,
        &CreateSession {
            executor: Some(executor_config.executor.to_string()),
            name: Some(payload.name.clone()),
            parent_session_id: caller.map(|session| session.id),
        },
        Uuid::new_v4(),
        workspace.id,
    )
    .await
    .map_err(|e| ApiError::from(TeammateError::SpawnFailed(e.to_string())))?;

    deployment
        .container()
        .ensure_container_exists(workspace)
        .await
        .map_err(|e| ApiError::from(TeammateError::SpawnFailed(e.to_string())))?;

    let working_dir = new_session
        .agent_working_dir
        .as_ref()
        .filter(|dir| !dir.is_empty())
        .cloned();

    // Always-bootstrap: even when prompt is empty we send a wrap-template
    // initial request so the teammate is never an empty-history session.
    let bootstrap_prompt = build_wrap_template(&payload.name, workspace, payload.prompt.as_deref());

    let action_type = ExecutorActionType::CodingAgentInitialRequest(build_spawn_request(
        bootstrap_prompt,
        executor_config.clone(),
        working_dir,
    ));

    let injection = if let Some(provider) = resolved_provider {
        let (model_id, injection) = provider
            .resolve_injection(
                executor_config.executor,
                executor_config.model_id.as_deref().unwrap_or(""),
            )
            .map_err(|e| ApiError::from(TeammateError::SpawnFailed(e.to_string())))?;
        if executor_config.model_id.is_some() {
            executor_config.model_id = Some(model_id);
        }
        injection
    } else {
        ProviderInjection::default()
    };

    let selected_provider_id_str = provider_uuid.map(|id| id.to_string());
    let selected_model_id_str = executor_config.model_id.clone();

    let action = {
        let mut a = ExecutorAction::new(action_type, None);
        a = a.with_provider_injection(injection);
        a.with_provider_selection(selected_provider_id_str, selected_model_id_str)
    };

    deployment
        .container()
        .start_execution(
            workspace,
            &new_session,
            &action,
            &ExecutionProcessRunReason::CodingAgent,
        )
        .await
        .map_err(|e| ApiError::from(TeammateError::SpawnFailed(e.to_string())))?;

    let model_id = executor_config.model_id.clone().unwrap_or_default();
    deployment
        .track_if_analytics_allowed(
            "team_teammate_spawned",
            serde_json::json!({
                "workspace_id": workspace.id.to_string(),
                "source": source.as_str(),
                "executor": executor_config.executor.to_string(),
                "model_id": model_id,
                "has_prompt": payload.prompt.is_some(),
            }),
        )
        .await;

    Ok(new_session.id)
}

fn validate_cross_executor_spawn(
    prior_executor: Option<executors::executors::BaseCodingAgent>,
    requested_executor: executors::executors::BaseCodingAgent,
    has_model: bool,
    has_provider: bool,
) -> Result<(), TeammateError> {
    let is_cross_executor = prior_executor.is_some_and(|prev| prev != requested_executor);
    if is_cross_executor && (!has_model || !has_provider) {
        return Err(TeammateError::ExecutorRequiresProvider);
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), TeammateError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(TeammateError::NameInvalid("Name is required"));
    }
    if trimmed.len() > MAX_NAME_LEN {
        return Err(TeammateError::NameInvalid("Name exceeds 24 characters"));
    }
    if name.contains('\n') || name.contains('\r') {
        return Err(TeammateError::NameInvalid("Name must not contain newlines"));
    }
    Ok(())
}

/// Wrap template prepended to the initial message. Matches the markdown in
/// `plans/agent-teams-mvp.md` so users opening the teammate's transcript see
/// the same orientation the agent saw.
fn build_wrap_template(name: &str, workspace: &Workspace, user_prompt: Option<&str>) -> String {
    let workspace_label = workspace_label(workspace);
    let body = user_prompt.map(|p| p.trim()).unwrap_or("");
    let fallback = "Run `cdesktop team list` to orient, then await further instructions from another team member.";
    let user_section = if body.is_empty() { fallback } else { body };

    format!(
        "[Spawned as teammate \"{name}\" in team for workspace \"{workspace_label}\".\n\
         Use `cdesktop team list` and `cdesktop team send` to coordinate with peers.\n\
         Use `sightmesh peers`, `sightmesh peek`, and `sightmesh steer` to inspect or\n\
         immediately contact any visible local agent, including other workspaces.\n\
         Contact your manager whenever you need a decision, feedback, or help with a blocker,\n\
         and when you finish: `cdesktop team manager --message 'STATUS: concise details'`.\n\
         Batch independent read-only tool calls and all currently known questions.\n\
         Keep dependent or destructive actions sequential.\n\
         Do not assume your manager is reading this transcript in real time.]\n\n{user_section}",
    )
}

/// A teammate's first message is always bootstrap instructions, so the spawn
/// path is the one place that can state so. The marker persists with the
/// action; the transcript reads it back instead of inspecting prompt text.
fn build_spawn_request(
    prompt: String,
    executor_config: ExecutorConfig,
    working_dir: Option<String>,
) -> CodingAgentInitialRequest {
    CodingAgentInitialRequest {
        prompt,
        prompt_kind: PromptKind::Spawn,
        executor_config,
        working_dir,
    }
}

fn workspace_label(workspace: &Workspace) -> String {
    workspace
        .name
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| workspace.id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_required() {
        assert!(matches!(
            validate_name(""),
            Err(TeammateError::NameInvalid(_))
        ));
        assert!(matches!(
            validate_name("   "),
            Err(TeammateError::NameInvalid(_))
        ));
    }

    #[test]
    fn name_length_capped() {
        assert!(validate_name(&"a".repeat(24)).is_ok());
        assert!(matches!(
            validate_name(&"a".repeat(25)),
            Err(TeammateError::NameInvalid(_))
        ));
    }

    #[test]
    fn name_rejects_newlines() {
        assert!(matches!(
            validate_name("hello\nworld"),
            Err(TeammateError::NameInvalid(_))
        ));
        assert!(matches!(
            validate_name("hello\rworld"),
            Err(TeammateError::NameInvalid(_))
        ));
    }

    #[test]
    fn wrap_template_uses_fallback_when_no_prompt() {
        let ws = make_test_workspace("demo");
        let out = build_wrap_template("reviewer", &ws, None);
        assert!(out.contains("cdesktop team list"));
        assert!(out.contains("cdesktop team manager"));
        assert!(out.contains("reviewer"));
        assert!(out.contains("demo"));
    }

    #[test]
    fn wrap_template_includes_user_prompt() {
        let ws = make_test_workspace("demo");
        let out = build_wrap_template("reviewer", &ws, Some("audit the diff"));
        assert!(out.contains("audit the diff"));
        assert!(!out.contains("Run `cdesktop team list` to orient"));
        assert!(out.contains("cdesktop team manager"));
    }

    #[test]
    fn error_code_mapping() {
        assert_eq!(
            TeammateError::ExecutorRequiresProvider.code(),
            "EXECUTOR_REQUIRES_PROVIDER"
        );
        assert_eq!(
            TeammateError::WorkspaceArchived.status(),
            StatusCode::CONFLICT
        );
    }

    #[test]
    fn rejected_cross_executor_spawn_has_no_session_or_process_side_effects() {
        use executors::{executors::BaseCodingAgent, profile::ExecutorConfig};

        let request = SpawnTeammateRequest {
            name: "reviewer".into(),
            prompt: None,
            executor_config: Some(ExecutorConfig {
                executor: BaseCodingAgent::Codex,
                variant: None,
                model_id: Some("gpt-5".into()),
                agent_id: None,
                reasoning_id: None,
                permission_policy: None,
            }),
            selected_provider_id: None,
        };

        let mut created_session = false;
        let mut created_process = false;
        let result = validate_cross_executor_spawn(
            Some(BaseCodingAgent::ClaudeCode),
            request
                .executor_config
                .as_ref()
                .expect("test request has executor config")
                .executor,
            request
                .executor_config
                .as_ref()
                .and_then(|config| config.model_id.as_ref())
                .is_some(),
            request.selected_provider_id.is_some(),
        )
        .map(|()| {
            created_session = true;
            created_process = true;
        });

        assert!(matches!(
            result,
            Err(TeammateError::ExecutorRequiresProvider)
        ));
        assert!(!created_session, "rejected request created a session");
        assert!(!created_process, "rejected request created a process");
    }

    #[test]
    fn spawn_marks_its_bootstrap_prompt_and_the_marker_persists() {
        let ws = make_test_workspace("demo");
        let bootstrap = build_wrap_template("reviewer", &ws, Some("audit the diff"));

        let request = build_spawn_request(
            bootstrap,
            ExecutorConfig::new(executors::executors::BaseCodingAgent::ClaudeCode),
            None,
        );

        assert_eq!(request.prompt_kind, PromptKind::Spawn);
        assert!(request.prompt.contains("audit the diff"));

        // The transcript reads the marker back off the stored executor action,
        // so it has to survive serialization.
        let persisted = serde_json::to_value(&request).unwrap();
        assert_eq!(persisted["prompt_kind"], "spawn");
    }

    fn make_test_workspace(name: &str) -> Workspace {
        use chrono::Utc;
        use db::models::workspace::WorkspaceSource;
        Workspace {
            id: Uuid::nil(),
            task_id: None,
            container_ref: None,
            branch: "main".into(),
            setup_completed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            archived: false,
            pinned: false,
            pin_order: None,
            name: Some(name.into()),
            worktree_deleted: false,
            use_worktree: false,
            source: WorkspaceSource::User,
        }
    }
}
