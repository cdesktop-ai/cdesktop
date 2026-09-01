use std::{
    io,
    path::{Component, Path},
    sync::Arc,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use ts_rs::TS;
use workspace_utils::{command_ext::GroupSpawnNoWindowExt, shell::get_shell_command};

use crate::{
    actions::Executable,
    approvals::ExecutorApprovalService,
    env::ExecutionEnv,
    executors::{ExecutorError, SpawnedChild},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub enum ScriptRequestLanguage {
    Bash,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub enum ScriptContext {
    SetupScript,
    CleanupScript,
    ArchiveScript,
    DevServer,
    ToolInstallScript,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct ScriptRequest {
    pub script: String,
    pub language: ScriptRequestLanguage,
    pub context: ScriptContext,
    /// Optional relative path to execute the script in (relative to container_ref).
    /// If None, uses the container_ref directory directly.
    #[serde(default)]
    pub working_dir: Option<String>,
}

#[async_trait]
impl Executable for ScriptRequest {
    async fn spawn(
        &self,
        current_dir: &Path,
        _approvals: Arc<dyn ExecutorApprovalService>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        // Script working directories are repository-relative to the workspace
        // root. The inherited cwd belongs to the executor action and can be a
        // primary repo path (or no longer exist), so it is not a safe base for
        // resolving another repository's setup script.
        let effective_dir = match &self.working_dir {
            Some(rel_path) => {
                let rel_path = Path::new(rel_path);
                if rel_path.is_absolute()
                    || rel_path
                        .components()
                        .any(|component| matches!(component, Component::ParentDir))
                {
                    return Err(ExecutorError::Io(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "script working_dir must be a safe relative path",
                    )));
                }
                env.repo_context.workspace_root.join(rel_path)
            }
            None => current_dir.to_path_buf(),
        };

        let (shell_cmd, shell_arg) = get_shell_command();
        let mut command = Command::new(shell_cmd);
        command
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .arg(shell_arg)
            .arg(&self.script)
            .current_dir(&effective_dir);

        // Apply environment variables
        env.apply_to_command(&mut command);

        let child = command.group_spawn_no_window()?;

        Ok(child.into())
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, time::SystemTime};

    use super::*;
    use crate::{
        approvals::NoopExecutorApprovalService,
        env::{ExecutionEnv, RepoContext},
    };

    fn temp_workspace_root() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("cdesktop-script-request-{unique}"))
    }

    #[tokio::test]
    async fn setup_script_uses_workspace_root_when_inherited_cwd_is_missing() {
        let workspace_root = temp_workspace_root();
        let repo_dir = workspace_root.join("catapult-games");
        fs::create_dir_all(&repo_dir).unwrap();

        let request = ScriptRequest {
            script: "pwd > .setup-script-cwd".to_string(),
            language: ScriptRequestLanguage::Bash,
            context: ScriptContext::SetupScript,
            working_dir: Some("catapult-games".to_string()),
        };
        let env = ExecutionEnv::new(
            RepoContext::new(
                workspace_root.clone(),
                vec!["catapult-games".to_string()],
                vec![repo_dir.clone()],
            ),
            false,
            String::new(),
        );
        let missing_inherited_cwd = workspace_root.join("gone");
        let expected_cwd = fs::canonicalize(&repo_dir).unwrap();

        let result = async {
            let mut child = request
                .spawn(
                    &missing_inherited_cwd,
                    Arc::new(NoopExecutorApprovalService {}),
                    &env,
                )
                .await?;
            assert!(child.child.wait().await?.success());
            Ok::<_, ExecutorError>(fs::read_to_string(repo_dir.join(".setup-script-cwd"))?)
        }
        .await;

        fs::remove_dir_all(&workspace_root).unwrap();
        assert_eq!(result.unwrap().trim(), expected_cwd.display().to_string());
    }

    #[tokio::test]
    async fn setup_script_rejects_unsafe_working_dirs() {
        let env = ExecutionEnv::new(RepoContext::default(), false, String::new());

        let unsafe_dirs = if cfg!(windows) {
            ["../outside", "C:\\outside"]
        } else {
            ["../outside", "/outside"]
        };
        for working_dir in unsafe_dirs {
            let request = ScriptRequest {
                script: "true".to_string(),
                language: ScriptRequestLanguage::Bash,
                context: ScriptContext::SetupScript,
                working_dir: Some(working_dir.to_string()),
            };

            let error = request
                .spawn(
                    Path::new("/tmp"),
                    Arc::new(NoopExecutorApprovalService {}),
                    &env,
                )
                .await
                .unwrap_err();
            assert!(
                matches!(error, ExecutorError::Io(error) if error.kind() == io::ErrorKind::InvalidInput),
                "{working_dir} should be rejected"
            );
        }
    }
}
