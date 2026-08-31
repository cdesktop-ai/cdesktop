use std::sync::LazyLock;

use anyhow;
use axum::{
    Extension, Router,
    extract::{Path, Query, State, ws::Message},
    middleware::from_fn_with_state,
    response::{IntoResponse, Json as ResponseJson},
    routing::{get, post},
};
use db::models::{
    execution_process::{ExecutionProcess, ExecutionProcessStatus},
    execution_process_repo_state::ExecutionProcessRepoState,
    execution_process_stop_operation::{
        StopExecutionOperation, StopExecutionOperationState, StopExecutionOutcome,
    },
};
use deployment::Deployment;
use futures_util::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use services::services::container::ContainerService;
use utils::{log_msg::LogMsg, response::ApiResponse};
use uuid::Uuid;

use crate::{
    DeploymentImpl,
    error::ApiError,
    middleware::{
        load_execution_process_middleware,
        signed_ws::{MaybeSignedWebSocket, SignedWsUpgrade},
    },
};

/// Identifies the server process that owns a pending keyed stop. A different
/// value after restart can reconcile, but never re-execute, an orphaned stop.
static STOP_OPERATION_INSTANCE_ID: LazyLock<Uuid> = LazyLock::new(Uuid::new_v4);

#[derive(Debug, Deserialize)]
struct SessionExecutionProcessQuery {
    pub session_id: Uuid,
    /// If true, include soft-deleted (dropped) processes in results/stream
    #[serde(default)]
    pub show_soft_deleted: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct StopExecutionProcessRequest {
    /// Caller-owned, deterministic key used to replay a lost stop response.
    #[serde(default)]
    dedupe_key: Option<String>,
}

#[derive(Debug, Serialize)]
struct NormalizedLogSnapshot {
    entries: Vec<serde_json::Value>,
    patch_count: usize,
    skipped_patch_count: usize,
    complete: bool,
}

fn apply_normalized_message(
    document: &mut serde_json::Value,
    message: LogMsg,
    patch_count: &mut usize,
    skipped_patch_count: &mut usize,
) -> bool {
    match message {
        LogMsg::JsonPatch(patch) => {
            if json_patch::patch(document, &patch).is_ok() {
                *patch_count += 1;
            } else {
                *skipped_patch_count += 1;
            }
            false
        }
        LogMsg::Finished => true,
        _ => false,
    }
}

async fn get_execution_process_by_id(
    Extension(execution_process): Extension<ExecutionProcess>,
    State(_deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<ExecutionProcess>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(execution_process)))
}

async fn list_execution_processes_by_session(
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<SessionExecutionProcessQuery>,
) -> Result<ResponseJson<ApiResponse<Vec<ExecutionProcess>>>, ApiError> {
    let processes = ExecutionProcess::find_by_session_id(
        &deployment.db().pool,
        query.session_id,
        query.show_soft_deleted.unwrap_or(false),
    )
    .await?;
    Ok(ResponseJson(ApiResponse::success(processes)))
}

async fn get_normalized_log_snapshot(
    Extension(execution_process): Extension<ExecutionProcess>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<NormalizedLogSnapshot>>, ApiError> {
    let Some(mut stream) = deployment
        .container()
        .stream_normalized_logs(&execution_process.id)
        .await
    else {
        return Err(ApiError::BadRequest(
            "normalized logs are unavailable for this execution".into(),
        ));
    };

    let mut document = serde_json::json!({ "entries": [] });
    let mut patch_count = 0;
    let mut skipped_patch_count = 0;
    let mut complete = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(250);
    while patch_count < 100_000 {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let next = tokio::time::timeout(
            remaining.min(std::time::Duration::from_millis(50)),
            stream.next(),
        )
        .await;
        let Ok(Some(message)) = next else {
            break;
        };
        if apply_normalized_message(
            &mut document,
            message?,
            &mut patch_count,
            &mut skipped_patch_count,
        ) {
            complete = true;
            break;
        }
    }

    let entries = document
        .get_mut("entries")
        .and_then(serde_json::Value::as_array_mut)
        .map(std::mem::take)
        .unwrap_or_default();
    Ok(ResponseJson(ApiResponse::success(NormalizedLogSnapshot {
        entries,
        patch_count,
        skipped_patch_count,
        complete,
    })))
}

async fn stream_raw_logs_ws(
    ws: SignedWsUpgrade,
    State(deployment): State<DeploymentImpl>,
    Path(exec_id): Path<Uuid>,
) -> impl IntoResponse {
    // Always accept the WebSocket upgrade — handle "not found" inside the
    // connection by sending `finished` and closing cleanly, instead of
    // rejecting with HTTP 404 which the browser surfaces as an opaque
    // connection failure.
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_raw_logs_ws(socket, deployment, exec_id).await {
            tracing::warn!("raw logs WS closed: {}", e);
        }
    })
}

