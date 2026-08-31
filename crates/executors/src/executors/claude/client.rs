use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use workspace_utils::approvals::{ApprovalPatterns, ApprovalScope, ApprovalStatus, QuestionStatus};

use super::types::PermissionMode;
use crate::{
    approvals::{ExecutorApprovalError, ExecutorApprovalService},
    env::RepoContext,
    executors::{
        ExecutorError,
        claude::{
            ClaudeJson,
            types::{
                PermissionResult, PermissionUpdate, PermissionUpdateDestination,
                PermissionUpdateType,
            },
        },
        codex::client::LogWriter,
    },
};

const EXIT_PLAN_MODE_NAME: &str = "ExitPlanMode";
const ASK_USER_QUESTION_NAME: &str = "AskUserQuestion";
pub const AUTO_APPROVE_CALLBACK_ID: &str = "AUTO_APPROVE_CALLBACK_ID";
pub const STOP_GIT_CHECK_CALLBACK_ID: &str = "STOP_GIT_CHECK_CALLBACK_ID";
// Prefix for denial messages from the user, mirrors claude code CLI behavior
const TOOL_DENY_PREFIX: &str = "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). To tell you how to proceed, the user said: ";

/// Render a CLI permission update as the rule text an operator reads.
///
/// Only `addRules` updates carry a scope worth showing; a mode switch or a
/// rule removal is not something an operator is being asked to allow.
fn describe_permission(update: &PermissionUpdate) -> Vec<String> {
    if !matches!(update.update_type, PermissionUpdateType::AddRules) {
        return Vec::new();
    }
    update
        .rules
        .iter()
        .flatten()
        .map(|rule| match &rule.rule_content {
            Some(content) => format!("{}({})", rule.tool_name, content),
            None => rule.tool_name.clone(),
        })
        .collect()
}

/// Turn an operator's session-scoped approval into permission updates.
///
/// The updates are the CLI's own suggestions with their destination forced to
/// `session`: a suggestion may name `userSettings` or `projectSettings`, and
/// honouring that would write a rule into the operator's files that outlives
/// the run they approved it for.
///
/// No suggestions means Claude Code offered no rule for this request, so there
/// is nothing to persist and the approval stays one-shot. Verified against
/// Claude Code 2.1.232: on cdesktop's path - a `PreToolUse` hook answering
/// `ask` - `can_use_tool` arrives with no `permission_suggestions` at all.
fn session_permission_updates(
    scope: ApprovalScope,
    suggestions: Vec<PermissionUpdate>,
) -> Option<Vec<PermissionUpdate>> {
    if scope != ApprovalScope::Session || suggestions.is_empty() {
        return None;
    }
    Some(
        suggestions
            .into_iter()
            .map(|update| PermissionUpdate {
                destination: Some(PermissionUpdateDestination::Session),
                ..update
            })
            .collect(),
    )
}

/// Claude Agent client with control protocol support
pub struct ClaudeAgentClient {
    log_writer: LogWriter,
    approvals: Option<Arc<dyn ExecutorApprovalService>>,
    auto_approve: bool, // true when approvals is None
    repo_context: RepoContext,
    commit_reminder_prompt: String,
    cancel: CancellationToken,
}

impl ClaudeAgentClient {
    /// Create a new client with optional approval service
    pub fn new(
        log_writer: LogWriter,
        approvals: Option<Arc<dyn ExecutorApprovalService>>,
        repo_context: RepoContext,
        commit_reminder_prompt: String,
        cancel: CancellationToken,
    ) -> Arc<Self> {
        let auto_approve = approvals.is_none();
        Arc::new(Self {
            log_writer,
            approvals,
            auto_approve,
            repo_context,
            commit_reminder_prompt,
            cancel,
        })
    }

