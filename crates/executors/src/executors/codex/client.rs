use std::{
    collections::{HashMap, VecDeque},
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex as StdMutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use codex_app_server_protocol::{
    ClientInfo, ClientNotification, ClientRequest, CommandExecutionApprovalDecision,
    CommandExecutionRequestApprovalParams, CommandExecutionRequestApprovalResponse,
    ConfigBatchWriteParams, ConfigEdit, ConfigReadParams, ConfigReadResponse, ConfigWriteResponse,
    DynamicToolCallOutputContentItem, DynamicToolCallResponse, FileChangeApprovalDecision,
    FileChangeRequestApprovalParams, FileChangeRequestApprovalResponse, GetAccountParams,
    GetAccountRateLimitsResponse, GetAccountResponse, InitializeCapabilities, InitializeParams,
    InitializeResponse, ItemCompletedNotification, JSONRPCError, JSONRPCNotification,
    JSONRPCRequest, JSONRPCResponse, ListMcpServerStatusParams, ListMcpServerStatusResponse,
    McpServerStatusDetail, RequestId, ReviewStartParams, ReviewStartResponse, ReviewTarget,
    ServerRequest, ThreadCompactStartParams, ThreadCompactStartResponse, ThreadForkParams,
    ThreadForkResponse, ThreadItem, ThreadReadParams, ThreadReadResponse, ThreadStartParams,
    ThreadStartResponse, ToolRequestUserInputAnswer, ToolRequestUserInputQuestion,
    ToolRequestUserInputResponse, TurnCompletedNotification, TurnStartParams, TurnStartResponse,
    TurnStatus, UserInput,
};
use codex_protocol::config_types::{CollaborationMode, ModeKind, Settings};
use futures::TryFutureExt;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{self, Value};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt, BufWriter},
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;
use workspace_utils::approvals::{ApprovalPatterns, ApprovalScope, ApprovalStatus, QuestionStatus};

use super::{
    jsonrpc::{JsonRpcCallbacks, JsonRpcPeer},
    storage_guard::{StorageLimits, find_rollout_file},
};
use crate::{
    approvals::{ExecutorApprovalError, ExecutorApprovalService},
    env::RepoContext,
    executors::{ExecutorError, ExecutorExitResult, codex::normalize_logs::Approval},
    outcome::{ExecutionOutcomeClass, NormalizedExecutionOutcome},
};

struct PendingPlan {
    item_id: String,
}

pub struct AppServerClient {
    rpc: OnceLock<JsonRpcPeer>,
    log_writer: LogWriter,
    approvals: Option<Arc<dyn ExecutorApprovalService>>,
    thread_id: Mutex<Option<String>>,
    rollout_path: Mutex<Option<PathBuf>>,
    storage_limits: StorageLimits,
    pending_feedback: Mutex<VecDeque<String>>,
    auto_approve: bool,
    plan_mode: bool,
    resolved_model: OnceLock<String>,
    pending_plan: Mutex<Option<PendingPlan>>,
    repo_context: RepoContext,
    commit_reminder: bool,
    commit_reminder_prompt: String,
    commit_reminder_sent: AtomicBool,
    cancel: CancellationToken,
    /// Normalized failure observed in the notification stream (e.g. a failed
    /// turn); reported through `final_exit_result` when the stream ends.
    exit_outcome: StdMutex<Option<NormalizedExecutionOutcome>>,
}