async fn handle_raw_logs_ws(
    mut socket: MaybeSignedWebSocket,
    deployment: DeploymentImpl,
    exec_id: Uuid,
) -> anyhow::Result<()> {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use executors::logs::utils::patch::ConversationPatch;
    use utils::log_msg::LogMsg;

    // Get the raw stream — if not found, send finished and close cleanly
    let raw_stream = match deployment.container().stream_raw_logs(&exec_id).await {
        Some(stream) => stream,
        None => {
            // No logs available: send finished so the client gets a clean
            // close instead of retrying endlessly.
            let _ = socket
                .send(LogMsg::Finished.to_ws_message_unchecked())
                .await;
            let _ = socket.close().await;
            return Ok(());
        }
    };

    let counter = Arc::new(AtomicUsize::new(0));
    let mut stream = raw_stream.map_ok({
        let counter = counter.clone();
        move |m| match m {
            LogMsg::Stdout(content) => {
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let patch = ConversationPatch::add_stdout(index, content);
                LogMsg::JsonPatch(patch).to_ws_message_unchecked()
            }
            LogMsg::Stderr(content) => {
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let patch = ConversationPatch::add_stderr(index, content);
                LogMsg::JsonPatch(patch).to_ws_message_unchecked()
            }
            LogMsg::Finished => LogMsg::Finished.to_ws_message_unchecked(),
            _ => unreachable!("Raw stream should only have Stdout/Stderr/Finished"),
        }
    });

    loop {
        tokio::select! {
            item = stream.next() => {
                match item {
                    Some(Ok(msg)) => {
                        if socket.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        tracing::error!("stream error: {}", e);
                        break;
                    }
                    None => break,
                }
            }
            inbound = socket.recv() => {
                match inbound {
                    Ok(Some(Message::Close(_))) => break,
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }
    }
    // Send a proper close frame so the client sees code 1000 (normal closure)
    // instead of an abnormal TCP drop that triggers reconnection attempts.
    let _ = socket.close().await;
    Ok(())
}

async fn stream_normalized_logs_ws(
    ws: SignedWsUpgrade,
    State(deployment): State<DeploymentImpl>,
    Path(exec_id): Path<Uuid>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        let stream = deployment
            .container()
            .stream_normalized_logs(&exec_id)
            .await;

        match stream {
            Some(stream) => {
                let stream = stream.err_into::<anyhow::Error>().into_stream();
                if let Err(e) = handle_normalized_logs_ws(socket, stream).await {
                    tracing::warn!("normalized logs WS closed: {}", e);
                }
            }
            None => {
                // No logs available: send finished and close cleanly
                let mut socket = socket;
                let _ = socket
                    .send(utils::log_msg::LogMsg::Finished.to_ws_message_unchecked())
                    .await;
                let _ = socket.close().await;
            }
        }
    })
}

