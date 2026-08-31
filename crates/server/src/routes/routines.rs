use axum::{
    Json, Router,
    extract::{Path, State},
    response::Json as ResponseJson,
    routing::{get, post},
};
use db::models::{
    requests::WorkspaceRepoInput,
    routine::{CreateRoutine, Routine, ScheduleKind, UpdateRoutine},
    routine_run::{RoutineRun, RoutineRunStatus},
    workspace::{Workspace, WorkspaceSource},
};
use deployment::Deployment;
use executors::profile::ExecutorConfig;
use serde::Serialize;
use services::services::container::ContainerService;
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;
use workspace_manager::WorkspaceManager;

use crate::{
    DeploymentImpl, error::ApiError, provider_injection::build_injection_for_provider,
    routes::workspaces::create::create_workspace_record, scheduling::compute_next_run_at,
};

#[derive(Debug, Serialize, TS)]
pub struct RunNowResponse {
    pub routine_run: RoutineRun,
    pub workspace_id: Option<Uuid>,
    pub skipped: bool,
    pub skip_reason: Option<String>,
}

fn validate_executor_config(cfg: &ExecutorConfig) -> Result<(), ApiError> {
    // model_id is optional — null means "use the agent's ambient default
    // model" (the AGENT_DEFAULT sentinel from the picker). When set, it
    // may be "provider/model" or a bare model id; spawn parses it.
    if cfg.permission_policy.is_none() {
        return Err(ApiError::BadRequest(
            "executor_config.permission_policy is required".into(),
        ));
    }
    Ok(())
}

fn validate_schedule(
    kind: ScheduleKind,
    time: Option<&str>,
    dow: Option<i64>,
) -> Result<(), ApiError> {
    match kind {
        ScheduleKind::Manual => Ok(()),
        ScheduleKind::Hourly => match time {
            Some(mm) if mm.len() == 2 && mm.chars().all(|c| c.is_ascii_digit()) => {
                let mm_val: u32 = mm.parse().unwrap();
                if mm_val > 59 {
                    Err(ApiError::BadRequest(
                        "schedule_time for hourly must be 00-59".into(),
                    ))
                } else {
                    Ok(())
                }
            }
            _ => Err(ApiError::BadRequest(
                "schedule_time for hourly must be MM (00-59)".into(),
            )),
        },
        ScheduleKind::Daily | ScheduleKind::Weekdays => match time {
            Some(t) if parse_hhmm(t).is_some() => Ok(()),
            _ => Err(ApiError::BadRequest(
                "schedule_time for daily/weekdays must be HH:MM".into(),
            )),
        },
        ScheduleKind::Weekly => {
            let _hhmm = parse_hhmm(time.unwrap_or(""))
                .ok_or_else(|| ApiError::BadRequest("schedule_time HH:MM required".into()))?;
            match dow {
                Some(d) if (0..=6).contains(&d) => Ok(()),
                _ => Err(ApiError::BadRequest(
                    "schedule_dow must be 0-6 for weekly".into(),
                )),
            }
        }
    }
}

fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some((h, m))
}

