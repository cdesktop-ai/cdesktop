use std::{future::Future, sync::LazyLock};

use axum::{
    Json, Router,
    extract::{Path, State},
    response::Json as ResponseJson,
    routing::put,
};
use db::models::{
    managed_task_effect::{ManagedTaskEffect, ManagedTaskEffectError, NewManagedTaskEffect},
    requests::CreateAndStartWorkspaceRequest,
    session::{Session, SessionError},
    workspace::{Workspace, WorkspaceError},
};
use deployment::Deployment;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{
    DeploymentImpl,
    error::ApiError,
    routes::{
        teammates::{SpawnSource, SpawnTeammateRequest, spawn_teammate_with_id},
        workspaces::create::create_and_start_workspace_with_ids,
    },
};

static INSTANCE_ID: LazyLock<Uuid> = LazyLock::new(Uuid::new_v4);

pub fn router() -> Router<DeploymentImpl> {
    Router::new().route(
        "/managed-tasks/{task_id}/epochs/{epoch}",
        put(create_or_return).get(get_effect),
    )
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ManagedLaunch {
    Workspace {
        request: CreateAndStartWorkspaceRequest,
    },
    Session {
        workspace_id: Uuid,
        caller_session_id: Uuid,
        request: SpawnTeammateRequest,
    },
}

impl ManagedLaunch {
    fn kind(&self) -> &'static str {
        match self {
            Self::Workspace { .. } => "workspace",
            Self::Session { .. } => "session",
        }
    }

    fn workspace_id(&self) -> Uuid {
        match self {
            Self::Workspace { .. } => Uuid::new_v4(),
            Self::Session { workspace_id, .. } => *workspace_id,
        }
    }
}

#[derive(Debug, Serialize)]
struct ManagedTaskEffectResponse {
    state: String,
    workspace_id: Option<Uuid>,
    session_id: Option<Uuid>,
    reason: Option<String>,
    created: bool,
}

struct ReservedManagedLaunch {
    record: ManagedTaskEffect,
    inserted: bool,
    launch_result: Option<Result<(), ApiError>>,
}

/// The reservation is committed before the native launch closure runs. A
/// duplicate `(task_id, epoch)` therefore returns the IDs of the first native
/// effect without invoking the launcher again.
async fn reserve_and_launch<F, Fut>(
    pool: &sqlx::SqlitePool,
    effect: NewManagedTaskEffect<'_>,
    launch: F,
) -> Result<ReservedManagedLaunch, ApiError>
where
    F: FnOnce(Uuid, Uuid) -> Fut,
    Fut: Future<Output = Result<(), ApiError>>,
{
    let (record, inserted) = ManagedTaskEffect::begin(pool, effect)
        .await
        .map_err(map_effect_error)?;
    let launch_result = inserted.then(|| launch(record.workspace_id, record.session_id));
    let launch_result = match launch_result {
        Some(launch) => Some(launch.await),
        None => None,
    };
    Ok(ReservedManagedLaunch {
        record,
        inserted,
        launch_result,
    })
}

async fn create_or_return(
    State(deployment): State<DeploymentImpl>,
    Path((task_id, epoch)): Path<(Uuid, i64)>,
    Json(launch): Json<ManagedLaunch>,
) -> Result<ResponseJson<ApiResponse<ManagedTaskEffectResponse>>, ApiError> {
    if epoch < 1 {
        return Err(ApiError::BadRequest("Task epoch must be positive".into()));
    }
    let request_hash = request_hash(&launch)?;
    let kind = launch.kind();
    let workspace_id = launch.workspace_id();
    let session_id = Uuid::new_v4();
    let deployment_for_launch = deployment.clone();
    let reserved = reserve_and_launch(
        &deployment.db().pool,
        NewManagedTaskEffect {
            task_id,
            epoch,
            request_hash: &request_hash,
            kind,
            workspace_id,
            session_id,
            owner_instance_id: *INSTANCE_ID,
        },
        |workspace_id, session_id| async move {
            match launch {
                ManagedLaunch::Workspace { request } => {
                    create_and_start_workspace_with_ids(
                        &deployment_for_launch,
                        request,
                        workspace_id,
                        session_id,
                    )
                    .await?;
                }
                ManagedLaunch::Session {
                    workspace_id: requested_workspace_id,
                    caller_session_id,
                    request,
                } => {
                    let workspace = Workspace::find_by_id(
                        &deployment_for_launch.db().pool,
                        requested_workspace_id,
                    )
                    .await?
                    .ok_or(WorkspaceError::WorkspaceNotFound)?;
                    let caller =
                        Session::find_by_id(&deployment_for_launch.db().pool, caller_session_id)
                            .await?
                            .ok_or(SessionError::NotFound)?;
                    if caller.workspace_id != workspace.id || workspace.id != workspace_id {
                        return Err(ApiError::Conflict(
                            "Caller session belongs to another workspace".into(),
                        ));
                    }
                    spawn_teammate_with_id(
                        &deployment_for_launch,
                        &workspace,
                        Some(&caller),
                        request,
                        SpawnSource::SessionCli,
                        session_id,
                    )
                    .await?;
                }
            }
            Ok(())
        },
    )
    .await?;
    let record = reserved.record;
    let inserted = reserved.inserted;

    if !inserted {
        let record = reconcile(&deployment, record).await?;
        if record.state == "pending" && record.owner_instance_id == *INSTANCE_ID {
            return Err(ApiError::TooEarly(
                "Managed task launch is still running".into(),
            ));
        }
        return Ok(ResponseJson(ApiResponse::success(to_response(
            record, false,
        ))));
    }

    let launch_result = reserved
        .launch_result
        .expect("the inserting request always invokes the native launcher");

    let record = match launch_result {
        Ok(()) => {
            ManagedTaskEffect::finish(&deployment.db().pool, task_id, epoch, "active", true, None)
                .await?
        }
        Err(error) => {
            tracing::error!(%task_id, epoch, "managed task launch failed: {error}");
            let current = ManagedTaskEffect::find(&deployment.db().pool, task_id, epoch)
                .await?
                .ok_or_else(|| ApiError::BadRequest("Managed task effect not found".into()))?;
            let effect_created = native_effect_exists(&deployment, &current).await?;
            ManagedTaskEffect::finish(
                &deployment.db().pool,
                task_id,
                epoch,
                if effect_created { "active" } else { "lost" },
                effect_created,
                (!effect_created).then_some("native_launch_failed"),
            )
            .await?
        }
    };

    Ok(ResponseJson(ApiResponse::success(to_response(
        record, true,
    ))))
}