async fn handle_normalized_logs_ws(
    mut socket: MaybeSignedWebSocket,
    stream: impl futures_util::Stream<Item = anyhow::Result<LogMsg>> + Unpin + Send + 'static,
) -> anyhow::Result<()> {
    let mut stream = stream.map_ok(|msg| msg.to_ws_message_unchecked());
    loop {
        tokio::select! {
            item = stream.next() => {
                match item {
                    Some(Ok(msg)) => {
                        if socket.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        tracing::error!("stream error: {}", e);
                        break;
                    }
                    None => break,
                }
            }
            inbound = socket.recv() => {
                match inbound {
                    Ok(Some(Message::Close(_))) => break,
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }
    }
    let _ = socket.close().await;
    Ok(())
}

async fn stop_execution_process(
    Extension(execution_process): Extension<ExecutionProcess>,
    State(deployment): State<DeploymentImpl>,
    payload: Option<axum::Json<StopExecutionProcessRequest>>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    let Some(dedupe_key) = payload.and_then(|axum::Json(request)| request.dedupe_key) else {
        deployment
            .container()
            .stop_execution(&execution_process, ExecutionProcessStatus::Killed)
            .await?;

        return Ok(ResponseJson(ApiResponse::success(())));
    };
    if dedupe_key.is_empty() {
        return Err(ApiError::BadRequest("dedupe_key must not be empty".into()));
    }

    let pool = &deployment.db().pool;
    let instance_id = *STOP_OPERATION_INSTANCE_ID;
    let state =
        StopExecutionOperation::begin(pool, execution_process.id, &dedupe_key, instance_id).await?;
    match state {
        StopExecutionOperationState::Complete(outcome) => return stop_outcome_response(outcome),
        StopExecutionOperationState::Owner => {}
        // Terminal execution status is only written after cancellation/kill
        // succeeds. Therefore an orphaned pending request may be accepted
        // only when that durable side-effect boundary was crossed.
        StopExecutionOperationState::Pending {
            owned_by_current_instance: true,
        } => {
            // 425 is deliberately distinct from the durable 409 rejection:
            // retry this exact key until the owner publishes its outcome.
            return Err(ApiError::TooEarly(
                "The original stop request is still in progress; retry the same dedupe_key.".into(),
            ));
        }
        StopExecutionOperationState::Pending {
            owned_by_current_instance: false,
        } => {
            let outcome = orphaned_stop_outcome();
            let outcome = StopExecutionOperation::complete(
                pool,
                execution_process.id,
                &dedupe_key,
                outcome,
                instance_id,
            )
            .await?;
            return stop_outcome_response(outcome);
        }
    }

    let outcome = match deployment
        .container()
        .stop_execution(&execution_process, ExecutionProcessStatus::Killed)
        .await
    {
        Ok(()) => StopExecutionOutcome::Accepted,
        Err(error) => {
            tracing::warn!(
                execution_process_id = %execution_process.id,
                "keyed stop request rejected: {error}"
            );
            StopExecutionOutcome::Rejected
        }
    };
    let outcome = StopExecutionOperation::complete(
        pool,
        execution_process.id,
        &dedupe_key,
        outcome,
        instance_id,
    )
    .await?;
    stop_outcome_response(outcome)
}

fn stop_outcome_response(
    outcome: StopExecutionOutcome,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    match outcome {
        StopExecutionOutcome::Accepted => Ok(ResponseJson(ApiResponse::success(()))),
        StopExecutionOutcome::Rejected => Err(ApiError::Conflict(
            "The original stop request was rejected.".into(),
        )),
        StopExecutionOutcome::Interrupted => Err(ApiError::StopInterrupted(
            "The original stop owner ended before its result was durably known; reconcile this key without issuing another stop."
                .into(),
        )),
    }
}

fn orphaned_stop_outcome() -> StopExecutionOutcome {
    // A terminal execution row can come from the independent exit monitor,
    // not this stop operation. Without a durable process-controller identity,
    // it cannot prove this key performed the side effect. Preserve safety by
    // recording a distinct terminal interruption rather than inferring either
    // acceptance or rejection.
    StopExecutionOutcome::Interrupted
}

async fn stream_execution_processes_by_session_ws(
    ws: SignedWsUpgrade,
    State(deployment): State<DeploymentImpl>,
    Query(query): Query<SessionExecutionProcessQuery>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = handle_execution_processes_by_session_ws(
            socket,
            deployment,
            query.session_id,
            query.show_soft_deleted.unwrap_or(false),
        )
        .await
        {
            tracing::warn!("execution processes by session WS closed: {}", e);
        }
    })
}