    async fn handle_approval(
        &self,
        tool_use_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
        permission_suggestions: Option<Vec<PermissionUpdate>>,
    ) -> Result<PermissionResult, ExecutorError> {
        let approval_service = self
            .approvals
            .as_ref()
            .ok_or(ExecutorApprovalError::ServiceUnavailable)?;

        // The CLI's own suggestions are the only rules cdesktop will ever
        // persist for it. Deriving a rule from `tool_input` instead would mean
        // guessing a width - `Bash(echo x)` or `Bash(echo *)` - and quietly
        // granting whichever one guessed wrong.
        let session_rules = permission_suggestions.unwrap_or_default();
        let patterns = ApprovalPatterns {
            request: Vec::new(),
            session: session_rules.iter().flat_map(describe_permission).collect(),
        };

        let approval_id = match approval_service
            .create_tool_approval(&tool_name, patterns)
            .await
        {
            Ok(id) => id,
            Err(err) => {
                self.handle_approval_error(&tool_name, &tool_use_id, &err)
                    .await?;
                return Err(err.into());
            }
        };

        let _ = self
            .log_writer
            .log_raw(&serde_json::to_string(&ClaudeJson::ApprovalRequested {
                tool_call_id: tool_use_id.clone(),
                tool_name: tool_name.clone(),
                approval_id: approval_id.clone(),
            })?)
            .await;

        let status = match approval_service
            .wait_tool_approval(&approval_id, self.cancel.clone())
            .await
        {
            Ok(s) => s,
            Err(err) => {
                self.handle_approval_error(&tool_name, &tool_use_id, &err)
                    .await?;
                return Err(err.into());
            }
        };

        self.log_writer
            .log_raw(&serde_json::to_string(&ClaudeJson::ApprovalResponse {
                tool_call_id: tool_use_id.clone(),
                tool_name: tool_name.clone(),
                approval_status: status.clone(),
            })?)
            .await?;

        match status {
            ApprovalStatus::Approved { scope } => {
                if tool_name == EXIT_PLAN_MODE_NAME {
                    Ok(PermissionResult::Allow {
                        updated_input: tool_input,
                        updated_permissions: Some(vec![PermissionUpdate {
                            update_type: PermissionUpdateType::SetMode,
                            mode: Some(PermissionMode::BypassPermissions),
                            destination: Some(PermissionUpdateDestination::Session),
                            rules: None,
                            behavior: None,
                            directories: None,
                        }]),
                    })
                } else {
                    Ok(PermissionResult::Allow {
                        updated_input: tool_input,
                        updated_permissions: session_permission_updates(scope, session_rules),
                    })
                }
            }
            ApprovalStatus::Denied { reason } => Ok(PermissionResult::Deny {
                message: format!("{}{}", TOOL_DENY_PREFIX, reason.unwrap_or_default()),
                interrupt: Some(false),
            }),
            ApprovalStatus::TimedOut => Ok(PermissionResult::Deny {
                message: "Approval request timed out".to_string(),
                interrupt: Some(true),
            }),
            ApprovalStatus::Pending => Ok(PermissionResult::Deny {
                message: "Approval still pending (unexpected)".to_string(),
                interrupt: Some(false),
            }),
        }
    }

    async fn handle_question(
        &self,
        tool_use_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
    ) -> Result<PermissionResult, ExecutorError> {
        let approval_service = self
            .approvals
            .as_ref()
            .ok_or(ExecutorApprovalError::ServiceUnavailable)?;

        let question_count = tool_input
            .get("questions")
            .and_then(|q| q.as_array())
            .map(|a| a.len())
            .unwrap_or(1);

        let approval_id = match approval_service
            .create_question_approval(&tool_name, question_count)
            .await
        {
            Ok(id) => id,
            Err(err) => {
                self.handle_question_error(&tool_use_id, &tool_name, &err)
                    .await?;
                return Err(err.into());
            }
        };

        let _ = self
            .log_writer
            .log_raw(&serde_json::to_string(&ClaudeJson::ApprovalRequested {
                tool_call_id: tool_use_id.clone(),
                tool_name: tool_name.clone(),
                approval_id: approval_id.clone(),
            })?)
            .await;

        let status = match approval_service
            .wait_question_answer(&approval_id, self.cancel.clone())
            .await
        {
            Ok(s) => s,
            Err(err) => {
                self.handle_question_error(&tool_use_id, &tool_name, &err)
                    .await?;
                return Err(err.into());
            }
        };

        self.log_writer
            .log_raw(&serde_json::to_string(&ClaudeJson::QuestionResponse {
                tool_call_id: tool_use_id.clone(),
                tool_name: tool_name.clone(),
                question_status: status.clone(),
            })?)
            .await?;

        match status {
            QuestionStatus::Answered { answers } => {
                let answers_map: serde_json::Map<String, serde_json::Value> = answers
                    .iter()
                    .map(|qa| {
                        (
                            qa.question.clone(),
                            serde_json::Value::String(qa.answer.join(", ")),
                        )
                    })
                    .collect();
                let mut updated = tool_input.clone();
                if let Some(obj) = updated.as_object_mut() {
                    obj.insert(
                        "answers".to_string(),
                        serde_json::Value::Object(answers_map),
                    );
                }
                Ok(PermissionResult::Allow {
                    updated_input: updated,
                    updated_permissions: None,
                })
            }
            QuestionStatus::TimedOut => Ok(PermissionResult::Deny {
                message: "Question request timed out".to_string(),
                interrupt: Some(true),
            }),
        }
    }

