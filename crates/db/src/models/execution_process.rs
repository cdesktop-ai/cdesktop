use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use executors::{
    actions::{ExecutorAction, ExecutorActionType},
    profile::{ExecutorConfig, ExecutorProfileId},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, SqlitePool, Type};
use thiserror::Error;
use ts_rs::TS;
use uuid::Uuid;

use super::{
    execution_process_repo_state::{CreateExecutionProcessRepoState, ExecutionProcessRepoState},
    repo::Repo,
    session::Session,
    workspace::Workspace,
    workspace_repo::WorkspaceRepo,
};

#[derive(Debug, Error)]
pub enum ExecutionProcessError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("Execution process not found")]
    ExecutionProcessNotFound,
    #[error("Failed to create execution process: {0}")]
    CreateFailed(String),
    #[error("Failed to update execution process: {0}")]
    UpdateFailed(String),
    #[error("Invalid executor action format")]
    InvalidExecutorAction,
    #[error("Validation error: {0}")]
    ValidationError(String),
}

#[cfg(test)]
mod tests {
    use executors::{
        actions::{
            ExecutorAction, ExecutorActionType,
            script::{ScriptContext, ScriptRequest, ScriptRequestLanguage},
        },
        outcome::{ExecutionOutcomeClass, NormalizedExecutionOutcome},
    };
    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    use super::{ExecutionProcess, ExecutionProcessStatus, ExecutorActionField};

    #[test]
    fn boxed_executor_action_preserves_json_shape() {
        let action = ExecutorAction::new(
            ExecutorActionType::ScriptRequest(ScriptRequest {
                script: "echo ready".to_string(),
                language: ScriptRequestLanguage::Bash,
                context: ScriptContext::SetupScript,
                working_dir: None,
            }),
            None,
        );
        let expected = serde_json::to_value(&action).unwrap();
        let field = ExecutorActionField::ExecutorAction(Box::new(action));

        assert_eq!(serde_json::to_value(&field).unwrap(), expected);
        assert!(matches!(
            serde_json::from_value::<ExecutorActionField>(expected).unwrap(),
            ExecutorActionField::ExecutorAction(_)
        ));
    }

    #[tokio::test]
    async fn complete_running_attempt_is_exact_once() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"CREATE TABLE execution_processes (
                id BLOB PRIMARY KEY NOT NULL,
                status TEXT NOT NULL,
                exit_code INTEGER,
                completed_at TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE execution_process_outcomes (
                execution_process_id BLOB PRIMARY KEY NOT NULL,
                outcome TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO execution_processes (id, status, exit_code, completed_at) \
             VALUES (?, 'running', NULL, NULL)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            ExecutionProcess::complete_running_attempt(
                &pool,
                id,
                ExecutionProcessStatus::Completed,
                Some(0),
                None,
            )
            .await
            .unwrap()
        );
        assert!(
            !ExecutionProcess::complete_running_attempt(
                &pool,
                id,
                ExecutionProcessStatus::Failed,
                Some(1),
                Some(&NormalizedExecutionOutcome::new(
                    ExecutionOutcomeClass::Unknown
                )),
            )
            .await
            .unwrap()
        );

        let (status, exit_code): (String, i64) =
            sqlx::query_as("SELECT status, exit_code FROM execution_processes WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(exit_code, 0);
        // The losing duplicate completion must not attach its outcome either.
        let outcomes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_process_outcomes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(outcomes, 0);

        let missing = ExecutionProcess::complete_running_attempt(
            &pool,
            Uuid::new_v4(),
            ExecutionProcessStatus::Completed,
            Some(0),
            None,
        )
        .await
        .unwrap();
        assert!(!missing);
    }