pub async fn list_routines(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<Routine>>>, ApiError> {
    let routines = Routine::list(&deployment.db().pool).await?;
    Ok(ResponseJson(ApiResponse::success(routines)))
}

pub async fn get_routine(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<Routine>>, ApiError> {
    let routine = Routine::find_by_id(&deployment.db().pool, id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Routine not found".into()))?;
    Ok(ResponseJson(ApiResponse::success(routine)))
}

pub async fn create_routine(
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<CreateRoutine>,
) -> Result<ResponseJson<ApiResponse<Routine>>, ApiError> {
    validate_executor_config(&payload.executor_config)?;
    validate_schedule(
        payload.schedule_kind,
        payload.schedule_time.as_deref(),
        payload.schedule_dow,
    )?;
    let next_run_at = compute_next_run_at(
        payload.schedule_kind,
        payload.schedule_time.as_deref(),
        payload.schedule_dow,
        payload.enabled,
        chrono::Local::now(),
    );
    let id = Uuid::new_v4();
    let routine = Routine::create(&deployment.db().pool, &payload, id, next_run_at).await?;
    Ok(ResponseJson(ApiResponse::success(routine)))
}

pub async fn update_routine(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateRoutine>,
) -> Result<ResponseJson<ApiResponse<Routine>>, ApiError> {
    let pool = &deployment.db().pool;
    let existing = Routine::find_by_id(pool, id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Routine not found".into()))?;

    if let Some(cfg) = &payload.executor_config {
        validate_executor_config(cfg)?;
    }
    let new_kind = payload.schedule_kind.unwrap_or(existing.schedule_kind);
    let new_time = payload
        .schedule_time
        .as_deref()
        .or(existing.schedule_time.as_deref());
    let new_dow = payload.schedule_dow.or(existing.schedule_dow);
    validate_schedule(new_kind, new_time, new_dow)?;

    let executor_config_json = match payload.executor_config {
        Some(cfg) => Some(
            serde_json::to_string(&cfg)
                .map_err(|e| ApiError::BadRequest(format!("invalid executor_config: {e}")))?,
        ),
        None => None,
    };

    let name = payload.name.clone();
    let description = payload.description.clone();
    let instructions = payload.instructions.clone();
    let target_branch = payload.target_branch.clone();
    let use_worktree = payload.use_worktree;
    let schedule_kind_str = payload.schedule_kind.map(|k| k.to_string());
    let schedule_time = payload.schedule_time.clone();
    let schedule_dow = payload.schedule_dow;
    let enabled = payload.enabled;

    sqlx::query!(
        r#"UPDATE routines SET
            name           = COALESCE($2, name),
            description    = COALESCE($3, description),
            instructions   = COALESCE($4, instructions),
            target_branch  = COALESCE($5, target_branch),
            use_worktree   = COALESCE($6, use_worktree),
            executor_config = COALESCE($7, executor_config),
            schedule_kind  = COALESCE($8, schedule_kind),
            schedule_time  = COALESCE($9, schedule_time),
            schedule_dow   = COALESCE($10, schedule_dow),
            enabled        = COALESCE($11, enabled),
            updated_at     = datetime('now','subsec')
           WHERE id = $1"#,
        id,
        name,
        description,
        instructions,
        target_branch,
        use_worktree,
        executor_config_json,
        schedule_kind_str,
        schedule_time,
        schedule_dow,
        enabled,
    )
    .execute(pool)
    .await?;

    let routine = Routine::find_by_id(pool, id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Routine not found".into()))?;

    // Recompute next_run_at when schedule or enable state changed.
    let schedule_changed = payload.schedule_kind.is_some()
        || payload.schedule_time.is_some()
        || payload.schedule_dow.is_some()
        || payload.enabled.is_some();
    if schedule_changed {
        let next = compute_next_run_at(
            routine.schedule_kind,
            routine.schedule_time.as_deref(),
            routine.schedule_dow,
            routine.enabled,
            chrono::Local::now(),
        );
        Routine::set_next_run_at(pool, routine.id, next).await?;
    }
    let routine = Routine::find_by_id(pool, id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Routine not found".into()))?;
    Ok(ResponseJson(ApiResponse::success(routine)))
}

pub async fn delete_routine(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    let pool = &deployment.db().pool;
    // Gather workspaces to wipe before cascading the routine itself.
    let workspace_ids = RoutineRun::workspace_ids_for_routine(pool, id).await?;
    for workspace_id in workspace_ids {
        if let Ok(Some(ws)) = Workspace::find_by_id(pool, workspace_id).await {
            let managed = deployment
                .workspace_manager()
                .load_managed_workspace(ws)
                .await?;
            let ctx = managed.prepare_deletion_context().await?;
            let _ = managed.delete_record().await?;
            WorkspaceManager::spawn_workspace_deletion_cleanup(ctx, true);
        }
    }
    Routine::delete(pool, id).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

pub async fn list_runs(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<Vec<RoutineRun>>>, ApiError> {
    let runs = RoutineRun::list_by_routine(&deployment.db().pool, id).await?;
    Ok(ResponseJson(ApiResponse::success(runs)))
}

pub async fn run_now(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<RunNowResponse>>, ApiError> {
    let pool = &deployment.db().pool;
    let now = chrono::Utc::now();

    let routine = Routine::find_by_id(pool, id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Routine not found".into()))?;

    let run_id = Uuid::new_v4();

    if RoutineRun::has_running(pool, id).await? {
        let run = RoutineRun::create(
            pool,
            run_id,
            id,
            now,
            RoutineRunStatus::Skipped,
            Some("prev_still_running".to_string()),
        )
        .await?;
        return Ok(ResponseJson(ApiResponse::success(RunNowResponse {
            routine_run: run,
            workspace_id: None,
            skipped: true,
            skip_reason: Some("prev_still_running".to_string()),
        })));
    }

    let run = RoutineRun::create(pool, run_id, id, now, RoutineRunStatus::Pending, None).await?;

    // Manual "Run now" does NOT touch next_run_at (per plan §3). Spawn directly.
    let workspace_id = spawn_routine_run(&deployment, &routine, run.id).await?;

    Ok(ResponseJson(ApiResponse::success(RunNowResponse {
        routine_run: run,
        workspace_id: Some(workspace_id),
        skipped: false,
        skip_reason: None,
    })))
}

/// Spawn a workspace for a routine run. Mirrors the inline flow in
/// `create_and_start_workspace` but for the routine code path. Returns the
/// spawned workspace_id.
pub(crate) async fn spawn_routine_run(
    deployment: &DeploymentImpl,
    routine: &Routine,
    routine_run_id: Uuid,
) -> Result<Uuid, ApiError> {
    let pool = &deployment.db().pool;
    let now = chrono::Utc::now();

    let repo = db::models::repo::Repo::find_by_id(pool, routine.repo_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Routine references missing repo".into()))?;

    let target_branch = match &routine.target_branch {
        Some(b) if !b.is_empty() => b.clone(),
        _ => deployment
            .git()
            .get_current_branch(&repo.path)
            .unwrap_or_default(),
    };

    let mut executor_config: ExecutorConfig = routine
        .executor_config_parsed()
        .map_err(|e| ApiError::BadRequest(format!("Routine has invalid executor_config: {e}")))?;

    // The picker stores "provider/model" (or just "model" if no provider was
    // selected) in executor_config.model_id. Split here so start_workspace
    // can receive selected_provider_id + selected_model_id like the workspace
    // create flow does. Null model_id passes through as "agent default".
    let (selected_provider_id, selected_model_id) = match executor_config.model_id.clone() {
        Some(raw) if !raw.is_empty() => match raw.split_once('/') {
            Some((provider, model)) if !provider.is_empty() && !model.is_empty() => {
                executor_config.model_id = Some(model.to_string());
                (Some(provider.to_string()), Some(model.to_string()))
            }
            _ => (None, Some(raw)),
        },
        _ => {
            executor_config.model_id = None;
            (None, None)
        }
    };

    // Resolve the selected provider (if any) into env + Codex injection so the
    // routine spawn matches the workspace-create path (third-party provider
    // env vars like ANTHROPIC_BASE_URL must be passed at start_workspace).
    let provider_uuid = selected_provider_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| ApiError::BadRequest("Routine has invalid provider id".into()))?;
    let injection = build_injection_for_provider(pool, provider_uuid, &mut executor_config).await?;
    let selected_model_id = executor_config.model_id.clone().or(selected_model_id);

    let workspace =
        create_workspace_record(deployment, Some(routine.name.clone()), routine.use_worktree)
            .await?;

    let mut managed = deployment
        .workspace_manager()
        .load_managed_workspace(workspace)
        .await?;
    managed
        .add_repository(
            &WorkspaceRepoInput {
                repo_id: repo.id,
                target_branch: target_branch.clone(),
            },
            deployment.git(),
        )
        .await
        .map_err(ApiError::from)?;

    if !routine.use_worktree && repo.is_git && !target_branch.is_empty() {
        let git = deployment.git();
        let current = git.get_current_branch(&repo.path).ok();
        if current.as_deref() != Some(target_branch.as_str()) {
            git.checkout_branch(&repo.path, &target_branch, false)
                .map_err(|e| {
                    ApiError::Workspace(db::models::workspace::WorkspaceError::ValidationError(
                        e.to_string(),
                    ))
                })?;
        }
    }

    if !routine.use_worktree
        && let Ok(current_branch) = deployment.git().get_current_branch(&repo.path)
    {
        Workspace::update_branch_name(pool, managed.workspace.id, &current_branch).await?;
        managed.workspace.branch = current_branch;
    }
    if !repo.is_git {
        Workspace::update_branch_name(pool, managed.workspace.id, "").await?;
        managed.workspace.branch = String::new();
    }

    Workspace::set_source(pool, managed.workspace.id, WorkspaceSource::Routine).await?;
    RoutineRun::attach_workspace(pool, routine_run_id, managed.workspace.id).await?;
    RoutineRun::mark_running(pool, routine_run_id, now).await?;

    let _process = deployment
        .container()
        .start_workspace(
            &managed.workspace,
            executor_config,
            routine.instructions.clone(),
            injection,
            selected_provider_id,
            selected_model_id,
        )
        .await?;

    Ok(managed.workspace.id)
}

pub fn router(_deployment: &DeploymentImpl) -> Router<DeploymentImpl> {
    Router::new()
        .route("/routines", get(list_routines).post(create_routine))
        .route(
            "/routines/{id}",
            get(get_routine)
                .patch(update_routine)
                .delete(delete_routine),
        )
        .route("/routines/{id}/run", post(run_now))
        .route("/routines/{id}/runs", get(list_runs))
}