    async fn handle_approval_error(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        err: &ExecutorApprovalError,
    ) -> Result<(), ExecutorError> {
        if !matches!(err, ExecutorApprovalError::Cancelled) {
            tracing::error!(
                "Claude approval failed for tool={} call_id={}: {err}",
                tool_name,
                tool_use_id
            );
        }
        let _ = self
            .log_writer
            .log_raw(&serde_json::to_string(&ClaudeJson::ApprovalResponse {
                tool_call_id: tool_use_id.to_string(),
                tool_name: tool_name.to_string(),
                approval_status: ApprovalStatus::Denied {
                    reason: Some(format!("Approval service error: {err}")),
                },
            })?)
            .await;
        Ok(())
    }

    async fn handle_question_error(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        err: &ExecutorApprovalError,
    ) -> Result<(), ExecutorError> {
        if !matches!(err, ExecutorApprovalError::Cancelled) {
            tracing::error!("Claude question failed {err}",);
        }
        let _ = self
            .log_writer
            .log_raw(&serde_json::to_string(&ClaudeJson::QuestionResponse {
                tool_call_id: tool_use_id.to_string(),
                tool_name: tool_name.to_string(),
                question_status: QuestionStatus::TimedOut,
            })?)
            .await;
        Ok(())
    }

    pub async fn on_can_use_tool(
        &self,
        tool_name: String,
        input: serde_json::Value,
        permission_suggestions: Option<Vec<PermissionUpdate>>,
        tool_use_id: Option<String>,
    ) -> Result<PermissionResult, ExecutorError> {
        if tool_name == ASK_USER_QUESTION_NAME {
            if let Some(latest_tool_use_id) = tool_use_id {
                return self
                    .handle_question(latest_tool_use_id, tool_name, input)
                    .await;
            } else {
                tracing::warn!("AskUserQuestion without tool_use_id, cannot route to approval");
                return Ok(PermissionResult::Deny {
                    message:
                        "AskUserQuestion requires user interaction but no tool_use_id was provided"
                            .to_string(),
                    interrupt: Some(false),
                });
            }
        }
        if self.auto_approve {
            Ok(PermissionResult::Allow {
                updated_input: input,
                updated_permissions: None,
            })
        } else if let Some(latest_tool_use_id) = tool_use_id {
            self.handle_approval(latest_tool_use_id, tool_name, input, permission_suggestions)
                .await
        } else {
            // Auto approve tools with no matching tool_use_id
            // tool_use_id is undocumented so this may not be possible
            tracing::warn!(
                "No tool_use_id available for tool '{}', cannot request approval",
                tool_name
            );
            Ok(PermissionResult::Allow {
                updated_input: input,
                updated_permissions: None,
            })
        }
    }

    pub async fn on_hook_callback(
        &self,
        callback_id: String,
        input: serde_json::Value,
        _tool_use_id: Option<String>,
    ) -> Result<serde_json::Value, ExecutorError> {
        // Stop hook git check - uses `decision` (approve/block) and `reason` fields
        if callback_id == STOP_GIT_CHECK_CALLBACK_ID {
            if input
                .get("stop_hook_active")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Ok(serde_json::json!({"decision": "approve"}));
            }
            let status = self.repo_context.check_uncommitted_changes().await;
            return Ok(if status.is_empty() {
                serde_json::json!({"decision": "approve"})
            } else {
                serde_json::json!({
                    "decision": "block",
                    "reason": format!("{}\n{}", self.commit_reminder_prompt, status)
                })
            });
        }