    #[tokio::test]
    async fn winning_failed_attempt_records_normalized_outcome_once() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"CREATE TABLE execution_processes (
                id BLOB PRIMARY KEY NOT NULL,
                status TEXT NOT NULL,
                exit_code INTEGER,
                completed_at TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE execution_process_outcomes (
                execution_process_id BLOB PRIMARY KEY NOT NULL,
                outcome TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO execution_processes (id, status) VALUES (?, 'running')")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();

        let outcome = NormalizedExecutionOutcome::new(ExecutionOutcomeClass::QuotaExhausted)
            .with_provider_code("usage_limit_exceeded");
        assert!(
            ExecutionProcess::complete_running_attempt(
                &pool,
                id,
                ExecutionProcessStatus::Failed,
                Some(1),
                Some(&outcome),
            )
            .await
            .unwrap()
        );

        let stored: String = sqlx::query_scalar(
            "SELECT outcome FROM execution_process_outcomes WHERE execution_process_id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let stored: NormalizedExecutionOutcome = serde_json::from_str(&stored).unwrap();
        assert_eq!(stored.class, ExecutionOutcomeClass::QuotaExhausted);
        assert_eq!(
            stored.provider_code.as_deref(),
            Some("usage_limit_exceeded")
        );
    }
}

#[derive(Debug, Clone, Type, Serialize, Deserialize, PartialEq, TS)]
#[sqlx(type_name = "execution_process_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[ts(use_ts_enum)]
pub enum ExecutionProcessStatus {
    Running,
    Completed,
    Failed,
    Killed,
}

#[derive(Debug, Clone, Type, Serialize, Deserialize, PartialEq, TS)]
#[sqlx(type_name = "execution_process_run_reason", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ExecutionProcessRunReason {
    SetupScript,
    CleanupScript,
    ArchiveScript,
    CodingAgent,
    DevServer,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct ExecutionProcess {
    pub id: Uuid,
    pub session_id: Uuid,
    pub run_reason: ExecutionProcessRunReason,
    #[ts(type = "ExecutorAction")]
    pub executor_action: sqlx::types::Json<ExecutorActionField>,
    pub status: ExecutionProcessStatus,
    pub exit_code: Option<i64>,
    /// dropped: true if this process is excluded from the current
    /// history view (due to restore/trimming). Hidden from logs/timeline;
    /// still listed in the Processes tab.
    pub dropped: bool,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, TS)]
pub struct CreateExecutionProcess {
    pub session_id: Uuid,
    pub executor_action: ExecutorAction,
    pub run_reason: ExecutionProcessRunReason,
}

#[derive(Debug)]
pub struct ExecutionContext {
    pub execution_process: ExecutionProcess,
    pub session: Session,
    pub workspace: Workspace,
    pub repos: Vec<Repo>,
}