impl AppServerClient {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        log_writer: LogWriter,
        approvals: Option<Arc<dyn ExecutorApprovalService>>,
        auto_approve: bool,
        plan_mode: bool,
        repo_context: RepoContext,
        commit_reminder: bool,
        commit_reminder_prompt: String,
        cancel: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            rpc: OnceLock::new(),
            log_writer,
            approvals,
            auto_approve,
            plan_mode,
            resolved_model: OnceLock::new(),
            pending_plan: Mutex::new(None),
            thread_id: Mutex::new(None),
            rollout_path: Mutex::new(None),
            storage_limits: StorageLimits::default(),
            pending_feedback: Mutex::new(VecDeque::new()),
            repo_context,
            commit_reminder,
            commit_reminder_prompt,
            commit_reminder_sent: AtomicBool::new(false),
            cancel,
            exit_outcome: StdMutex::new(None),
        })
    }

    pub fn connect(&self, peer: JsonRpcPeer) {
        let _ = self.rpc.set(peer);
    }

    pub fn set_resolved_model(&self, model: String) {
        let _ = self.resolved_model.set(model);
    }

    fn rpc(&self) -> &JsonRpcPeer {
        self.rpc.get().expect("Codex RPC peer not attached")
    }

    pub fn log_writer(&self) -> &LogWriter {
        &self.log_writer
    }

    pub async fn initialize(&self) -> Result<(), ExecutorError> {
        let request = ClientRequest::Initialize {
            request_id: self.next_request_id(),
            params: InitializeParams {
                client_info: ClientInfo {
                    name: "vibe-codex-executor".to_string(),
                    title: None,
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
                capabilities: Some(InitializeCapabilities {
                    experimental_api: true,
                    ..Default::default()
                }),
            },
        };

        self.send_request::<InitializeResponse>(request, "initialize")
            .await?;
        self.send_message(&ClientNotification::Initialized).await
    }

    pub async fn thread_start(
        &self,
        params: ThreadStartParams,
    ) -> Result<ThreadStartResponse, ExecutorError> {
        self.storage_limits
            .ensure_start_allowed()
            .await
            .map_err(ExecutorError::Io)?;
        let request = ClientRequest::ThreadStart {
            request_id: self.next_request_id(),
            params,
        };
        self.send_request(request, "thread/start").await
    }

    pub async fn thread_fork(
        &self,
        params: ThreadForkParams,
    ) -> Result<ThreadForkResponse, ExecutorError> {
        self.storage_limits
            .ensure_fork_allowed(&params.thread_id)
            .await
            .map_err(ExecutorError::Io)?;
        let request = ClientRequest::ThreadFork {
            request_id: self.next_request_id(),
            params,
        };
        self.send_request(request, "thread/fork").await
    }

    pub async fn turn_start_with_mode(
        &self,
        thread_id: String,
        input: Vec<UserInput>,
        collaboration_mode: Option<CollaborationMode>,
    ) -> Result<TurnStartResponse, ExecutorError> {
        let request = ClientRequest::TurnStart {
            request_id: self.next_request_id(),
            params: TurnStartParams {
                thread_id,
                input,
                collaboration_mode,
                ..Default::default()
            },
        };
        self.send_request(request, "turn/start").await
    }

    fn collaboration_mode(&self, mode: ModeKind) -> Result<CollaborationMode, ExecutorError> {
        let model = self.resolved_model.get().cloned().ok_or_else(|| {
            tracing::error!("collaboration_mode called before resolved_model was set");
            ExecutorError::Io(io::Error::other(
                "resolved model not available for collaboration mode",
            ))
        })?;
        Ok(CollaborationMode {
            mode,
            settings: Settings {
                model,
                reasoning_effort: None,
                developer_instructions: None,
            },
        })
    }

    pub fn initial_collaboration_mode(&self) -> Result<CollaborationMode, ExecutorError> {
        if self.plan_mode {
            self.collaboration_mode(ModeKind::Plan)
        } else {
            self.collaboration_mode(ModeKind::Default)
        }
    }

    pub async fn get_account(&self) -> Result<GetAccountResponse, ExecutorError> {
        let request = ClientRequest::GetAccount {
            request_id: self.next_request_id(),
            params: GetAccountParams {
                refresh_token: false,
            },
        };
        self.send_request(request, "account/read").await
    }

    pub async fn start_review(
        &self,
        thread_id: String,
        target: ReviewTarget,
    ) -> Result<ReviewStartResponse, ExecutorError> {
        let request = ClientRequest::ReviewStart {
            request_id: self.next_request_id(),
            params: ReviewStartParams {
                thread_id,
                target,
                delivery: None,
            },
        };
        self.send_request(request, "reviewStart").await
    }

    pub async fn list_mcp_server_status(
        &self,
        cursor: Option<String>,
    ) -> Result<ListMcpServerStatusResponse, ExecutorError> {
        let request = ClientRequest::McpServerStatusList {
            request_id: self.next_request_id(),
            params: ListMcpServerStatusParams {
                cursor,
                limit: None,
                detail: Some(McpServerStatusDetail::ToolsAndAuthOnly),
                thread_id: None,
            },
        };
        self.send_request(request, "mcpServerStatus/list").await
    }

    pub async fn thread_compact_start(
        &self,
        thread_id: String,
    ) -> Result<ThreadCompactStartResponse, ExecutorError> {
        let request = ClientRequest::ThreadCompactStart {
            request_id: self.next_request_id(),
            params: ThreadCompactStartParams { thread_id },
        };
        self.send_request(request, "thread/compact/start").await
    }

    pub async fn thread_read(
        &self,
        thread_id: String,
    ) -> Result<ThreadReadResponse, ExecutorError> {
        let request = ClientRequest::ThreadRead {
            request_id: self.next_request_id(),
            params: ThreadReadParams {
                thread_id,
                include_turns: false,
            },
        };
        self.send_request(request, "thread/read").await
    }

    pub async fn config_batch_write(
        &self,
        edits: Vec<ConfigEdit>,
    ) -> Result<ConfigWriteResponse, ExecutorError> {
        let request = ClientRequest::ConfigBatchWrite {
            request_id: self.next_request_id(),
            params: ConfigBatchWriteParams {
                edits,
                file_path: None,
                expected_version: None,
                reload_user_config: false,
            },
        };
        self.send_request(request, "config/batchWrite").await
    }

    pub async fn config_read(
        &self,
        cwd: Option<String>,
    ) -> Result<ConfigReadResponse, ExecutorError> {
        let request = ClientRequest::ConfigRead {
            request_id: self.next_request_id(),
            params: ConfigReadParams {
                include_layers: false,
                cwd,
            },
        };
        self.send_request(request, "config/read").await
    }

    pub async fn get_account_rate_limits(
        &self,
    ) -> Result<GetAccountRateLimitsResponse, ExecutorError> {
        let request = ClientRequest::GetAccountRateLimits {
            request_id: self.next_request_id(),
            params: None,
        };
        self.send_request(request, "account/rateLimits/read").await
    }

    async fn handle_server_request(
        &self,
        peer: &JsonRpcPeer,
        request: ServerRequest,
    ) -> Result<(), ExecutorError> {
        match request {
            ServerRequest::FileChangeRequestApproval { request_id, params } => {
                let call_id = params.item_id.clone();
                let status = self
                    .request_tool_approval(
                        "edit",
                        "codex.apply_patch",
                        &call_id,
                        file_change_patterns(&params),
                    )
                    .await
                    .inspect_err(|err| {
                        if !matches!(
                            err,
                            ExecutorError::ExecutorApprovalError(ExecutorApprovalError::Cancelled)
                        ) {
                            tracing::error!(
                                "Codex file_change approval failed for item_id={}: {err}",
                                call_id
                            );
                        }
                    })?;
                self.log_writer
                    .log_raw(
                        &Approval::approval_response(
                            call_id,
                            "codex.apply_patch".to_string(),
                            status.clone(),
                        )
                        .raw(),
                    )
                    .await?;
                let (decision, feedback) = self.file_change_decision(&status);
                let response = FileChangeRequestApprovalResponse { decision };
                send_server_response(peer, request_id, response).await?;
                if let Some(message) = feedback {
                    tracing::debug!("queueing file change denial feedback: {message}");
                    self.enqueue_feedback(message).await;
                }
                Ok(())
            }
            ServerRequest::CommandExecutionRequestApproval { request_id, params } => {
                let call_id = params.item_id.clone();
                let status = self
                    .request_tool_approval(
                        "bash",
                        "codex.exec_command",
                        &call_id,
                        command_patterns(&params),
                    )
                    .await
                    .inspect_err(|err| {
                        if !matches!(
                            err,
                            ExecutorError::ExecutorApprovalError(ExecutorApprovalError::Cancelled)
                        ) {
                            tracing::error!(
                                "Codex command_execution approval failed for item_id={}: {err}",
                                call_id
                            );
                        }
                    })?;
                self.log_writer
                    .log_raw(
                        &Approval::approval_response(
                            call_id,
                            "codex.exec_command".to_string(),
                            status.clone(),
                        )
                        .raw(),
                    )
                    .await?;
                let (decision, feedback) = self.command_execution_decision(&status);
                let response = CommandExecutionRequestApprovalResponse { decision };
                send_server_response(peer, request_id, response).await?;
                if let Some(message) = feedback {
                    tracing::debug!("queueing exec denial feedback: {message}");
                    self.enqueue_feedback(message).await;
                }
                Ok(())
            }
            ServerRequest::ToolRequestUserInput { request_id, params } => {
                let call_id = params.item_id.clone();
                let question_count = params.questions.len();
                let status = self
                    .request_question_answer(question_count, &call_id)
                    .await
                    .inspect_err(|err| {
                        if !matches!(
                            err,
                            ExecutorError::ExecutorApprovalError(ExecutorApprovalError::Cancelled)
                        ) {
                            tracing::error!(
                                "Codex question approval failed for call_id={}: {err}",
                                call_id
                            );
                        }
                    })?;
                self.log_writer
                    .log_raw(&Approval::question_response(call_id.clone(), status.clone()).raw())
                    .await?;
                let response = match &status {
                    QuestionStatus::Answered { answers } => {
                        let answers_map: HashMap<String, Vec<String>> = answers
                            .iter()
                            .map(|qa| (qa.question.clone(), qa.answer.clone()))
                            .collect();
                        answers_to_codex_format(&params.questions, &answers_map)
                    }
                    _ => ToolRequestUserInputResponse {
                        answers: HashMap::new(),
                    },
                };
                send_server_response(peer, request_id, response).await?;
                Ok(())
            }
            ServerRequest::DynamicToolCall { request_id, params } => {
                tracing::warn!(
                    "received unsupported dynamic tool call: tool={} call_id={}",
                    params.tool,
                    params.call_id
                );
                let response = DynamicToolCallResponse {
                    content_items: vec![DynamicToolCallOutputContentItem::InputText {
                        text: format!(
                            "Dynamic tool '{}' is not supported by this client.",
                            params.tool
                        ),
                    }],
                    success: false,
                };
                send_server_response(peer, request_id, response).await?;
                Ok(())
            }
            ServerRequest::AttestationGenerate { .. }
            | ServerRequest::CurrentTimeRead { .. }
            | ServerRequest::ChatgptAuthTokensRefresh { .. }
            | ServerRequest::McpServerElicitationRequest { .. }
            | ServerRequest::PermissionsRequestApproval { .. } => {
                tracing::warn!("received unhandled v2 server request: {:?}", request);
                let response = JSONRPCResponse {
                    id: request.id().clone(),
                    result: Value::Null,
                };
                peer.send(&response).await
            }
            ServerRequest::ApplyPatchApproval { .. }
            | ServerRequest::ExecCommandApproval { .. } => {
                tracing::error!(
                    "received deprecated v1 server request (session may have been started with legacy API): {:?}",
                    request
                );
                Err(ExecutorApprovalError::RequestFailed(
                    "deprecated v1 server request".to_string(),
                )
                .into())
            }
        }
    }

    async fn request_tool_approval(
        &self,
        tool_name: &str,
        display_tool_name: &str,
        tool_call_id: &str,
        patterns: ApprovalPatterns,
    ) -> Result<ApprovalStatus, ExecutorError> {
        if self.auto_approve {
            return Ok(ApprovalStatus::Approved {
                scope: ApprovalScope::Once,
            });
        }
        let approval_service = self
            .approvals
            .as_ref()
            .ok_or(ExecutorApprovalError::ServiceUnavailable)?;

        let approval_id = approval_service
            .create_tool_approval(tool_name, patterns)
            .or_else(|err| async {
                self.handle_approval_error(display_tool_name, tool_call_id)
                    .await;
                Err(err)
            })
            .await?;

        let _ = self
            .log_writer
            .log_raw(
                &Approval::approval_requested(
                    tool_call_id.to_string(),
                    display_tool_name.to_string(),
                    approval_id.clone(),
                )
                .raw(),
            )
            .await;

        approval_service
            .wait_tool_approval(&approval_id, self.cancel.clone())
            .or_else(|err| async {
                self.handle_approval_error(display_tool_name, tool_call_id)
                    .await;
                Err(err)
            })
            .await
            .map_err(ExecutorError::from)
    }

    async fn handle_approval_error(&self, display_tool_name: &str, tool_call_id: &str) {
        let _ = self
            .log_writer
            .log_raw(
                &Approval::approval_response(
                    tool_call_id.to_string(),
                    display_tool_name.to_string(),
                    ApprovalStatus::TimedOut,
                )
                .raw(),
            )
            .await;
    }

    async fn request_question_answer(
        &self,
        question_count: usize,
        tool_call_id: &str,
    ) -> Result<QuestionStatus, ExecutorError> {
        let approval_service = self
            .approvals
            .as_ref()
            .ok_or(ExecutorApprovalError::ServiceUnavailable)?;

        let approval_id = approval_service
            .create_question_approval("question", question_count)
            .or_else(|err| async {
                self.handle_question_error(tool_call_id).await;
                Err(err)
            })
            .await?;

        let _ = self
            .log_writer
            .log_raw(
                &Approval::approval_requested(
                    tool_call_id.to_string(),
                    "codex.question".to_string(),
                    approval_id.clone(),
                )
                .raw(),
            )
            .await;

        approval_service
            .wait_question_answer(&approval_id, self.cancel.clone())
            .or_else(|err| async {
                self.handle_question_error(tool_call_id).await;
                Err(err)
            })
            .await
            .map_err(ExecutorError::from)
    }

    async fn handle_question_error(&self, tool_call_id: &str) {
        let _ = self
            .log_writer
            .log_raw(
                &Approval::question_response(tool_call_id.to_string(), QuestionStatus::TimedOut)
                    .raw(),
            )
            .await;
    }

    async fn handle_plan_completed(&self, plan: PendingPlan) -> Result<bool, ExecutorError> {
        let approval_service = self
            .approvals
            .as_ref()
            .ok_or(ExecutorApprovalError::ServiceUnavailable)?;

        let approval_id = approval_service
            .create_tool_approval("plan", ApprovalPatterns::default())
            .or_else(|err| async {
                self.handle_approval_error("codex.plan", &plan.item_id)
                    .await;
                Err(err)
            })
            .await?;

        let _ = self
            .log_writer
            .log_raw(
                &Approval::approval_requested(
                    plan.item_id.clone(),
                    "codex.plan".to_string(),
                    approval_id.clone(),
                )
                .raw(),
            )
            .await;

        let status = approval_service
            .wait_tool_approval(&approval_id, self.cancel.clone())
            .or_else(|err| async {
                self.handle_approval_error("codex.plan", &plan.item_id)
                    .await;
                Err(err)
            })
            .await
            .map_err(ExecutorError::from)?;

        self.log_writer
            .log_raw(
                &Approval::approval_response(
                    plan.item_id,
                    "codex.plan".to_string(),
                    status.clone(),
                )
                .raw(),
            )
            .await?;

        let Some(thread_id) = self.thread_id.lock().await.clone() else {
            return Ok(true);
        };

        match status {
            ApprovalStatus::Approved { .. } => {
                self.spawn_turn_start(
                    thread_id,
                    "Implement the plan.".to_string(),
                    Some(self.collaboration_mode(ModeKind::Default)?),
                );
                Ok(false)
            }
            ApprovalStatus::Denied { reason } => {
                let feedback = reason
                    .as_ref()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                if let Some(feedback_text) = feedback {
                    self.spawn_turn_start(
                        thread_id,
                        format!("User feedback on the plan: {feedback_text}"),
                        Some(self.collaboration_mode(ModeKind::Plan)?),
                    );
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
            ApprovalStatus::TimedOut | ApprovalStatus::Pending => Ok(true),
        }
    }

    pub async fn register_session(&self, thread_id: &str) -> Result<(), ExecutorError> {
        {
            let mut guard = self.thread_id.lock().await;
            guard.replace(thread_id.to_string());
        }
        let mut rollout = self.rollout_path.lock().await;
        *rollout = find_rollout_file(thread_id).await;
        drop(rollout);
        self.flush_pending_feedback().await;
        Ok(())
    }

    async fn enforce_storage_limits(&self) -> Result<(), ExecutorError> {
        let thread_id = self.thread_id.lock().await.clone();
        let Some(thread_id) = thread_id else {
            return self
                .storage_limits
                .ensure_start_allowed()
                .await
                .map_err(ExecutorError::Io);
        };
        let cached = self.rollout_path.lock().await.clone();
        let path = match cached {
            Some(path) => Some(path),
            None => {
                let found = find_rollout_file(&thread_id).await;
                if let Some(path) = &found {
                    *self.rollout_path.lock().await = Some(path.clone());
                }
                found
            }
        };
        let result = match path {
            Some(path) => self.storage_limits.ensure_rollout_allowed(&path).await,
            None => self.storage_limits.ensure_start_allowed().await,
        };
        if let Err(error) = result {
            *self.exit_outcome.lock().unwrap() = Some(
                NormalizedExecutionOutcome::new(ExecutionOutcomeClass::TaskFailed)
                    .with_provider_code("local_storage_limit"),
            );
            self.cancel.cancel();
            return Err(ExecutorError::Io(error));
        }
        Ok(())
    }

    async fn send_message<M>(&self, message: &M) -> Result<(), ExecutorError>
    where
        M: Serialize + Sync,
    {
        self.rpc().send(message).await
    }

    async fn send_request<R>(&self, request: ClientRequest, label: &str) -> Result<R, ExecutorError>
    where
        R: DeserializeOwned + std::fmt::Debug,
    {
        let request_id = request_id(&request);
        self.rpc()
            .request(request_id, &request, label, self.cancel.clone())
            .await
    }

    fn next_request_id(&self) -> RequestId {
        self.rpc().next_request_id()
    }

    fn command_execution_decision(
        &self,
        status: &ApprovalStatus,
    ) -> (CommandExecutionApprovalDecision, Option<String>) {
        command_execution_decision(self.auto_approve, status)
    }

    fn file_change_decision(
        &self,
        status: &ApprovalStatus,
    ) -> (FileChangeApprovalDecision, Option<String>) {
        file_change_decision(self.auto_approve, status)
    }

    async fn enqueue_feedback(&self, message: String) {
        if message.trim().is_empty() {
            return;
        }
        let mut guard = self.pending_feedback.lock().await;
        guard.push_back(message);
    }

    /// Sends pending feedback messages as new turns.
    /// Returns `true` if any messages were sent.
    async fn flush_pending_feedback(&self) -> bool {
        let messages: Vec<String> = {
            let mut guard = self.pending_feedback.lock().await;
            guard.drain(..).collect()
        };

        if messages.is_empty() {
            return false;
        }

        let Some(thread_id) = self.thread_id.lock().await.clone() else {
            tracing::warn!(
                "pending Codex feedback but thread id unavailable; dropping {} messages",
                messages.len()
            );
            return false;
        };

        let mut sent = false;
        for message in messages {
            let trimmed = message.trim();
            if trimmed.is_empty() {
                continue;
            }
            self.spawn_user_message(thread_id.clone(), format!("User feedback: {trimmed}"));
            sent = true;
        }
        sent
    }

    fn spawn_turn_start(
        &self,
        thread_id: String,
        message: String,
        collaboration_mode: Option<CollaborationMode>,
    ) {
        let peer = self.rpc().clone();
        let cancel = self.cancel.clone();
        let request = ClientRequest::TurnStart {
            request_id: peer.next_request_id(),
            params: TurnStartParams {
                thread_id,
                input: vec![UserInput::Text {
                    text: message,
                    text_elements: vec![],
                }],
                collaboration_mode,
                ..Default::default()
            },
        };
        tokio::spawn(async move {
            if let Err(err) = peer
                .request::<TurnStartResponse, _>(
                    request_id(&request),
                    &request,
                    "turn/start",
                    cancel,
                )
                .await
            {
                tracing::error!("failed to send user message: {err}");
            }
        });
    }

    fn spawn_user_message(&self, thread_id: String, message: String) {
        self.spawn_turn_start(thread_id, message, None);
    }
}

#[async_trait]
impl JsonRpcCallbacks for AppServerClient {
    async fn on_request(
        &self,
        peer: &JsonRpcPeer,
        raw: &str,
        request: JSONRPCRequest,
    ) -> Result<(), ExecutorError> {
        self.log_writer.log_raw(raw).await?;
        self.enforce_storage_limits().await?;
        match ServerRequest::try_from(request.clone()) {
            Ok(server_request) => self.handle_server_request(peer, server_request).await,
            Err(err) => {
                tracing::debug!("Unhandled server request `{}`: {err}", request.method);
                let response = JSONRPCResponse {
                    id: request.id,
                    result: Value::Null,
                };
                peer.send(&response).await
            }
        }
    }

    async fn on_response(
        &self,
        _peer: &JsonRpcPeer,
        raw: &str,
        _response: &JSONRPCResponse,
    ) -> Result<(), ExecutorError> {
        self.log_writer.log_raw(raw).await?;
        self.enforce_storage_limits().await
    }

    async fn on_error(
        &self,
        _peer: &JsonRpcPeer,
        raw: &str,
        _error: &JSONRPCError,
    ) -> Result<(), ExecutorError> {
        self.log_writer.log_raw(raw).await?;
        self.enforce_storage_limits().await
    }

    async fn on_notification(
        &self,
        _peer: &JsonRpcPeer,
        raw: &str,
        notification: JSONRPCNotification,
    ) -> Result<bool, ExecutorError> {
        self.log_writer.log_raw(raw).await?;
        self.enforce_storage_limits().await?;

        let method = notification.method.as_str();

        // Detect completed plan items in the notification stream
        if self.plan_mode
            && method == "item/completed"
            && let Some(ref params) = notification.params
            && let Ok(completed) =
                serde_json::from_value::<ItemCompletedNotification>(params.clone())
            && let ThreadItem::Plan { id, .. } = completed.item
        {
            *self.pending_plan.lock().await = Some(PendingPlan { item_id: id });
        }

        // V2 turn completion detection
        if method == "turn/completed" {
            let mut keep_alive = false;

            let completed = notification.params.and_then(|params| {
                serde_json::from_value::<TurnCompletedNotification>(params).ok()
            });

            if let Some(completed) = &completed {
                match completed.turn.status {
                    TurnStatus::Interrupted => {
                        tracing::debug!("codex turn interrupted; flushing feedback queue");
                        if self.flush_pending_feedback().await {
                            keep_alive = true;
                        }
                    }
                    TurnStatus::Failed => {
                        let outcome = normalized_turn_failure(completed.turn.error.as_ref());
                        *self.exit_outcome.lock().unwrap() = Some(outcome);
                    }
                    TurnStatus::Completed | TurnStatus::InProgress => {}
                }
            }

            // Handle plan approval on turn completion
            let pending = if self.plan_mode {
                self.pending_plan.lock().await.take()
            } else {
                None
            };
            if let Some(plan) = pending {
                return self.handle_plan_completed(plan).await;
            }

            // Handle commit reminder on turn completion
            if !keep_alive
                && self.commit_reminder
                && !self.commit_reminder_sent.swap(true, Ordering::SeqCst)
                && let status = self.repo_context.check_uncommitted_changes().await
                && !status.is_empty()
                && let Some(thread_id) = self.thread_id.lock().await.clone()
            {
                let prompt = format!("{}\n{}", self.commit_reminder_prompt, status);
                self.spawn_user_message(thread_id, prompt);
                return Ok(false);
            }

            return Ok(!keep_alive);
        }

        Ok(false)
    }

    async fn on_non_json(&self, raw: &str) -> Result<(), ExecutorError> {
        self.log_writer.log_raw(raw).await?;
        self.enforce_storage_limits().await
    }

    async fn final_exit_result(&self) -> ExecutorExitResult {
        match self.exit_outcome.lock().unwrap().take() {
            Some(outcome) => ExecutorExitResult::Failure(Some(outcome)),
            None => ExecutorExitResult::Success,
        }
    }
}

/// Map Codex's stable structured turn error taxonomy to the normalized
/// outcome contract. Only `codex_error_info` (a stable enum) drives the
/// classification; raw `message` text is never used, so unrecognized errors
/// map to `Unknown` instead of being guessed from text.
fn normalized_turn_failure(
    error: Option<&codex_app_server_protocol::TurnError>,
) -> NormalizedExecutionOutcome {
    use codex_app_server_protocol::CodexErrorInfo;

    let Some(info) = error.and_then(|error| error.codex_error_info.as_ref()) else {
        return NormalizedExecutionOutcome::new(ExecutionOutcomeClass::Unknown);
    };

    match info {
        CodexErrorInfo::UsageLimitExceeded => {
            NormalizedExecutionOutcome::new(ExecutionOutcomeClass::QuotaExhausted)
                .with_provider_code("usage_limit_exceeded")
        }
        CodexErrorInfo::SessionBudgetExceeded => {
            NormalizedExecutionOutcome::new(ExecutionOutcomeClass::QuotaExhausted)
                .with_provider_code("session_budget_exceeded")
        }
        CodexErrorInfo::Unauthorized => {
            NormalizedExecutionOutcome::new(ExecutionOutcomeClass::AuthExpired)
                .with_provider_code("unauthorized")
        }
        CodexErrorInfo::ServerOverloaded => {
            NormalizedExecutionOutcome::new(ExecutionOutcomeClass::NetworkTransient)
                .with_provider_code("server_overloaded")
        }
        CodexErrorInfo::InternalServerError => {
            NormalizedExecutionOutcome::new(ExecutionOutcomeClass::NetworkTransient)
                .with_provider_code("internal_server_error")
        }
        CodexErrorInfo::HttpConnectionFailed { http_status_code }
        | CodexErrorInfo::ResponseStreamConnectionFailed { http_status_code }
        | CodexErrorInfo::ResponseStreamDisconnected { http_status_code }
        | CodexErrorInfo::ResponseTooManyFailedAttempts { http_status_code } => {
            NormalizedExecutionOutcome::from_http_status(*http_status_code)
        }
        CodexErrorInfo::ContextWindowExceeded => {
            NormalizedExecutionOutcome::new(ExecutionOutcomeClass::TaskFailed)
                .with_provider_code("context_window_exceeded")
        }
        _ => NormalizedExecutionOutcome::new(ExecutionOutcomeClass::Unknown),
    }
}

async fn send_server_response<T>(
    peer: &JsonRpcPeer,
    request_id: RequestId,
    response: T,
) -> Result<(), ExecutorError>
where
    T: Serialize,
{
    let payload = JSONRPCResponse {
        id: request_id,
        result: serde_json::to_value(response)
            .map_err(|err| ExecutorError::Io(io::Error::other(err.to_string())))?,
    };

    peer.send(&payload).await
}

/// Convert our `HashMap<question_text, Vec<answer_labels>>` answer format to
/// Codex's `HashMap<question_id, ToolRequestUserInputAnswer>` format.
fn answers_to_codex_format(
    questions: &[ToolRequestUserInputQuestion],
    answers: &HashMap<String, Vec<String>>,
) -> ToolRequestUserInputResponse {
    let codex_answers = questions
        .iter()
        .filter_map(|q| {
            answers.get(&q.question).map(|answer_vec| {
                (
                    q.id.clone(),
                    ToolRequestUserInputAnswer {
                        answers: answer_vec.clone(),
                    },
                )
            })
        })
        .collect();

    ToolRequestUserInputResponse {
        answers: codex_answers,
    }
}

fn request_id(request: &ClientRequest) -> RequestId {
    match request {
        ClientRequest::Initialize { request_id, .. }
        | ClientRequest::ThreadStart { request_id, .. }
        | ClientRequest::ThreadFork { request_id, .. }
        | ClientRequest::TurnStart { request_id, .. }
        | ClientRequest::GetAccount { request_id, .. }
        | ClientRequest::ReviewStart { request_id, .. }
        | ClientRequest::McpServerStatusList { request_id, .. }
        | ClientRequest::ThreadCompactStart { request_id, .. }
        | ClientRequest::ThreadRead { request_id, .. }
        | ClientRequest::ConfigRead { request_id, .. }
        | ClientRequest::ConfigBatchWrite { request_id, .. }
        | ClientRequest::GetAccountRateLimits { request_id, .. } => request_id.clone(),
        _ => unreachable!("request_id called for unsupported request variant"),
    }
}

#[derive(Clone)]
pub struct LogWriter {
    writer: Arc<Mutex<BufWriter<Box<dyn AsyncWrite + Send + Unpin>>>>,
}

impl LogWriter {
    pub fn new(writer: impl AsyncWrite + Send + Unpin + 'static) -> Self {
        Self {
            writer: Arc::new(Mutex::new(BufWriter::new(Box::new(writer)))),
        }
    }

    pub async fn log_raw(&self, raw: &str) -> Result<(), ExecutorError> {
        let mut guard = self.writer.lock().await;
        guard
            .write_all(raw.as_bytes())
            .await
            .map_err(ExecutorError::Io)?;
        guard.write_all(b"\n").await.map_err(ExecutorError::Io)?;
        guard.flush().await.map_err(ExecutorError::Io)?;
        Ok(())
    }
}

#[cfg(test)]
mod outcome_mapping_tests {
    use codex_app_server_protocol::{CodexErrorInfo, TurnError};

    use super::*;

    fn turn_error(info: CodexErrorInfo) -> TurnError {
        TurnError {
            message: "raw provider text that must never be classified".to_string(),
            codex_error_info: Some(info),
            additional_details: None,
        }
    }

    #[test]
    fn usage_limit_maps_to_quota_exhausted() {
        let outcome =
            normalized_turn_failure(Some(&turn_error(CodexErrorInfo::UsageLimitExceeded)));
        assert_eq!(outcome.class, ExecutionOutcomeClass::QuotaExhausted);
        assert_eq!(
            outcome.provider_code.as_deref(),
            Some("usage_limit_exceeded")
        );
    }

    #[test]
    fn unauthorized_maps_to_auth_expired() {
        let outcome = normalized_turn_failure(Some(&turn_error(CodexErrorInfo::Unauthorized)));
        assert_eq!(outcome.class, ExecutionOutcomeClass::AuthExpired);
        assert_eq!(outcome.provider_code.as_deref(), Some("unauthorized"));
    }

    #[test]
    fn transport_failures_map_by_http_status() {
        let throttled = normalized_turn_failure(Some(&turn_error(
            CodexErrorInfo::ResponseStreamConnectionFailed {
                http_status_code: Some(429),
            },
        )));
        assert_eq!(throttled.class, ExecutionOutcomeClass::RateLimitedTransient);
        assert_eq!(throttled.provider_code.as_deref(), Some("http_429"));

        let disconnected = normalized_turn_failure(Some(&turn_error(
            CodexErrorInfo::ResponseStreamDisconnected {
                http_status_code: None,
            },
        )));
        assert_eq!(disconnected.class, ExecutionOutcomeClass::NetworkTransient);
    }

    #[test]
    fn overloaded_maps_to_network_transient() {
        let outcome = normalized_turn_failure(Some(&turn_error(CodexErrorInfo::ServerOverloaded)));
        assert_eq!(outcome.class, ExecutionOutcomeClass::NetworkTransient);
        assert_eq!(outcome.provider_code.as_deref(), Some("server_overloaded"));
    }

    #[test]
    fn missing_or_unrecognized_signal_maps_to_unknown_without_provider_text() {
        let missing = normalized_turn_failure(None);
        assert_eq!(missing.class, ExecutionOutcomeClass::Unknown);
        assert_eq!(missing.provider_code, None);

        let unrecognized = normalized_turn_failure(Some(&turn_error(CodexErrorInfo::Other)));
        assert_eq!(unrecognized.class, ExecutionOutcomeClass::Unknown);
        // Raw provider message text must never leak into the safe contract.
        assert!(!unrecognized.safe_message.contains("raw provider text"));
    }
}

/// Scope for a command approval.
///
/// Codex documents `acceptForSession` as "future prompts in the same
/// session-scoped approval cache run without prompting", so the command is
/// both what is being asked about and what a session decision covers. It is
/// never empty, because Codex offers `acceptForSession` on every command
/// approval whether or not it chose to send the command text.
fn command_patterns(params: &CommandExecutionRequestApprovalParams) -> ApprovalPatterns {
    let command = params
        .command
        .clone()
        .unwrap_or_else(|| "this command".to_string());
    ApprovalPatterns {
        request: vec![command.clone()],
        session: vec![command],
    }
}

/// Scope for a file-change approval.
///
/// `acceptForSession` here means "future changes to the same files run without
/// prompting", and `grantRoot` - when Codex sends it - is the wider write root
/// it is asking for. Neither is a path list Codex hands over, so the request
/// side names the change itself rather than inventing a glob.
fn file_change_patterns(params: &FileChangeRequestApprovalParams) -> ApprovalPatterns {
    let request = match &params.grant_root {
        Some(root) => vec![format!("write under {}", root.display())],
        None => vec!["the files in this change".to_string()],
    };
    ApprovalPatterns {
        session: request.clone(),
        request,
    }
}

fn command_execution_decision(
    auto_approve: bool,
    status: &ApprovalStatus,
) -> (CommandExecutionApprovalDecision, Option<String>) {
    if auto_approve {
        return (CommandExecutionApprovalDecision::AcceptForSession, None);
    }

    match status {
        ApprovalStatus::Approved {
            scope: ApprovalScope::Session,
        } => (CommandExecutionApprovalDecision::AcceptForSession, None),
        ApprovalStatus::Approved {
            scope: ApprovalScope::Once,
        } => (CommandExecutionApprovalDecision::Accept, None),
        ApprovalStatus::Denied { reason } => {
            let feedback = reason
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            if feedback.is_some() {
                (CommandExecutionApprovalDecision::Cancel, feedback)
            } else {
                (CommandExecutionApprovalDecision::Decline, None)
            }
        }
        ApprovalStatus::TimedOut => (CommandExecutionApprovalDecision::Decline, None),
        ApprovalStatus::Pending => (CommandExecutionApprovalDecision::Decline, None),
    }
}

fn file_change_decision(
    auto_approve: bool,
    status: &ApprovalStatus,
) -> (FileChangeApprovalDecision, Option<String>) {
    if auto_approve {
        return (FileChangeApprovalDecision::AcceptForSession, None);
    }

    match status {
        ApprovalStatus::Approved {
            scope: ApprovalScope::Session,
        } => (FileChangeApprovalDecision::AcceptForSession, None),
        ApprovalStatus::Approved {
            scope: ApprovalScope::Once,
        } => (FileChangeApprovalDecision::Accept, None),
        ApprovalStatus::Denied { reason } => {
            let feedback = reason
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            if feedback.is_some() {
                (FileChangeApprovalDecision::Cancel, feedback)
            } else {
                (FileChangeApprovalDecision::Decline, None)
            }
        }
        ApprovalStatus::TimedOut => (FileChangeApprovalDecision::Decline, None),
        ApprovalStatus::Pending => (FileChangeApprovalDecision::Decline, None),
    }
}

#[cfg(test)]
mod permission_tests {
    use super::*;

    /// Built through the protocol crate's own deserializer so the wire shape,
    /// not this test, decides which fields an approval request really has.
    fn exec_params(command: Option<&str>) -> CommandExecutionRequestApprovalParams {
        serde_json::from_value(serde_json::json!({
            "threadId": "thread",
            "turnId": "turn",
            "itemId": "item",
            "startedAtMs": 0,
            "command": command,
        }))
        .expect("params match the protocol shape")
    }

    fn file_change_params(grant_root: Option<&str>) -> FileChangeRequestApprovalParams {
        serde_json::from_value(serde_json::json!({
            "threadId": "thread",
            "turnId": "turn",
            "itemId": "item",
            "startedAtMs": 0,
            "reason": null,
            "grantRoot": grant_root,
        }))
        .expect("params match the protocol shape")
    }

    /// Codex offers `acceptForSession` on every approval it raises, so the
    /// session action must always be on offer for it - unlike Claude Code,
    /// which names a rule only sometimes.
    #[test]
    fn codex_always_offers_a_session_scope() {
        for patterns in [
            command_patterns(&exec_params(Some("cargo test"))),
            command_patterns(&exec_params(None)),
            file_change_patterns(&file_change_params(Some("/repo"))),
            file_change_patterns(&file_change_params(None)),
        ] {
            assert!(!patterns.is_once_only());
            assert!(!patterns.request.is_empty());
        }
    }

    #[test]
    fn a_command_approval_names_the_command() {
        let patterns = command_patterns(&exec_params(Some("cargo test")));
        assert_eq!(patterns.request, vec!["cargo test".to_string()]);
        assert_eq!(patterns.session, patterns.request);
    }

    #[test]
    fn a_write_root_is_named_when_codex_asks_for_one() {
        let patterns = file_change_patterns(&file_change_params(Some("/repo")));
        assert_eq!(patterns.request, vec!["write under /repo".to_string()]);
    }

    #[test]
    fn scope_selects_the_session_cache() {
        let session = ApprovalStatus::Approved {
            scope: ApprovalScope::Session,
        };
        let once = ApprovalStatus::Approved {
            scope: ApprovalScope::Once,
        };

        assert_eq!(
            command_execution_decision(false, &session).0,
            CommandExecutionApprovalDecision::AcceptForSession
        );
        assert_eq!(
            command_execution_decision(false, &once).0,
            CommandExecutionApprovalDecision::Accept
        );
        assert_eq!(
            file_change_decision(false, &session).0,
            FileChangeApprovalDecision::AcceptForSession
        );
        assert_eq!(
            file_change_decision(false, &once).0,
            FileChangeApprovalDecision::Accept
        );
    }

    /// A denial with something to say cancels the turn so the feedback is
    /// actually delivered; a bare denial only declines the one call.
    #[test]
    fn only_a_denial_with_feedback_interrupts_the_turn() {
        let spoken = ApprovalStatus::Denied {
            reason: Some("run the fixture instead".to_string()),
        };
        let (decision, feedback) = command_execution_decision(false, &spoken);
        assert_eq!(decision, CommandExecutionApprovalDecision::Cancel);
        assert_eq!(feedback.as_deref(), Some("run the fixture instead"));

        for silent in [
            ApprovalStatus::Denied { reason: None },
            ApprovalStatus::Denied {
                reason: Some("   ".to_string()),
            },
            ApprovalStatus::TimedOut,
        ] {
            let (decision, feedback) = command_execution_decision(false, &silent);
            assert_eq!(decision, CommandExecutionApprovalDecision::Decline);
            assert!(feedback.is_none());
        }
    }
}
