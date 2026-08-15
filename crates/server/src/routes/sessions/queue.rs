use axum::{
    Extension, Json, Router, extract::State, middleware::from_fn_with_state,
    response::Json as ResponseJson, routing::get,
};
use db::models::{
    scratch::{DraftFollowUpData, Scratch, ScratchType},
    session::Session,
    session_command::{
        NewSessionCommand, SessionCommand, SessionCommandConfig, SessionCommandIntent,
    },
};
use deployment::Deployment;
use executors::profile::ExecutorConfig;
use serde::Deserialize;
use services::services::{container::ContainerService, queued_message::QueueStatus};
use ts_rs::TS;
use utils::response::ApiResponse;

use crate::{DeploymentImpl, error::ApiError, middleware::load_session_middleware};

/// Request body for queueing a follow-up message
#[derive(Debug, Deserialize, TS)]
struct QueueMessageRequest {
    pub message: String,
    pub executor_config: ExecutorConfig,
}

/// Queue a follow-up message to be executed when the current execution finishes
async fn queue_message(
    Extension(session): Extension<Session>,
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<QueueMessageRequest>,
) -> Result<ResponseJson<ApiResponse<QueueStatus>>, ApiError> {
    let data = DraftFollowUpData {
        message: payload.message,
        executor_config: payload.executor_config,
    };

    let _ = SessionCommand::enqueue(
        &deployment.db().pool,
        NewSessionCommand {
            session_id: session.id,
            dedupe_key: None,
            intent: SessionCommandIntent::Continue,
            body: data.message,
            config: SessionCommandConfig {
                executor_config: data.executor_config,
                selected_provider_id: None,
            },
        },
    )
    .await?;
    deployment
        .container()
        .dispatch_pending_commands(session.id)
        .await?;
    Scratch::delete(
        &deployment.db().pool,
        session.id,
        &ScratchType::DraftFollowUp,
    )
    .await?;

    deployment
        .track_if_analytics_allowed(
            "follow_up_queued",
            serde_json::json!({
                "session_id": session.id.to_string(),
                "workspace_id": session.workspace_id.to_string(),
            }),
        )
        .await;

    Ok(ResponseJson(ApiResponse::success(
        queue_status(&deployment, session.id).await?,
    )))
}

/// Cancel a queued follow-up message
async fn cancel_queued_message(
    Extension(session): Extension<Session>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<QueueStatus>>, ApiError> {
    SessionCommand::cancel_pending(&deployment.db().pool, session.id).await?;

    deployment
        .track_if_analytics_allowed(
            "follow_up_queue_cancelled",
            serde_json::json!({
                "session_id": session.id.to_string(),
                "workspace_id": session.workspace_id.to_string(),
            }),
        )
        .await;

    Ok(ResponseJson(ApiResponse::success(QueueStatus::Empty)))
}

/// Get the current queue status for a session's workspace
async fn get_queue_status(
    Extension(session): Extension<Session>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<QueueStatus>>, ApiError> {
    let status = queue_status(&deployment, session.id).await?;

    Ok(ResponseJson(ApiResponse::success(status)))
}

async fn queue_status(
    deployment: &DeploymentImpl,
    session_id: uuid::Uuid,
) -> Result<QueueStatus, ApiError> {
    let Some(command) = SessionCommand::pending(&deployment.db().pool, session_id)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(QueueStatus::Empty);
    };
    Ok(QueueStatus::from_command(command))
}

pub(super) fn router(deployment: &DeploymentImpl) -> Router<DeploymentImpl> {
    Router::new()
        .route(
            "/",
            get(get_queue_status)
                .post(queue_message)
                .delete(cancel_queued_message),
        )
        .layer(from_fn_with_state(
            deployment.clone(),
            load_session_middleware,
        ))
}
