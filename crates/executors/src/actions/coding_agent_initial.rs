use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[cfg(not(feature = "qa-mode"))]
use crate::profile::ExecutorConfigs;
use crate::{
    actions::Executable,
    approvals::ExecutorApprovalService,
    env::ExecutionEnv,
    executors::{BaseCodingAgent, ExecutorError, SpawnedChild, StandardCodingAgentExecutor},
    profile::ExecutorConfig,
};

/// What the initial prompt *is*, decided by whoever built the request.
///
/// The marker is persisted with the action, so the transcript never has to
/// infer a prompt's provenance from its text. Requests written before this
/// field existed deserialize as `User`, which renders exactly as before.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    /// A prompt a human typed into the composer.
    #[default]
    User,
    /// Bootstrap instructions handed to a teammate at spawn time.
    Spawn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct CodingAgentInitialRequest {
    pub prompt: String,
    /// Provenance of `prompt`. Defaults to `User` for back-compat with
    /// actions persisted before the marker existed.
    #[serde(default)]
    pub prompt_kind: PromptKind,
    /// Unified executor identity + overrides
    #[serde(alias = "executor_profile_id", alias = "profile_variant_label")]
    pub executor_config: ExecutorConfig,
    /// Optional relative path to execute the agent in (relative to container_ref).
    /// If None, uses the container_ref directory directly.
    #[serde(default)]
    pub working_dir: Option<String>,
}

impl CodingAgentInitialRequest {
    pub fn base_executor(&self) -> BaseCodingAgent {
        self.executor_config.executor
    }

    pub fn effective_dir(&self, current_dir: &Path) -> std::path::PathBuf {
        match &self.working_dir {
            Some(rel_path) => current_dir.join(rel_path),
            None => current_dir.to_path_buf(),
        }
    }
}

#[async_trait]
impl Executable for CodingAgentInitialRequest {
    #[cfg_attr(feature = "qa-mode", allow(unused_variables))]
    async fn spawn(
        &self,
        current_dir: &Path,
        approvals: Arc<dyn ExecutorApprovalService>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let effective_dir = self.effective_dir(current_dir);

        #[cfg(feature = "qa-mode")]
        {
            tracing::info!("QA mode: using mock executor instead of real agent");
            let executor = crate::executors::qa_mock::QaMockExecutor;
            return executor.spawn(&effective_dir, &self.prompt, env).await;
        }

        #[cfg(not(feature = "qa-mode"))]
        {
            let profile_id = self.executor_config.profile_id();
            let mut agent = ExecutorConfigs::get_cached()
                .get_coding_agent(&profile_id)
                .ok_or(ExecutorError::UnknownExecutorType(profile_id.to_string()))?;

            if self.executor_config.has_overrides() {
                agent.apply_overrides(&self.executor_config);
            }
            agent.use_approvals(approvals.clone());

            agent.spawn(&effective_dir, &self.prompt, env).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Actions persisted before the marker existed must keep rendering as
    /// ordinary user prompts.
    #[test]
    fn legacy_request_without_marker_deserializes_as_user_prompt() {
        let legacy = serde_json::json!({
            "prompt": "ship it",
            "executor_config": { "executor": "CLAUDE_CODE" },
        });

        let request: CodingAgentInitialRequest = serde_json::from_value(legacy).unwrap();

        assert_eq!(request.prompt_kind, PromptKind::User);
    }

    #[test]
    fn marker_round_trips_through_persisted_json() {
        let request = CodingAgentInitialRequest {
            prompt: "bootstrap".to_string(),
            prompt_kind: PromptKind::Spawn,
            executor_config: ExecutorConfig::new(BaseCodingAgent::ClaudeCode),
            working_dir: None,
        };

        let persisted = serde_json::to_value(&request).unwrap();
        assert_eq!(persisted["prompt_kind"], "spawn");

        let restored: CodingAgentInitialRequest = serde_json::from_value(persisted).unwrap();
        assert_eq!(restored.prompt_kind, PromptKind::Spawn);
    }
}
