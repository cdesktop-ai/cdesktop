//! Durable metered-fallback approvals (plan §12).
//!
//! Unlike tool approvals (`routes::approvals`), these are database-backed and
//! survive service and machine restart. Responding to one resolves the
//! durable row single-winner; an approval resumes the held command exactly
//! once through the normal session command dispatcher.

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::Json as ResponseJson,
    routing::{get, post},
};
use db::models::{metered_approval::MeteredApproval, session_command::SessionCommand};
use deployment::Deployment;
use serde::Deserialize;
use services::services::container::ContainerService;
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::DeploymentImpl;

#[derive(Debug, Deserialize, TS)]
pub struct MeteredApprovalResponseRequest {
    pub approved: bool,
    #[serde(default)]
    #[ts(optional)]
    pub reason: Option<String>,
}

async fn list_pending_metered_approvals(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<MeteredApproval>>>, StatusCode> {
    let pending = MeteredApproval::list_pending(&deployment.db().pool)
        .await
        .map_err(|error| {
            tracing::error!("Failed to list metered approvals: {}", error);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(ResponseJson(ApiResponse::success(pending)))
}

async fn respond_to_metered_approval(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    ResponseJson(request): ResponseJson<MeteredApprovalResponseRequest>,
) -> Result<ResponseJson<ApiResponse<MeteredApproval>>, StatusCode> {
    let pool = &deployment.db().pool;
    let approval = MeteredApproval::find_by_id(pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let resolved = MeteredApproval::respond(pool, id, request.approved, request.reason.as_deref())
        .await
        .map_err(|error| {
            tracing::error!("Failed to respond to metered approval {}: {}", id, error);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if !resolved {
        // Already resolved earlier — the first decision stands.
        return Err(StatusCode::CONFLICT);
    }

    // Approval resumes the held command exactly once through the normal
    // dispatcher; its native single-winner claim prevents duplicate dispatch.
    if request.approved
        && let Ok(Some(command)) =
            SessionCommand::find_by_id(pool, approval.session_command_id).await
        && let Err(error) = deployment
            .container()
            .dispatch_pending_commands(command.session_id)
            .await
    {
        tracing::warn!(
            "Approved metered command {} did not dispatch immediately: {}",
            approval.session_command_id,
            error
        );
    }

    let updated = MeteredApproval::find_by_id(pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(ResponseJson(ApiResponse::success(updated)))
}

pub(super) fn router() -> Router<DeploymentImpl> {
    Router::new()
        .route("/metered-approvals", get(list_pending_metered_approvals))
        .route(
            "/metered-approvals/{id}/respond",
            post(respond_to_metered_approval),
        )
}