async fn handle_execution_processes_by_session_ws(
    mut socket: MaybeSignedWebSocket,
    deployment: DeploymentImpl,
    session_id: uuid::Uuid,
    show_soft_deleted: bool,
) -> anyhow::Result<()> {
    // Get the raw stream and convert LogMsg to WebSocket messages
    let mut stream = deployment
        .events()
        .stream_execution_processes_for_session_raw(session_id, show_soft_deleted)
        .await?
        .map_ok(|msg| msg.to_ws_message_unchecked());

    loop {
        tokio::select! {
            item = stream.next() => {
                match item {
                    Some(Ok(msg)) => {
                        if socket.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        tracing::error!("stream error: {}", e);
                        break;
                    }
                    None => break,
                }
            }
            inbound = socket.recv() => {
                match inbound {
                    Ok(Some(Message::Close(_))) => break,
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }
    }
    Ok(())
}

async fn get_execution_process_repo_states(
    Extension(execution_process): Extension<ExecutionProcess>,
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<ExecutionProcessRepoState>>>, ApiError> {
    let pool = &deployment.db().pool;
    let repo_states =
        ExecutionProcessRepoState::find_by_execution_process_id(pool, execution_process.id).await?;
    Ok(ResponseJson(ApiResponse::success(repo_states)))
}

pub(super) fn router(deployment: &DeploymentImpl) -> Router<DeploymentImpl> {
    let workspace_id_router = Router::new()
        .route("/", get(get_execution_process_by_id))
        .route("/stop", post(stop_execution_process))
        .route("/repo-states", get(get_execution_process_repo_states))
        .route("/normalized-snapshot", get(get_normalized_log_snapshot))
        .route("/raw-logs/ws", get(stream_raw_logs_ws))
        .route("/normalized-logs/ws", get(stream_normalized_logs_ws))
        .layer(from_fn_with_state(
            deployment.clone(),
            load_execution_process_middleware,
        ));

    let workspaces_router = Router::new()
        .route("/", get(list_execution_processes_by_session))
        .route(
            "/stream/session/ws",
            get(stream_execution_processes_by_session_ws),
        )
        .nest("/{id}", workspace_id_router);

    Router::new().nest("/execution-processes", workspaces_router)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_snapshot_coalesces_streaming_replacements() {
        let mut document = serde_json::json!({ "entries": [] });
        let mut applied = 0;
        let mut skipped = 0;
        for patch in [
            serde_json::json!([{
                "op": "add",
                "path": "/entries/0",
                "value": { "content": "hel" }
            }]),
            serde_json::json!([{
                "op": "replace",
                "path": "/entries/0",
                "value": { "content": "hello" }
            }]),
        ] {
            let message =
                LogMsg::JsonPatch(serde_json::from_value(patch).expect("valid JSON patch fixture"));
            assert!(!apply_normalized_message(
                &mut document,
                message,
                &mut applied,
                &mut skipped,
            ));
        }

        assert_eq!(applied, 2);
        assert_eq!(skipped, 0);
        assert_eq!(document["entries"][0]["content"], "hello");
        assert!(apply_normalized_message(
            &mut document,
            LogMsg::Finished,
            &mut applied,
            &mut skipped,
        ));
    }

    #[test]
    fn orphaned_intent_never_infers_acceptance_from_natural_exit_status() {
        for _natural_status in [
            ExecutionProcessStatus::Running,
            ExecutionProcessStatus::Completed,
            ExecutionProcessStatus::Failed,
        ] {
            assert_eq!(orphaned_stop_outcome(), StopExecutionOutcome::Interrupted);
        }
    }

    #[test]
    fn keyed_stop_outcomes_keep_rejection_and_interruption_distinct() {
        assert!(matches!(
            stop_outcome_response(StopExecutionOutcome::Rejected),
            Err(ApiError::Conflict(_))
        ));
        assert!(matches!(
            stop_outcome_response(StopExecutionOutcome::Interrupted),
            Err(ApiError::StopInterrupted(_))
        ));
        assert!(stop_outcome_response(StopExecutionOutcome::Accepted).is_ok());
    }

    #[test]
    fn omitted_dedupe_key_preserves_the_legacy_stop_request() {
        let request: StopExecutionProcessRequest =
            serde_json::from_value(serde_json::json!({})).expect("empty stop request is valid");
        assert!(request.dedupe_key.is_none());
    }
}