        if self.auto_approve {
            Ok(serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "permissionDecisionReason": "Auto-approved by SDK"
                }
            }))
        } else {
            match callback_id.as_str() {
                AUTO_APPROVE_CALLBACK_ID => Ok(serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                        "permissionDecisionReason": "Approved by SDK"
                    }
                })),
                _ => {
                    // Hook callbacks is only used to forward approval requests to can_use_tool.
                    // This works because `ask` decision in hook callback triggers a can_use_tool request
                    // https://docs.claude.com/en/api/agent-sdk/permissions#permission-flow-diagram
                    Ok(serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "ask",
                            "permissionDecisionReason": "Forwarding to canusetool service"
                        }
                    }))
                }
            }
        }
    }

    pub async fn log_message(&self, line: &str) -> Result<(), ExecutorError> {
        self.log_writer.log_raw(line).await
    }
}

#[cfg(test)]
mod permission_tests {
    use super::*;
    use crate::executors::claude::types::{CLIMessage, ControlRequestType, PermissionRuleValue};

    /// Verbatim `can_use_tool` frame from a live `claude` CLI (2.1.232) driven
    /// with cdesktop's own flags and hooks. See `fixtures/README.md`.
    const CAN_USE_TOOL_BASH: &str = include_str!("fixtures/can_use_tool_bash.json");

    fn can_use_tool_fixture() -> ControlRequestType {
        match serde_json::from_str::<CLIMessage>(CAN_USE_TOOL_BASH).expect("fixture parses") {
            CLIMessage::ControlRequest { request, .. } => request,
            other => panic!("expected a control request, got {other:?}"),
        }
    }

    fn add_rules(destination: PermissionUpdateDestination) -> PermissionUpdate {
        PermissionUpdate {
            update_type: PermissionUpdateType::AddRules,
            mode: None,
            destination: Some(destination),
            rules: Some(vec![PermissionRuleValue {
                tool_name: "Bash".to_string(),
                rule_content: Some("echo:*".to_string()),
            }]),
            behavior: Some("allow".to_string()),
            directories: None,
        }
    }

    /// The finding this lane acted on: cdesktop's approval path is the
    /// hook-forwarded one, and on it the CLI names no rule to persist.
    #[test]
    fn the_cli_offers_no_rule_on_the_path_cdesktop_takes() {
        let raw: serde_json::Value =
            serde_json::from_str(CAN_USE_TOOL_BASH).expect("fixture parses");
        assert_eq!(raw["request"]["decision_reason_type"], "hook");

        let ControlRequestType::CanUseTool {
            permission_suggestions,
            ..
        } = can_use_tool_fixture()
        else {
            panic!("fixture is not a can_use_tool request");
        };

        assert!(permission_suggestions.is_none());
        assert!(
            session_permission_updates(
                ApprovalScope::Session,
                permission_suggestions.unwrap_or_default(),
            )
            .is_none(),
            "with no suggestion to persist, a session approval must stay one-shot"
        );
    }

    #[test]
    fn a_once_approval_never_persists_a_rule() {
        assert!(
            session_permission_updates(
                ApprovalScope::Once,
                vec![add_rules(PermissionUpdateDestination::Session)],
            )
            .is_none()
        );
    }

    #[test]
    fn a_session_approval_never_writes_beyond_the_run() {
        let suggested = vec![
            add_rules(PermissionUpdateDestination::UserSettings),
            add_rules(PermissionUpdateDestination::ProjectSettings),
        ];

        let applied = session_permission_updates(ApprovalScope::Session, suggested)
            .expect("suggestions are persisted for a session approval");

        assert_eq!(applied.len(), 2);
        for update in applied {
            assert!(matches!(
                update.destination,
                Some(PermissionUpdateDestination::Session)
            ));
        }
    }

    #[test]
    fn only_rule_additions_are_shown_as_scope() {
        assert_eq!(
            describe_permission(&add_rules(PermissionUpdateDestination::Session)),
            vec!["Bash(echo:*)".to_string()]
        );

        let set_mode = PermissionUpdate {
            update_type: PermissionUpdateType::SetMode,
            mode: Some(PermissionMode::BypassPermissions),
            destination: Some(PermissionUpdateDestination::Session),
            rules: None,
            behavior: None,
            directories: None,
        };
        assert!(describe_permission(&set_mode).is_empty());
    }

    #[test]
    fn a_rule_without_content_reads_as_the_bare_tool() {
        let mut update = add_rules(PermissionUpdateDestination::Session);
        update.rules = Some(vec![PermissionRuleValue {
            tool_name: "WebFetch".to_string(),
            rule_content: None,
        }]);
        assert_eq!(describe_permission(&update), vec!["WebFetch".to_string()]);
    }
}