async fn get_effect(
    State(deployment): State<DeploymentImpl>,
    Path((task_id, epoch)): Path<(Uuid, i64)>,
) -> Result<ResponseJson<ApiResponse<ManagedTaskEffectResponse>>, ApiError> {
    let record = ManagedTaskEffect::find(&deployment.db().pool, task_id, epoch)
        .await?
        .ok_or_else(|| ApiError::BadRequest("Managed task effect not found".into()))?;
    let record = reconcile(&deployment, record).await?;
    Ok(ResponseJson(ApiResponse::success(to_response(
        record, false,
    ))))
}

async fn reconcile(
    deployment: &DeploymentImpl,
    record: ManagedTaskEffect,
) -> Result<ManagedTaskEffect, ApiError> {
    if record.state != "pending" || record.owner_instance_id == *INSTANCE_ID {
        return Ok(record);
    }
    let effect_created = native_effect_exists(deployment, &record).await?;
    ManagedTaskEffect::finish(
        &deployment.db().pool,
        record.task_id,
        record.epoch,
        if effect_created { "active" } else { "lost" },
        effect_created,
        (!effect_created).then_some("owner_restarted_before_effect_was_observed"),
    )
    .await
    .map_err(ApiError::from)
}

async fn native_effect_exists(
    deployment: &DeploymentImpl,
    record: &ManagedTaskEffect,
) -> Result<bool, ApiError> {
    let session = Session::find_by_id(&deployment.db().pool, record.session_id).await?;
    Ok(session.is_some_and(|session| session.workspace_id == record.workspace_id))
}

fn request_hash(launch: &ManagedLaunch) -> Result<String, ApiError> {
    let bytes =
        serde_json::to_vec(launch).map_err(|error| ApiError::BadRequest(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn map_effect_error(error: ManagedTaskEffectError) -> ApiError {
    match error {
        ManagedTaskEffectError::Database(error) => ApiError::Database(error),
        ManagedTaskEffectError::Conflict => ApiError::Conflict(
            "The task epoch already belongs to different launch parameters".into(),
        ),
        ManagedTaskEffectError::StaleEpoch => ApiError::Conflict("The task epoch is stale".into()),
    }
}

fn to_response(record: ManagedTaskEffect, created_by_request: bool) -> ManagedTaskEffectResponse {
    let visible = record.effect_created;
    ManagedTaskEffectResponse {
        state: record.state,
        workspace_id: visible.then_some(record.workspace_id),
        session_id: visible.then_some(record.session_id),
        reason: record.reason,
        created: created_by_request && visible,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    async fn pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        pool
    }

    fn effect(task_id: Uuid, session_id: Uuid) -> NewManagedTaskEffect<'static> {
        NewManagedTaskEffect {
            task_id,
            epoch: 1,
            request_hash: "same-request",
            kind: "session",
            workspace_id: Uuid::new_v4(),
            session_id,
            owner_instance_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn request_hash_is_stable_and_parameter_sensitive() {
        let request = SpawnTeammateRequest {
            name: "reviewer".into(),
            prompt: Some("audit".into()),
            executor_config: None,
            selected_provider_id: None,
        };
        let first = ManagedLaunch::Session {
            workspace_id: Uuid::nil(),
            caller_session_id: Uuid::nil(),
            request,
        };
        assert_eq!(request_hash(&first).unwrap(), request_hash(&first).unwrap());

        let second = ManagedLaunch::Session {
            workspace_id: Uuid::nil(),
            caller_session_id: Uuid::nil(),
            request: SpawnTeammateRequest {
                name: "reviewer".into(),
                prompt: Some("different".into()),
                executor_config: None,
                selected_provider_id: None,
            },
        };
        assert_ne!(
            request_hash(&first).unwrap(),
            request_hash(&second).unwrap()
        );
    }

    #[tokio::test]
    async fn route_launch_seam_retries_without_creating_a_second_native_session() {
        let pool = pool().await;
        let task_id = Uuid::new_v4();
        let launches = Arc::new(AtomicUsize::new(0));

        let first = reserve_and_launch(&pool, effect(task_id, Uuid::new_v4()), {
            let launches = launches.clone();
            move |_, _| async move {
                launches.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .await
        .unwrap();
        assert!(first.inserted);
        let first_session_id = first.record.session_id;
        ManagedTaskEffect::finish(&pool, task_id, 1, "active", true, None)
            .await
            .unwrap();

        let retry = reserve_and_launch(&pool, effect(task_id, Uuid::new_v4()), {
            let launches = launches.clone();
            move |_, _| async move {
                launches.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .await
        .unwrap();

        assert!(!retry.inserted);
        assert!(retry.launch_result.is_none());
        assert_eq!(retry.record.session_id, first_session_id);
        assert_eq!(launches.load(Ordering::SeqCst), 1);
    }
}