/// Summary info about the latest execution process for a workspace
#[derive(Debug, Clone, FromRow)]
pub struct LatestProcessInfo {
    pub workspace_id: Uuid,
    pub execution_process_id: Uuid,
    pub session_id: Uuid,
    pub status: ExecutionProcessStatus,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExecutorActionField {
    ExecutorAction(Box<ExecutorAction>),
    Other(Value),
}

#[derive(Debug, Clone)]
pub struct MissingBeforeContext {
    pub id: Uuid,
    pub session_id: Uuid,
    pub workspace_id: Uuid,
    pub repo_id: Uuid,
    pub prev_after_head_commit: Option<String>,
    pub target_branch: String,
    pub repo_path: Option<String>,
}

impl ExecutionProcess {
    /// Find execution process by ID
    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            ExecutionProcess,
            r#"SELECT
                    ep.id as "id!: Uuid",
                    ep.session_id as "session_id!: Uuid",
                    ep.run_reason as "run_reason!: ExecutionProcessRunReason",
                    ep.executor_action as "executor_action!: sqlx::types::Json<ExecutorActionField>",
                    ep.status as "status!: ExecutionProcessStatus",
                    ep.exit_code,
                    ep.dropped as "dropped!: bool",
                    ep.started_at as "started_at!: DateTime<Utc>",
                    ep.completed_at as "completed_at?: DateTime<Utc>",
                    ep.created_at as "created_at!: DateTime<Utc>",
                    ep.updated_at as "updated_at!: DateTime<Utc>"
               FROM execution_processes ep WHERE ep.id = ?"#,
            id
        )
        .fetch_optional(pool)
        .await
    }

    /// Context for backfilling before_head_commit for legacy rows
    /// List processes that have after_head_commit set but missing before_head_commit, with join context
    pub async fn list_missing_before_context(
        pool: &SqlitePool,
    ) -> Result<Vec<MissingBeforeContext>, sqlx::Error> {
        let rows = sqlx::query!(
            r#"SELECT
                ep.id                         as "id!: Uuid",
                ep.session_id                 as "session_id!: Uuid",
                s.workspace_id                as "workspace_id!: Uuid",
                eprs.repo_id                  as "repo_id!: Uuid",
                eprs.after_head_commit        as after_head_commit,
                prev.after_head_commit        as prev_after_head_commit,
                wr.target_branch              as "target_branch!",
                r.path                        as repo_path
            FROM execution_processes ep
            JOIN sessions s ON s.id = ep.session_id
            JOIN execution_process_repo_states eprs ON eprs.execution_process_id = ep.id
            JOIN repos r ON r.id = eprs.repo_id
            JOIN workspaces w ON w.id = s.workspace_id
            JOIN workspace_repos wr ON wr.workspace_id = w.id AND wr.repo_id = eprs.repo_id
            LEFT JOIN execution_process_repo_states prev
              ON prev.execution_process_id = (
                   SELECT id FROM execution_processes
                     WHERE session_id = ep.session_id
                       AND created_at < ep.created_at
                     ORDER BY created_at DESC
                     LIMIT 1
               )
              AND prev.repo_id = eprs.repo_id
            WHERE eprs.before_head_commit IS NULL
              AND eprs.after_head_commit IS NOT NULL"#
        )
        .fetch_all(pool)
        .await?;

        let result = rows
            .into_iter()
            .map(|r| MissingBeforeContext {
                id: r.id,
                session_id: r.session_id,
                workspace_id: r.workspace_id,
                repo_id: r.repo_id,
                prev_after_head_commit: r.prev_after_head_commit,
                target_branch: r.target_branch,
                repo_path: Some(r.repo_path),
            })
            .collect();
        Ok(result)
    }

    /// Find execution process by rowid
    pub async fn find_by_rowid(pool: &SqlitePool, rowid: i64) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            ExecutionProcess,
            r#"SELECT
                    ep.id as "id!: Uuid",
                    ep.session_id as "session_id!: Uuid",
                    ep.run_reason as "run_reason!: ExecutionProcessRunReason",
                    ep.executor_action as "executor_action!: sqlx::types::Json<ExecutorActionField>",
                    ep.status as "status!: ExecutionProcessStatus",
                    ep.exit_code,
                    ep.dropped as "dropped!: bool",
                    ep.started_at as "started_at!: DateTime<Utc>",
                    ep.completed_at as "completed_at?: DateTime<Utc>",
                    ep.created_at as "created_at!: DateTime<Utc>",
                    ep.updated_at as "updated_at!: DateTime<Utc>"
               FROM execution_processes ep WHERE ep.rowid = ?"#,
            rowid
        )
        .fetch_optional(pool)
        .await
    }

    /// Find all execution processes for a session (optionally include soft-deleted)
    pub async fn find_by_session_id(
        pool: &SqlitePool,
        session_id: Uuid,
        show_soft_deleted: bool,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            ExecutionProcess,
            r#"SELECT
                      ep.id              as "id!: Uuid",
                      ep.session_id      as "session_id!: Uuid",
                      ep.run_reason      as "run_reason!: ExecutionProcessRunReason",
                      ep.executor_action as "executor_action!: sqlx::types::Json<ExecutorActionField>",
                      ep.status          as "status!: ExecutionProcessStatus",
                      ep.exit_code,
                      ep.dropped as "dropped!: bool",
                      ep.started_at      as "started_at!: DateTime<Utc>",
                      ep.completed_at    as "completed_at?: DateTime<Utc>",
                      ep.created_at      as "created_at!: DateTime<Utc>",
                      ep.updated_at      as "updated_at!: DateTime<Utc>"
               FROM execution_processes ep
               WHERE ep.session_id = ?
                 AND (? OR ep.dropped = FALSE)
               ORDER BY ep.created_at ASC"#,
            session_id,
            show_soft_deleted
        )
        .fetch_all(pool)
        .await
    }

    /// Find running execution processes
    pub async fn find_running(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            ExecutionProcess,
            r#"SELECT
                    ep.id as "id!: Uuid",
                    ep.session_id as "session_id!: Uuid",
                    ep.run_reason as "run_reason!: ExecutionProcessRunReason",
                    ep.executor_action as "executor_action!: sqlx::types::Json<ExecutorActionField>",
                    ep.status as "status!: ExecutionProcessStatus",
                    ep.exit_code,
                    ep.dropped as "dropped!: bool",
                    ep.started_at as "started_at!: DateTime<Utc>",
                    ep.completed_at as "completed_at?: DateTime<Utc>",
                    ep.created_at as "created_at!: DateTime<Utc>",
                    ep.updated_at as "updated_at!: DateTime<Utc>"
               FROM execution_processes ep WHERE ep.status = 'running' ORDER BY ep.created_at ASC"#,
        )
        .fetch_all(pool)
        .await
    }

    /// Check if there's a running coding agent process for a session
    pub async fn has_running_coding_agent_for_session(
        pool: &SqlitePool,
        session_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let count: i64 = sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!: i64"
               FROM execution_processes ep
               WHERE ep.session_id = $1
                 AND ep.status = 'running'
                 AND ep.run_reason = 'codingagent'"#,
            session_id
        )
        .fetch_one(pool)
        .await?;
        Ok(count > 0)
    }

    pub async fn count_running_coding_agents(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution_processes \
             WHERE status = 'running' AND run_reason = 'codingagent'",
        )
        .fetch_one(pool)
        .await
    }

    /// Check if there are running processes (excluding dev servers) for a workspace (across all sessions)
    pub async fn has_running_non_dev_server_processes_for_workspace(
        pool: &SqlitePool,
        workspace_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let count: i64 = sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!: i64"
               FROM execution_processes ep
               JOIN sessions s ON ep.session_id = s.id
               WHERE s.workspace_id = $1
                 AND ep.status = 'running'
                 AND ep.run_reason != 'devserver'"#,
            workspace_id
        )
        .fetch_one(pool)
        .await?;
        Ok(count > 0)
    }

    /// Find running dev servers for a specific workspace (across all sessions)
    pub async fn find_running_dev_servers_by_workspace(
        pool: &SqlitePool,
        workspace_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as!(
            ExecutionProcess,
            r#"
        SELECT
            ep.id as "id!: Uuid",
            ep.session_id as "session_id!: Uuid",
            ep.run_reason as "run_reason!: ExecutionProcessRunReason",
            ep.executor_action as "executor_action!: sqlx::types::Json<ExecutorActionField>",
            ep.status as "status!: ExecutionProcessStatus",
            ep.exit_code,
            ep.dropped as "dropped!: bool",
            ep.started_at as "started_at!: DateTime<Utc>",
            ep.completed_at as "completed_at?: DateTime<Utc>",
            ep.created_at as "created_at!: DateTime<Utc>",
            ep.updated_at as "updated_at!: DateTime<Utc>"
        FROM execution_processes ep
        JOIN sessions s ON ep.session_id = s.id
        WHERE s.workspace_id = ?
          AND ep.status = 'running'
          AND ep.run_reason = 'devserver'
        ORDER BY ep.created_at DESC
        "#,
            workspace_id
        )
        .fetch_all(pool)
        .await
    }

    /// Find latest execution process by session and run reason
    /// Find latest execution process by workspace and run reason (across all sessions)
    pub async fn find_latest_by_workspace_and_run_reason(
        pool: &SqlitePool,
        workspace_id: Uuid,
        run_reason: &ExecutionProcessRunReason,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as!(
            ExecutionProcess,
            r#"SELECT
                    ep.id as "id!: Uuid",
                    ep.session_id as "session_id!: Uuid",
                    ep.run_reason as "run_reason!: ExecutionProcessRunReason",
                    ep.executor_action as "executor_action!: sqlx::types::Json<ExecutorActionField>",
                    ep.status as "status!: ExecutionProcessStatus",
                    ep.exit_code,
                    ep.dropped as "dropped!: bool",
                    ep.started_at as "started_at!: DateTime<Utc>",
                    ep.completed_at as "completed_at?: DateTime<Utc>",
                    ep.created_at as "created_at!: DateTime<Utc>",
                    ep.updated_at as "updated_at!: DateTime<Utc>"
               FROM execution_processes ep
               JOIN sessions s ON ep.session_id = s.id
               WHERE s.workspace_id = ? AND ep.run_reason = ? AND ep.dropped = FALSE
               ORDER BY ep.created_at DESC LIMIT 1"#,
            workspace_id,
            run_reason
        )
        .fetch_optional(pool)
        .await
    }

    /// Create a new execution process
    ///
    /// Note: We intentionally avoid using a transaction here. SQLite update
    /// hooks fire during transactions (before commit), and the hook spawns an
    /// async task that queries `find_by_rowid` on a different connection.
    /// If we used a transaction, that query would not see the uncommitted row,
    /// causing the WebSocket event to be lost.
    pub async fn create(
        pool: &SqlitePool,
        data: &CreateExecutionProcess,
        process_id: Uuid,
        repo_states: &[CreateExecutionProcessRepoState],
    ) -> Result<Self, sqlx::Error> {
        let now = Utc::now();
        let executor_action_json = sqlx::types::Json(&data.executor_action);

        sqlx::query!(
            r#"INSERT INTO execution_processes (
                    id, session_id, run_reason, executor_action,
                    status, exit_code, started_at, completed_at, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            process_id,
            data.session_id,
            data.run_reason,
            executor_action_json,
            ExecutionProcessStatus::Running,
            None::<i64>,
            now,
            None::<DateTime<Utc>>,
            now,
            now
        )
        .execute(pool)
        .await?;

        ExecutionProcessRepoState::create_many(pool, process_id, repo_states).await?;

        Self::find_by_id(pool, process_id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn was_stopped(pool: &SqlitePool, id: Uuid) -> bool {
        if let Ok(exp_process) = Self::find_by_id(pool, id).await
            && exp_process.is_some_and(|ep| {
                ep.status == ExecutionProcessStatus::Killed
                    || ep.status == ExecutionProcessStatus::Completed
            })
        {
            return true;
        }
        false
    }

    /// Update execution process status and completion info
    pub async fn update_completion(
        pool: &SqlitePool,
        id: Uuid,
        status: ExecutionProcessStatus,
        exit_code: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        let completed_at = if matches!(status, ExecutionProcessStatus::Running) {
            None
        } else {
            Some(Utc::now())
        };

        sqlx::query!(
            r#"UPDATE execution_processes
               SET status = $1, exit_code = $2, completed_at = $3
               WHERE id = $4"#,
            status,
            exit_code,
            completed_at,
            id
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Exact-once completion of a running attempt. The single winner also
    /// records the normalized outcome (when provided) in the same
    /// transaction, so a stale duplicate completion can neither flip the
    /// status nor attach a competing classification.
    pub async fn complete_running_attempt(
        pool: &SqlitePool,
        id: Uuid,
        status: ExecutionProcessStatus,
        exit_code: Option<i64>,
        outcome: Option<&executors::outcome::NormalizedExecutionOutcome>,
    ) -> Result<bool, sqlx::Error> {
        let completed_at = if matches!(status, ExecutionProcessStatus::Running) {
            None
        } else {
            Some(Utc::now())
        };

        let mut transaction = pool.begin().await?;
        let result = sqlx::query(
            "UPDATE execution_processes \
             SET status = ?, exit_code = ?, completed_at = ? \
             WHERE id = ? AND status = 'running'",
        )
        .bind(status)
        .bind(exit_code)
        .bind(completed_at)
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        let won = result.rows_affected() == 1;

        if won && let Some(outcome) = outcome {
            sqlx::query(
                "INSERT OR IGNORE INTO execution_process_outcomes \
                 (execution_process_id, outcome) VALUES (?, ?)",
            )
            .bind(id)
            .bind(sqlx::types::Json(outcome))
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;

        Ok(won)
    }

    pub fn executor_action(&self) -> Result<&ExecutorAction, anyhow::Error> {
        match &self.executor_action.0 {
            ExecutorActionField::ExecutorAction(action) => Ok(action.as_ref()),
            ExecutorActionField::Other(_) => Err(anyhow::anyhow!(
                "Executor action is not a valid ExecutorAction JSON object"
            )),
        }
    }

    /// Soft-drop processes at and after the specified boundary (inclusive)
    pub async fn drop_at_and_after(
        pool: &SqlitePool,
        session_id: Uuid,
        boundary_process_id: Uuid,
    ) -> Result<i64, sqlx::Error> {
        let result = sqlx::query!(
            r#"UPDATE execution_processes
               SET dropped = TRUE
             WHERE session_id = $1
               AND created_at >= (SELECT created_at FROM execution_processes WHERE id = $2)
               AND dropped = FALSE"#,
            session_id,
            boundary_process_id
        )
        .execute(pool)
        .await?;
        Ok(result.rows_affected() as i64)
    }

    /// Find the previous process's after_head_commit before the given boundary process
    /// for a specific repository
    pub async fn find_prev_after_head_commit(
        pool: &SqlitePool,
        session_id: Uuid,
        boundary_process_id: Uuid,
        repo_id: Uuid,
    ) -> Result<Option<String>, sqlx::Error> {
        let result = sqlx::query_scalar!(
            r#"SELECT eprs.after_head_commit
               FROM execution_process_repo_states eprs
               JOIN execution_processes ep ON ep.id = eprs.execution_process_id
              WHERE ep.session_id = $1
                AND eprs.repo_id = $2
                AND ep.created_at < (SELECT created_at FROM execution_processes WHERE id = $3)
              ORDER BY ep.created_at DESC
              LIMIT 1"#,
            session_id,
            repo_id,
            boundary_process_id
        )
        .fetch_optional(pool)
        .await?;
        Ok(result.flatten())
    }

    /// Get both the parent Workspace and Session for this execution process
    pub async fn parent_workspace_and_session(
        &self,
        pool: &SqlitePool,
    ) -> Result<Option<(Workspace, Session)>, sqlx::Error> {
        let session = match Session::find_by_id(pool, self.session_id).await? {
            Some(s) => s,
            None => return Ok(None),
        };
        let workspace = match Workspace::find_by_id(pool, session.workspace_id).await? {
            Some(w) => w,
            None => return Ok(None),
        };
        Ok(Some((workspace, session)))
    }

    /// Load execution context with related session, workspace, task, project, and repos
    pub async fn load_context(
        pool: &SqlitePool,
        exec_id: Uuid,
    ) -> Result<ExecutionContext, sqlx::Error> {
        let execution_process = Self::find_by_id(pool, exec_id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)?;

        let session = Session::find_by_id(pool, execution_process.session_id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)?;

        let workspace = Workspace::find_by_id(pool, session.workspace_id)
            .await?
            .ok_or(sqlx::Error::RowNotFound)?;

        let repos = WorkspaceRepo::find_repos_for_workspace(pool, workspace.id).await?;

        Ok(ExecutionContext {
            execution_process,
            session,
            workspace,
            repos,
        })
    }

    /// Fetch the latest CodingAgent executor profile for a session.
    /// Returns None if no CodingAgent execution process exists for this session.
    pub async fn latest_executor_profile_for_session(
        pool: &SqlitePool,
        session_id: Uuid,
    ) -> Result<Option<ExecutorProfileId>, ExecutionProcessError> {
        // Find the latest CodingAgent execution process for this session
        let latest_execution_process = sqlx::query_as!(
            ExecutionProcess,
            r#"SELECT
                    ep.id as "id!: Uuid",
                    ep.session_id as "session_id!: Uuid",
                    ep.run_reason as "run_reason!: ExecutionProcessRunReason",
                    ep.executor_action as "executor_action!: sqlx::types::Json<ExecutorActionField>",
                    ep.status as "status!: ExecutionProcessStatus",
                    ep.exit_code,
                    ep.dropped as "dropped!: bool",
                    ep.started_at as "started_at!: DateTime<Utc>",
                    ep.completed_at as "completed_at?: DateTime<Utc>",
                    ep.created_at as "created_at!: DateTime<Utc>",
                    ep.updated_at as "updated_at!: DateTime<Utc>"
               FROM execution_processes ep
               WHERE ep.session_id = ? AND ep.run_reason = ? AND ep.dropped = FALSE
               ORDER BY ep.created_at DESC LIMIT 1"#,
            session_id,
            ExecutionProcessRunReason::CodingAgent
        )
        .fetch_optional(pool)
        .await?;

        let Some(latest_execution_process) = latest_execution_process else {
            return Ok(None);
        };

        let action = latest_execution_process
            .executor_action()
            .map_err(|e| ExecutionProcessError::ValidationError(e.to_string()))?;

        match &action.typ {
            ExecutorActionType::CodingAgentInitialRequest(request) => {
                Ok(Some(request.executor_config.profile_id()))
            }
            ExecutorActionType::CodingAgentFollowUpRequest(request) => {
                Ok(Some(request.executor_config.profile_id()))
            }
            ExecutorActionType::ReviewRequest(request) => {
                Ok(Some(request.executor_config.profile_id()))
            }
            _ => Err(ExecutionProcessError::ValidationError(
                "Couldn't find profile from initial request".to_string(),
            )),
        }
    }

    /// Fetch the latest CodingAgent `ExecutorConfig` + provider id for a session.
    /// Returns `None` if no CodingAgent execution process exists. Used by team-
    /// spawn / team-send paths to inherit the caller's or recipient's last
    /// model/preset/permission/provider when the request omits them.
    pub async fn latest_executor_config_for_session(
        pool: &SqlitePool,
        session_id: Uuid,
    ) -> Result<Option<(ExecutorConfig, Option<String>)>, ExecutionProcessError> {
        let latest_execution_process = sqlx::query_as!(
            ExecutionProcess,
            r#"SELECT
                    ep.id as "id!: Uuid",
                    ep.session_id as "session_id!: Uuid",
                    ep.run_reason as "run_reason!: ExecutionProcessRunReason",
                    ep.executor_action as "executor_action!: sqlx::types::Json<ExecutorActionField>",
                    ep.status as "status!: ExecutionProcessStatus",
                    ep.exit_code,
                    ep.dropped as "dropped!: bool",
                    ep.started_at as "started_at!: DateTime<Utc>",
                    ep.completed_at as "completed_at?: DateTime<Utc>",
                    ep.created_at as "created_at!: DateTime<Utc>",
                    ep.updated_at as "updated_at!: DateTime<Utc>"
               FROM execution_processes ep
               WHERE ep.session_id = ? AND ep.run_reason = ? AND ep.dropped = FALSE
               ORDER BY ep.created_at DESC LIMIT 1"#,
            session_id,
            ExecutionProcessRunReason::CodingAgent
        )
        .fetch_optional(pool)
        .await?;

        let Some(latest_execution_process) = latest_execution_process else {
            return Ok(None);
        };

        let action = latest_execution_process
            .executor_action()
            .map_err(|e| ExecutionProcessError::ValidationError(e.to_string()))?;

        let cfg = match &action.typ {
            ExecutorActionType::CodingAgentInitialRequest(request) => {
                request.executor_config.clone()
            }
            ExecutorActionType::CodingAgentFollowUpRequest(request) => {
                request.executor_config.clone()
            }
            ExecutorActionType::ReviewRequest(request) => request.executor_config.clone(),
            _ => {
                return Err(ExecutionProcessError::ValidationError(
                    "Couldn't find executor config from initial request".to_string(),
                ));
            }
        };

        Ok(Some((cfg, action.selected_provider_id.clone())))
    }

    /// Fetch latest execution process info for all workspaces with the given archived status.
    /// Returns a map of workspace_id -> LatestProcessInfo for the most recent
    /// non-dropped execution process (excluding dev servers).
    pub async fn find_latest_for_workspaces(
        pool: &SqlitePool,
        archived: bool,
    ) -> Result<HashMap<Uuid, LatestProcessInfo>, sqlx::Error> {
        let rows: Vec<LatestProcessInfo> = sqlx::query_as!(
            LatestProcessInfo,
            r#"
            SELECT
                workspace_id as "workspace_id!: Uuid",
                execution_process_id as "execution_process_id!: Uuid",
                session_id as "session_id!: Uuid",
                status as "status!: ExecutionProcessStatus",
                completed_at as "completed_at?: DateTime<Utc>"
            FROM (
                SELECT
                    s.workspace_id,
                    ep.id as execution_process_id,
                    ep.session_id,
                    ep.status,
                    ep.completed_at,
                    ROW_NUMBER() OVER (
                        PARTITION BY s.workspace_id
                        ORDER BY ep.created_at DESC
                    ) as rn
                FROM execution_processes ep
                JOIN sessions s ON ep.session_id = s.id
                JOIN workspaces w ON s.workspace_id = w.id
                WHERE w.archived = $1
                  AND ep.run_reason IN ('codingagent', 'setupscript', 'cleanupscript')
                  AND ep.dropped = FALSE
            )
            WHERE rn = 1
            "#,
            archived
        )
        .fetch_all(pool)
        .await?;

        let result = rows
            .into_iter()
            .map(|info| (info.workspace_id, info))
            .collect();

        Ok(result)
    }

    /// Find all workspaces with running dev servers, filtered by archived status.
    /// Returns a set of workspace IDs that have at least one running dev server.
    pub async fn find_workspaces_with_running_dev_servers(
        pool: &SqlitePool,
        archived: bool,
    ) -> Result<HashSet<Uuid>, sqlx::Error> {
        let rows: Vec<Uuid> = sqlx::query_scalar!(
            r#"
            SELECT DISTINCT s.workspace_id as "workspace_id!: Uuid"
            FROM execution_processes ep
            JOIN sessions s ON ep.session_id = s.id
            JOIN workspaces w ON s.workspace_id = w.id
            WHERE w.archived = $1
              AND ep.status = 'running'
              AND ep.run_reason = 'devserver'
            "#,
            archived
        )
        .fetch_all(pool)
        .await?;

        Ok(rows.into_iter().collect())
    }
}
